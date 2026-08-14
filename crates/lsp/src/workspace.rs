//! Package discovery, workspace snapshots, and package input caching.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rsscript_language_service::*;
use rsscript_workspace_loader::{WorkspaceFileKind, WorkspaceLoader};
use tower_lsp::lsp_types::*;

use crate::documents::*;
use crate::scheduler::*;
use crate::source_index::*;

#[derive(Clone)]
pub(crate) struct WorkspaceDocument {
    pub(crate) uri: Url,
    pub(crate) text: Arc<str>,
    pub(crate) kind: Option<WorkspaceFileKind>,
    pub(crate) revision: u64,
    pub(crate) semantic_generation: u64,
    pub(crate) source_index: Arc<SourceIndexCache>,
}

impl WorkspaceDocument {
    pub(crate) fn symbol_index(&self) -> Arc<SymbolIndex> {
        self.source_index.get(
            SourceIndexIdentity {
                document_revision: self.revision,
                semantic_generation: self.semantic_generation,
            },
            self.uri.path(),
            &self.text,
        )
    }
}

pub(crate) struct PackageInputCache {
    pub(crate) documents: Mutex<HashMap<PathBuf, CachedPackageInput>>,
    generations: Mutex<HashMap<PathBuf, u64>>,
    next_generation: AtomicU64,
}

pub(crate) struct CachedPackageInput {
    generation: u64,
    documents: Arc<HashMap<Url, WorkspaceDocument>>,
}

impl Default for PackageInputCache {
    fn default() -> Self {
        Self {
            documents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
        }
    }
}

impl PackageInputCache {
    pub(crate) fn lock_documents(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PathBuf, CachedPackageInput>> {
        self.documents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn documents_for_root(
        &self,
        package_root: &Path,
    ) -> Arc<HashMap<Url, WorkspaceDocument>> {
        loop {
            let generation = self.generation_for_root(package_root);
            if let Some(documents) = self
                .lock_documents()
                .get(package_root)
                .filter(|entry| entry.generation == generation)
                .map(|entry| Arc::clone(&entry.documents))
            {
                return documents;
            }

            let documents = Arc::new(load_package_documents_at_generation(
                package_root,
                generation,
            ));
            let mut cache = self.lock_documents();
            if self.generation_for_root(package_root) != generation {
                continue;
            }
            let entry =
                cache
                    .entry(package_root.to_path_buf())
                    .or_insert_with(|| CachedPackageInput {
                        generation,
                        documents: Arc::clone(&documents),
                    });
            if entry.generation == generation {
                return Arc::clone(&entry.documents);
            }
            *entry = CachedPackageInput {
                generation,
                documents: Arc::clone(&documents),
            };
            return documents;
        }
    }

    pub(crate) fn invalidate(&self, package_root: &Path) {
        self.advance_generation(package_root);
        self.lock_documents().remove(package_root);
    }

    pub(crate) fn invalidate_path(&self, changed_path: &Path) -> Vec<PathBuf> {
        let mut invalidated = self
            .lock_documents()
            .keys()
            .filter(|package_root| changed_path.starts_with(package_root))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(package_root) = package_root_for_path(changed_path)
            && !invalidated.contains(&package_root)
        {
            invalidated.push(package_root);
        }
        for package_root in &invalidated {
            self.advance_generation(package_root);
        }
        self.lock_documents()
            .retain(|package_root, _| !invalidated.contains(package_root));
        invalidated
    }

    pub(crate) fn generation_for_root(&self, package_root: &Path) -> u64 {
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *generations
            .entry(package_root.to_path_buf())
            .or_insert_with(|| self.next_generation.fetch_add(1, Ordering::Relaxed))
    }

    fn advance_generation(&self, package_root: &Path) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(package_root.to_path_buf(), generation);
    }
}

pub(crate) fn workspace_documents_for_uri(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
    package_inputs: &PackageInputCache,
) -> Vec<WorkspaceDocument> {
    let mut documents = package_root_for_uri(uri)
        .map(|root| {
            let documents = package_inputs.documents_for_root(&root);
            (*documents).clone()
        })
        .unwrap_or_default();
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

pub(crate) fn workspace_documents_from_base(
    base: &HashMap<Url, WorkspaceDocument>,
    open_documents: &HashMap<Url, Document>,
) -> Vec<WorkspaceDocument> {
    let mut documents = base.clone();
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

pub(crate) fn workspace_documents(
    open_documents: &HashMap<Url, Document>,
    package_inputs: &PackageInputCache,
) -> Vec<WorkspaceDocument> {
    let mut documents = HashMap::new();
    for uri in open_documents.keys() {
        let Some(package_root) = package_root_for_uri(uri) else {
            continue;
        };
        documents.extend((*package_inputs.documents_for_root(&package_root)).clone());
    }
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

pub(crate) fn overlay_open_documents(
    documents: &mut HashMap<Url, WorkspaceDocument>,
    open_documents: &HashMap<Url, Document>,
) {
    for (uri, document) in open_documents {
        documents.insert(
            uri.clone(),
            WorkspaceDocument {
                uri: uri.clone(),
                text: Arc::clone(&document.text),
                kind: infer_document_kind(uri),
                revision: document.revision,
                semantic_generation: 0,
                source_index: Arc::clone(&document.source_index),
            },
        );
    }
}

fn load_package_documents_at_generation(
    package_dir: &Path,
    semantic_generation: u64,
) -> HashMap<Url, WorkspaceDocument> {
    // Capture one immutable input before constructing editor documents. The
    // loader owns filesystem access; later language-service queries consume
    // only these captured bytes.
    let Ok(snapshot) = WorkspaceLoader::default().snapshot_from(package_dir, Path::new(".")) else {
        return HashMap::new();
    };
    snapshot
        .into_files()
        .into_iter()
        .filter_map(|source| {
            Url::from_file_path(PathBuf::from(&source.path))
                .ok()
                .map(|uri| {
                    (
                        uri.clone(),
                        WorkspaceDocument {
                            uri,
                            text: Arc::from(source.contents),
                            kind: Some(source.kind),
                            revision: 0,
                            semantic_generation,
                            source_index: Arc::new(SourceIndexCache::default()),
                        },
                    )
                })
        })
        .collect()
}

pub(crate) fn infer_document_kind(uri: &Url) -> Option<WorkspaceFileKind> {
    let path = uri.path();
    if path.ends_with(".rssi") {
        Some(WorkspaceFileKind::Interface)
    } else if path.ends_with(".rss") {
        Some(WorkspaceFileKind::Source)
    } else {
        None
    }
}

pub(crate) fn analysis_key_for_uri(uri: &Url) -> AnalysisKey {
    if let Some(package_root) = package_root_for_uri(uri) {
        return AnalysisKey::Package(package_root);
    }
    if uri.to_file_path().is_ok() {
        return AnalysisKey::Workspace;
    }
    AnalysisKey::Uri(uri.clone())
}

pub(crate) fn package_root_for_uri(uri: &Url) -> Option<PathBuf> {
    let path = uri.to_file_path().ok()?;
    package_root_for_path(&path)
}

pub(crate) fn package_root_for_path(path: &Path) -> Option<PathBuf> {
    find_package_root(path)
}

pub(crate) fn find_package_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() { path } else { path.parent()? };
    loop {
        if current.join("rsspkg.toml").is_file() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
