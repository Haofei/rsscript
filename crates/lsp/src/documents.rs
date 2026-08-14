//! Open-document state and text synchronization.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use rsscript_language_service::{
    Diagnostic as RsDiagnostic, DocumentKind as ServiceDocumentKind, LanguageService,
};
use tower_lsp::lsp_types::*;

use crate::scheduler::*;
use crate::source_index::*;
use crate::text::apply_change;
use crate::workspace::*;

#[derive(Clone)]
pub(crate) struct Document {
    pub(crate) text: Arc<str>,
    pub(crate) diagnostics: Arc<Vec<RsDiagnostic>>,
    pub(crate) revision: u64,
    pub(crate) version: i32,
    pub(crate) sync_state: DocumentSyncState,
    pub(crate) source_index: Arc<SourceIndexCache>,
}

impl Document {
    pub(crate) fn symbol_index(&self, path: &str) -> Arc<rsscript_language_service::SymbolIndex> {
        self.source_index.get(
            SourceIndexIdentity {
                document_revision: self.revision,
                semantic_generation: 0,
            },
            path,
            &self.text,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentSyncState {
    Synchronized,
    Desynchronized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChangeFailure {
    MissingDocument,
    InvalidRange,
    OversizedDocument,
    FullSyncRequired,
}

pub(crate) enum ChangeOutcome {
    Applied(Box<AnalysisJob>),
    IgnoredStale,
    Desynchronized(ChangeFailure),
}

#[cfg(test)]
impl ChangeOutcome {
    pub(crate) fn expect_applied(self, message: &str) -> AnalysisJob {
        match self {
            Self::Applied(job) => *job,
            Self::IgnoredStale => panic!("{message}: stale change"),
            Self::Desynchronized(reason) => panic!("{message}: {reason:?}"),
        }
    }

    pub(crate) fn is_applied(&self) -> bool {
        matches!(self, Self::Applied(_))
    }
}

pub(crate) struct DocumentStore {
    pub(crate) documents: HashMap<Url, Document>,
    pub(crate) language_service: LanguageService,
    pub(crate) next_revision: u64,
    pub(crate) generations: HashMap<AnalysisKey, u64>,
}

impl DocumentStore {
    pub(crate) fn new() -> Self {
        Self {
            documents: HashMap::new(),
            language_service: LanguageService::new(),
            next_revision: 1,
            generations: HashMap::new(),
        }
    }

    pub(crate) fn allocate_revision(&mut self, analysis_key: &AnalysisKey) -> u64 {
        let revision = self.next_revision;
        self.next_revision += 1;
        self.generations.insert(analysis_key.clone(), revision);
        revision
    }

    pub(crate) fn generation(&self, analysis_key: &AnalysisKey) -> u64 {
        self.generations.get(analysis_key).copied().unwrap_or(0)
    }
}

fn service_document_kind(uri: &Url) -> ServiceDocumentKind {
    if uri.path().ends_with(".rssi") {
        ServiceDocumentKind::Interface
    } else {
        ServiceDocumentKind::Source
    }
}

fn sync_language_service(documents: &mut DocumentStore, uri: &Url) {
    if let Some(document) = documents.get(uri).cloned() {
        documents.language_service.set_file(
            uri.path(),
            document.revision,
            service_document_kind(uri),
            document.text,
        );
    }
}

impl Deref for DocumentStore {
    type Target = HashMap<Url, Document>;

    fn deref(&self) -> &Self::Target {
        &self.documents
    }
}

impl DerefMut for DocumentStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.documents
    }
}

#[derive(Clone)]

pub(crate) struct AnalysisJob {
    pub(crate) analysis_key: AnalysisKey,
    pub(crate) uri: Url,
    pub(crate) revision: u64,
    pub(crate) version: i32,
    pub(crate) generation: u64,
    pub(crate) open_documents: Arc<HashMap<Url, Document>>,
    pub(crate) cancellation: Arc<AnalysisCancellation>,
}

pub(crate) fn commit_diagnostics_if_current(
    documents: &mut HashMap<Url, Document>,
    uri: &Url,
    revision: u64,
    version: i32,
    generation: u64,
    current_generation: u64,
    diagnostics: Vec<RsDiagnostic>,
) -> bool {
    if generation != current_generation {
        return false;
    }
    let Some(document) = documents.get_mut(uri) else {
        return false;
    };
    if document.revision != revision || document.version != version {
        return false;
    }
    if document.sync_state != DocumentSyncState::Synchronized {
        return false;
    }
    document.diagnostics = Arc::new(diagnostics);
    true
}

pub(crate) fn analysis_job(
    documents: &DocumentStore,
    uri: Url,
    revision: u64,
    version: i32,
) -> AnalysisJob {
    let analysis_key = analysis_key_for_uri(&uri);
    let open_documents = documents
        .iter()
        .filter(|(candidate, document)| {
            document.sync_state == DocumentSyncState::Synchronized
                && analysis_key_for_uri(candidate) == analysis_key
        })
        .map(|(uri, document)| (uri.clone(), document.clone()))
        .collect();
    AnalysisJob {
        analysis_key: analysis_key.clone(),
        uri,
        revision,
        version,
        generation: documents.generation(&analysis_key),
        open_documents: Arc::new(open_documents),
        cancellation: Arc::new(AnalysisCancellation::default()),
    }
}

pub(crate) fn open_document(
    documents: &mut DocumentStore,
    uri: Url,
    text: String,
    version: i32,
) -> Option<AnalysisJob> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return None;
    }
    if documents
        .get(&uri)
        .is_some_and(|document| version <= document.version)
    {
        return None;
    }

    let analysis_key = analysis_key_for_uri(&uri);
    let revision = documents.allocate_revision(&analysis_key);
    documents.insert(
        uri.clone(),
        Document {
            text: Arc::from(text),
            diagnostics: Arc::new(Vec::new()),
            revision,
            version,
            sync_state: DocumentSyncState::Synchronized,
            source_index: Arc::new(SourceIndexCache::default()),
        },
    );
    sync_language_service(documents, &uri);
    Some(analysis_job(documents, uri, revision, version))
}

pub(crate) fn change_document(
    documents: &mut DocumentStore,
    uri: Url,
    version: i32,
    changes: &[TextDocumentContentChangeEvent],
) -> ChangeOutcome {
    let Some(document) = documents.get(&uri) else {
        return ChangeOutcome::Desynchronized(ChangeFailure::MissingDocument);
    };
    if version <= document.version {
        return ChangeOutcome::IgnoredStale;
    }
    if document.sync_state == DocumentSyncState::Desynchronized
        && changes.first().is_none_or(|change| change.range.is_some())
    {
        mark_document_desynchronized(documents, &uri, version);
        return ChangeOutcome::Desynchronized(ChangeFailure::FullSyncRequired);
    }

    let mut text = document.text.to_string();
    for change in changes {
        if !apply_change(&mut text, change) {
            mark_document_desynchronized(documents, &uri, version);
            return ChangeOutcome::Desynchronized(ChangeFailure::InvalidRange);
        }
        if text.len() > MAX_DOCUMENT_BYTES {
            mark_document_desynchronized(documents, &uri, version);
            return ChangeOutcome::Desynchronized(ChangeFailure::OversizedDocument);
        }
    }

    let analysis_key = analysis_key_for_uri(&uri);
    let revision = documents.allocate_revision(&analysis_key);
    let document = documents
        .get_mut(&uri)
        .expect("document remains present while the store is locked");
    document.text = Arc::from(text);
    document.diagnostics = Arc::new(Vec::new());
    document.revision = revision;
    document.version = version;
    document.sync_state = DocumentSyncState::Synchronized;
    document.source_index = Arc::new(SourceIndexCache::default());
    sync_language_service(documents, &uri);
    ChangeOutcome::Applied(Box::new(analysis_job(documents, uri, revision, version)))
}

pub(crate) fn mark_document_desynchronized(documents: &mut DocumentStore, uri: &Url, version: i32) {
    let analysis_key = analysis_key_for_uri(uri);
    let revision = documents.allocate_revision(&analysis_key);
    if let Some(document) = documents.get_mut(uri) {
        document.diagnostics = Arc::new(Vec::new());
        document.revision = revision;
        document.version = version;
        document.sync_state = DocumentSyncState::Desynchronized;
        document.source_index = Arc::new(SourceIndexCache::default());
    }
    documents.language_service.remove_file(uri.path());
}

pub(crate) fn save_document(
    documents: &mut DocumentStore,
    uri: Url,
    text: String,
) -> Option<AnalysisJob> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return None;
    }
    let version = documents.get(&uri)?.version;
    let analysis_key = analysis_key_for_uri(&uri);
    let revision = documents.allocate_revision(&analysis_key);
    let document = documents
        .get_mut(&uri)
        .expect("document remains present while the store is locked");
    document.text = Arc::from(text);
    document.diagnostics = Arc::new(Vec::new());
    document.revision = revision;
    document.sync_state = DocumentSyncState::Synchronized;
    document.source_index = Arc::new(SourceIndexCache::default());
    sync_language_service(documents, &uri);
    Some(analysis_job(documents, uri, revision, version))
}
