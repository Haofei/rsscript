//! Language server for RsScript.
//!
//! Reuses the `rsscript` checker library directly: diagnostics come from the
//! same `analyze_source_with_core` + `lint_source` path as the CLI, and
//! formatting from `format_source`, so the editor never disagrees with the
//! command line.

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

use rsscript::{
    Definition, Diagnostic as RsDiagnostic, PackageReviewFileKind, Reference, RssDocumentSymbol,
    Severity, Span, SymbolInfo, SymbolKind as RssSymbolKind, SymbolLookup,
    analyze_source_with_core, analyze_source_with_interfaces, analyze_sources_with_interfaces,
    document_symbols, explain_diagnostic_code, format_source, lint_source,
    package_sources_with_dependency_interfaces, symbol_index,
};
use serde_json::json;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// A document the editor has open, plus its most recent diagnostics (kept so
/// hover can explain whichever diagnostic the cursor is on).
#[derive(Clone)]
struct Document {
    text: Arc<str>,
    diagnostics: Arc<Vec<RsDiagnostic>>,
    revision: u64,
    version: i32,
}

struct DocumentStore {
    documents: HashMap<Url, Document>,
    next_revision: u64,
    generations: HashMap<AnalysisKey, u64>,
}

impl DocumentStore {
    fn new() -> Self {
        Self {
            documents: HashMap::new(),
            next_revision: 1,
            generations: HashMap::new(),
        }
    }

    fn allocate_revision(&mut self, analysis_key: &AnalysisKey) -> u64 {
        let revision = self.next_revision;
        self.next_revision += 1;
        self.generations.insert(analysis_key.clone(), revision);
        revision
    }

    fn generation(&self, analysis_key: &AnalysisKey) -> u64 {
        self.generations.get(analysis_key).copied().unwrap_or(0)
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
struct WorkspaceDocument {
    uri: Url,
    text: Arc<str>,
    kind: Option<PackageReviewFileKind>,
}

#[derive(Default)]
struct PackageInputCache {
    documents: Mutex<HashMap<PathBuf, Arc<HashMap<Url, WorkspaceDocument>>>>,
}

impl PackageInputCache {
    fn documents_for_root(&self, package_root: &Path) -> Arc<HashMap<Url, WorkspaceDocument>> {
        if let Some(documents) = self
            .documents
            .lock()
            .expect("package input cache lock poisoned")
            .get(package_root)
            .cloned()
        {
            return documents;
        }

        let documents = Arc::new(load_package_documents(package_root));
        let mut cache = self
            .documents
            .lock()
            .expect("package input cache lock poisoned");
        Arc::clone(cache.entry(package_root.to_path_buf()).or_insert(documents))
    }

    fn invalidate(&self, package_root: &Path) {
        self.documents
            .lock()
            .expect("package input cache lock poisoned")
            .remove(package_root);
    }
}

#[derive(Default)]
struct AnalysisCancellation {
    cancelled: AtomicBool,
}

impl AnalysisCancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

struct PendingAnalysis {
    task: tokio::task::AbortHandle,
    cancellation: Arc<AnalysisCancellation>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AnalysisKey {
    Package(PathBuf),
    Workspace,
    Uri(Url),
}

const MAX_BLOCKING_ANALYSES: usize = 2;

struct Backend {
    client: Client,
    documents: Arc<tokio::sync::Mutex<DocumentStore>>,
    diagnostics_publication: Arc<tokio::sync::Mutex<()>>,
    pending_analysis: tokio::sync::Mutex<HashMap<AnalysisKey, PendingAnalysis>>,
    package_inputs: Arc<PackageInputCache>,
    blocking_analysis_permits: Arc<Semaphore>,
}

struct AnalysisJob {
    analysis_key: AnalysisKey,
    uri: Url,
    revision: u64,
    version: i32,
    generation: u64,
    open_documents: Arc<HashMap<Url, Document>>,
    cancellation: Arc<AnalysisCancellation>,
}

fn commit_diagnostics_if_current(
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
    document.diagnostics = Arc::new(diagnostics);
    true
}

fn analysis_job(documents: &DocumentStore, uri: Url, revision: u64, version: i32) -> AnalysisJob {
    let analysis_key = analysis_key_for_uri(&uri);
    let open_documents = documents
        .iter()
        .filter(|(candidate, _)| analysis_key_for_uri(candidate) == analysis_key)
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

fn replace_pending_analysis(
    pending: &mut HashMap<AnalysisKey, PendingAnalysis>,
    analysis_key: AnalysisKey,
    task: PendingAnalysis,
) {
    if let Some(previous) = pending.insert(analysis_key, task) {
        previous.cancellation.cancel();
        previous.task.abort();
    }
}

fn open_document(
    documents: &mut DocumentStore,
    uri: Url,
    text: String,
    version: i32,
) -> Option<AnalysisJob> {
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
        },
    );
    Some(analysis_job(documents, uri, revision, version))
}

fn change_document(
    documents: &mut DocumentStore,
    uri: Url,
    version: i32,
    changes: &[TextDocumentContentChangeEvent],
) -> Option<AnalysisJob> {
    let document = documents.get(&uri)?;
    if version <= document.version {
        return None;
    }

    let mut text = document.text.to_string();
    for change in changes {
        apply_change(&mut text, change);
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
    Some(analysis_job(documents, uri, revision, version))
}

fn save_document(documents: &mut DocumentStore, uri: Url, text: String) -> Option<AnalysisJob> {
    let version = documents.get(&uri)?.version;
    let analysis_key = analysis_key_for_uri(&uri);
    let revision = documents.allocate_revision(&analysis_key);
    let document = documents
        .get_mut(&uri)
        .expect("document remains present while the store is locked");
    document.text = Arc::from(text);
    document.diagnostics = Arc::new(Vec::new());
    document.revision = revision;
    Some(analysis_job(documents, uri, revision, version))
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(tokio::sync::Mutex::new(DocumentStore::new())),
            diagnostics_publication: Arc::new(tokio::sync::Mutex::new(())),
            pending_analysis: tokio::sync::Mutex::new(HashMap::new()),
            package_inputs: Arc::new(PackageInputCache::default()),
            blocking_analysis_permits: Arc::new(Semaphore::new(MAX_BLOCKING_ANALYSES)),
        }
    }

    async fn cancel_pending_analysis(&self, analysis_key: &AnalysisKey) {
        if let Some(pending) = self.pending_analysis.lock().await.remove(analysis_key) {
            pending.cancellation.cancel();
            pending.task.abort();
        }
    }

    /// Debounce analysis for one package/workspace and cancel any superseded task.
    async fn schedule_analysis(&self, job: AnalysisJob) {
        let analysis_key = job.analysis_key.clone();
        let client = self.client.clone();
        let documents = Arc::clone(&self.documents);
        let diagnostics_publication = Arc::clone(&self.diagnostics_publication);
        let package_inputs = Arc::clone(&self.package_inputs);
        let blocking_analysis_permits = Arc::clone(&self.blocking_analysis_permits);
        let cancellation = Arc::clone(&job.cancellation);
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Self::analyze_and_publish(
                client,
                documents,
                diagnostics_publication,
                package_inputs,
                blocking_analysis_permits,
                job,
            )
            .await;
        });
        let mut pending = self.pending_analysis.lock().await;
        replace_pending_analysis(
            &mut pending,
            analysis_key,
            PendingAnalysis {
                task: task.abort_handle(),
                cancellation,
            },
        );
    }

    /// Run the checker over a stable document snapshot and publish if it is
    /// still the current revision.
    async fn analyze_and_publish(
        client: Client,
        documents: Arc<tokio::sync::Mutex<DocumentStore>>,
        diagnostics_publication: Arc<tokio::sync::Mutex<()>>,
        package_inputs: Arc<PackageInputCache>,
        blocking_analysis_permits: Arc<Semaphore>,
        job: AnalysisJob,
    ) {
        let AnalysisJob {
            analysis_key,
            uri,
            revision,
            version,
            generation,
            open_documents,
            cancellation,
        } = job;
        if cancellation.is_cancelled() {
            return;
        }
        let analysis_uri = uri.clone();
        let analysis_cancellation = Arc::clone(&cancellation);
        let analysis = run_bounded_blocking(blocking_analysis_permits, move || {
            if analysis_cancellation.is_cancelled() {
                return None;
            }
            let diagnostics = diagnostics_for_uri_cancellable(
                &analysis_uri,
                &open_documents,
                &package_inputs,
                || analysis_cancellation.is_cancelled(),
            )?;
            if analysis_cancellation.is_cancelled() {
                return None;
            }
            let lsp_diagnostics =
                lsp_diagnostics_from_diagnostics(&analysis_uri, &open_documents, &diagnostics);
            Some((diagnostics, lsp_diagnostics))
        })
        .await;
        let Ok(Some((diagnostics, lsp_diagnostics))) = analysis else {
            if cancellation.is_cancelled() {
                return;
            }
            client
                .log_message(MessageType::ERROR, "RSScript analysis task failed")
                .await;
            return;
        };

        if cancellation.is_cancelled() {
            return;
        }
        let _publication = diagnostics_publication.lock().await;
        if cancellation.is_cancelled() {
            return;
        }
        {
            let mut documents = documents.lock().await;
            let current_generation = documents.generation(&analysis_key);
            if !commit_diagnostics_if_current(
                &mut documents,
                &uri,
                revision,
                version,
                generation,
                current_generation,
                diagnostics,
            ) {
                return;
            }
        }
        client
            .publish_diagnostics(uri, lsp_diagnostics, Some(version))
            .await;
    }
}

async fn snapshot_documents(
    documents: &tokio::sync::Mutex<DocumentStore>,
) -> HashMap<Url, Document> {
    documents.lock().await.documents.clone()
}

async fn run_bounded_blocking<T, F>(
    permits: Arc<Semaphore>,
    work: F,
) -> std::result::Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let permit = permits
        .acquire_owned()
        .await
        .expect("blocking analysis semaphore closed");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "rss-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: Some("rsscript".to_string()),
                        inter_file_dependencies: true,
                        workspace_diagnostics: false,
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    },
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    ..SignatureHelpOptions::default()
                }),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensOptions {
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        legend: semantic_tokens_legend(),
                        range: None,
                        full: Some(SemanticTokensFullOptions::Bool(true)),
                    }
                    .into(),
                ),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "rss-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        let mut pending = self.pending_analysis.lock().await;
        for (_, analysis) in pending.drain() {
            analysis.cancellation.cancel();
            analysis.task.abort();
        }
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        let analysis_key = analysis_key_for_uri(&doc.uri);
        let _publication = self.diagnostics_publication.lock().await;
        self.cancel_pending_analysis(&analysis_key).await;
        let job = {
            let mut documents = self.documents.lock().await;
            open_document(&mut documents, doc.uri, doc.text, doc.version)
        };
        if let Some(job) = job {
            self.schedule_analysis(job).await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let analysis_key = analysis_key_for_uri(&params.text_document.uri);
        let _publication = self.diagnostics_publication.lock().await;
        self.cancel_pending_analysis(&analysis_key).await;
        let job = {
            let mut documents = self.documents.lock().await;
            change_document(
                &mut documents,
                params.text_document.uri,
                params.text_document.version,
                &params.content_changes,
            )
        };
        if let Some(job) = job {
            self.schedule_analysis(job).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let analysis_key = analysis_key_for_uri(&params.text_document.uri);
        let _publication = self.diagnostics_publication.lock().await;
        self.cancel_pending_analysis(&analysis_key).await;
        if let Some(package_root) = package_root_for_uri(&params.text_document.uri) {
            self.package_inputs.invalidate(&package_root);
        }
        if let Some(text) = params.text {
            let job = {
                let mut documents = self.documents.lock().await;
                save_document(&mut documents, params.text_document.uri, text)
            };
            if let Some(job) = job {
                self.schedule_analysis(job).await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let analysis_key = analysis_key_for_uri(&params.text_document.uri);
        let _publication = self.diagnostics_publication.lock().await;
        self.cancel_pending_analysis(&analysis_key).await;
        {
            let mut documents = self.documents.lock().await;
            documents.allocate_revision(&analysis_key);
            documents.remove(&params.text_document.uri);
        }
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let path = params.text_document.uri.path();
        let formatted = format_source(path, &document.text);
        if formatted == document.text.as_ref() {
            return Ok(None);
        }
        Ok(Some(vec![TextEdit {
            range: full_document_range(&document.text),
            new_text: formatted,
        }]))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let hovered = document.diagnostics.iter().find(|diagnostic| {
            let range = span_to_range(&document.text, &diagnostic.span);
            position_in_range(position, &range)
        });
        if let Some(diagnostic) = hovered {
            let mut markdown = format!("**{}** — {}", diagnostic.code, diagnostic.summary);
            if let Some(explanation) = explain_diagnostic_code(&diagnostic.code) {
                markdown.push_str("\n\n");
                markdown.push_str(explanation.explanation);
            }

            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(span_to_range(&document.text, &diagnostic.span)),
            }));
        }

        let (line, column) = char_position(&document.text, position);
        let index = symbol_index(uri.path(), &document.text);
        let Some(symbol) = hover_symbol_info(&uri, &documents, &index, line, column) else {
            return Ok(None);
        };
        let markdown = symbol_hover_markdown(&symbol);
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: Some(span_to_range(&document.text, &symbol.span)),
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let (line, column) = char_position(&document.text, position);
        let index = symbol_index(uri.path(), &document.text);
        if let Some(span) = index.definition_at(line, column) {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, span),
            })));
        }

        let Some(lookup) = index.lookup_at(line, column) else {
            return Ok(None);
        };
        let workspace_documents = workspace_documents_for_uri(&uri, &documents);
        let Some(location) = workspace_definition_location(&workspace_documents, &lookup) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(location)))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let documents = snapshot_documents(&self.documents).await;
        let locations = reference_locations_for_position(
            &uri,
            position,
            &documents,
            params.context.include_declaration,
        );
        if locations.is_empty() {
            return Ok(None);
        }
        Ok(Some(locations))
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = params.text_document.uri;
        let documents = snapshot_documents(&self.documents).await;
        let items = documents
            .get(&uri)
            .map(|document| {
                lsp_diagnostics_from_diagnostics(&uri, &documents, &document.diagnostics)
            })
            .unwrap_or_default();
        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let locations = reference_locations_for_position(&uri, position, &documents, true);
        let highlights = locations
            .into_iter()
            .filter(|location| location.uri == uri)
            .map(|location| DocumentHighlight {
                range: location.range,
                kind: Some(DocumentHighlightKind::READ),
            })
            .collect::<Vec<_>>();
        if highlights.is_empty() {
            let (line, column) = char_position(&document.text, position);
            let index = symbol_index(uri.path(), &document.text);
            let Some(symbol) = index.symbol_at(line, column) else {
                return Ok(None);
            };
            return Ok(Some(vec![DocumentHighlight {
                range: span_to_range(&document.text, &symbol.span),
                kind: Some(DocumentHighlightKind::TEXT),
            }]));
        }
        Ok(Some(highlights))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let symbols = document_symbols(uri.path(), &document.text)
            .into_iter()
            .map(|symbol| to_lsp_document_symbol(&document.text, symbol))
            .collect::<Vec<_>>();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };
        Ok(Some(SemanticTokensResult::Tokens(
            semantic_tokens_for_source(uri.path(), &document.text),
        )))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let documents = snapshot_documents(&self.documents).await;
        let workspace_documents = workspace_documents_for_uri(&uri, &documents);
        let Some(item) = call_hierarchy_item_at(&uri, position, &documents, &workspace_documents)
        else {
            return Ok(None);
        };
        Ok(Some(vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let documents = snapshot_documents(&self.documents).await;
        let workspace_documents = workspace_documents_for_uri(&params.item.uri, &documents);
        let calls = incoming_call_hierarchy(&workspace_documents, &params.item);
        if calls.is_empty() {
            return Ok(None);
        }
        Ok(Some(calls))
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let documents = snapshot_documents(&self.documents).await;
        let workspace_documents = workspace_documents_for_uri(&params.item.uri, &documents);
        let calls = outgoing_call_hierarchy(&workspace_documents, &params.item);
        if calls.is_empty() {
            return Ok(None);
        }
        Ok(Some(calls))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query.trim().to_lowercase();
        let documents = snapshot_documents(&self.documents).await;
        let mut symbols = Vec::new();
        for document in workspace_documents(&documents) {
            let index = symbol_index(document.uri.path(), &document.text);
            for definition in index.definitions() {
                if !query.is_empty() && !definition.name.to_lowercase().contains(&query) {
                    continue;
                }
                symbols.push(to_lsp_symbol_information(
                    &document.uri,
                    &document.text,
                    definition,
                ));
            }
        }
        Ok(Some(symbols))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let documents = snapshot_documents(&self.documents).await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };
        let Some(context) = call_context_at(&document.text, position) else {
            return Ok(None);
        };
        let workspace_documents = workspace_documents_for_uri(&uri, &documents);
        let Some(definition) = workspace_function_definition(&workspace_documents, &context.callee)
        else {
            return Ok(None);
        };
        let Some(signature) = signature_information(&definition, context.active_parameter) else {
            return Ok(None);
        };
        Ok(Some(SignatureHelp {
            signatures: vec![signature],
            active_signature: Some(0),
            active_parameter: Some(context.active_parameter as u32),
        }))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let documents = snapshot_documents(&self.documents).await;
        if !documents.contains_key(&uri) || !valid_rename_name(&params.new_name) {
            return Ok(None);
        }
        Ok(rename_workspace_edit(
            &uri,
            position,
            &params.new_name,
            &documents,
        ))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let documents = snapshot_documents(&self.documents).await;
        let Some((range, placeholder)) = rename_target(&uri, position, &documents) else {
            return Ok(None);
        };
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder,
        }))
    }
}

struct CallContext {
    callee: String,
    active_parameter: usize,
}

fn workspace_definition_location(
    documents: &[WorkspaceDocument],
    lookup: &SymbolLookup,
) -> Option<Location> {
    documents.iter().find_map(|document| {
        let index = symbol_index(document.uri.path(), &document.text);
        index
            .definitions()
            .iter()
            .find(|definition| definition_matches_lookup(definition, lookup))
            .map(|definition| Location {
                uri: document.uri.clone(),
                range: span_to_range(&document.text, &definition.span),
            })
    })
}

fn hover_symbol_info(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
    index: &rsscript::SymbolIndex,
    line: usize,
    column: usize,
) -> Option<SymbolInfo> {
    let symbol = index.symbol_at(line, column)?;
    if symbol.detail.is_some() {
        return Some(symbol);
    }
    let lookup = index.lookup_at(line, column)?;
    if lookup.local_definition.is_some() {
        return Some(symbol);
    }
    let workspace_documents = workspace_documents_for_uri(uri, open_documents);
    workspace_symbol_info(&workspace_documents, &lookup).or(Some(symbol))
}

fn workspace_symbol_info(
    documents: &[WorkspaceDocument],
    lookup: &SymbolLookup,
) -> Option<SymbolInfo> {
    documents.iter().find_map(|document| {
        let index = symbol_index(document.uri.path(), &document.text);
        index
            .definitions()
            .iter()
            .find(|definition| definition_matches_lookup(definition, lookup))
            .map(|definition| SymbolInfo {
                name: definition.name.clone(),
                kind: definition.kind,
                span: definition.span.clone(),
                detail: definition.detail.clone(),
            })
    })
}

fn symbol_hover_markdown(symbol: &SymbolInfo) -> String {
    let mut markdown = format!("**{}** `{}`", symbol_kind_label(symbol.kind), symbol.name);
    if let Some(detail) = &symbol.detail {
        markdown.push_str("\n\n```rss\n");
        markdown.push_str(detail);
        markdown.push_str("\n```");
    }
    markdown
}

fn call_context_at(source: &str, position: Position) -> Option<CallContext> {
    let cursor = byte_offset(source, position);
    let prefix = source.get(..cursor)?;
    let open = innermost_unclosed_call_open(prefix)?;
    let callee = callee_before_open(prefix, open)?;
    Some(CallContext {
        callee: normalize_callee_name(&callee),
        active_parameter: active_parameter_index(&prefix[open + 1..]),
    })
}

fn innermost_unclosed_call_open(prefix: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in prefix.char_indices().rev() {
        match character {
            ')' => depth += 1,
            '(' if depth == 0 => return Some(index),
            '(' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn callee_before_open(prefix: &str, open: usize) -> Option<String> {
    let before = prefix.get(..open)?.trim_end();
    let end = before.len();
    let start = before
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '<' | '>' | ',')
            {
                None
            } else {
                Some(index + character.len_utf8())
            }
        })
        .unwrap_or(0);
    let callee = before.get(start..end)?.trim();
    if callee.is_empty() {
        None
    } else {
        Some(callee.to_string())
    }
}

fn active_parameter_index(args_prefix: &str) -> usize {
    let mut depth = 0usize;
    let mut active = 0usize;
    for character in args_prefix.chars() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => active += 1,
            _ => {}
        }
    }
    active
}

fn normalize_callee_name(callee: &str) -> String {
    let mut normalized = String::new();
    let mut generic_depth = 0usize;
    for character in callee.chars() {
        match character {
            '<' => generic_depth += 1,
            '>' => generic_depth = generic_depth.saturating_sub(1),
            _ if generic_depth == 0 => normalized.push(character),
            _ => {}
        }
    }
    normalized
}

fn workspace_function_definition(
    documents: &[WorkspaceDocument],
    callee: &str,
) -> Option<Definition> {
    documents.iter().find_map(|document| {
        let index = symbol_index(document.uri.path(), &document.text);
        index
            .definitions()
            .iter()
            .find(|definition| {
                definition.name == callee && definition.kind == RssSymbolKind::Function
            })
            .cloned()
    })
}

fn call_hierarchy_item_at(
    uri: &Url,
    position: Position,
    open_documents: &HashMap<Url, Document>,
    documents: &[WorkspaceDocument],
) -> Option<CallHierarchyItem> {
    let document = open_documents.get(uri)?;
    let (line, column) = char_position(&document.text, position);
    let index = symbol_index(uri.path(), &document.text);
    if let Some(symbol) = index.symbol_at(line, column)
        && symbol.kind == RssSymbolKind::Function
    {
        let definition = find_function_definition(documents, &symbol.name)?;
        return Some(to_call_hierarchy_item(&document.text, uri, &definition));
    }
    let lookup = index.lookup_at(line, column)?;
    if lookup.is_type {
        return None;
    }
    let definition = find_function_definition(documents, &lookup.name)?;
    Some(to_call_hierarchy_item(&document.text, uri, &definition))
}

fn incoming_call_hierarchy(
    documents: &[WorkspaceDocument],
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyIncomingCall> {
    let mut calls_by_function: HashMap<(Url, String), (CallHierarchyItem, Vec<Range>)> =
        HashMap::new();
    for document in documents {
        let index = symbol_index(document.uri.path(), &document.text);
        for reference in index
            .references()
            .iter()
            .filter(|reference| reference.name == item.name && !reference.is_type)
        {
            let Some(caller) = enclosing_function_definition(&index, reference) else {
                continue;
            };
            if caller.name == item.name && document.uri == item.uri {
                continue;
            }
            let caller_item = to_call_hierarchy_item(&document.text, &document.uri, caller);
            calls_by_function
                .entry((document.uri.clone(), caller_item.name.clone()))
                .or_insert_with(|| (caller_item, Vec::new()))
                .1
                .push(span_to_range(&document.text, &reference.span));
        }
    }
    let mut calls = calls_by_function
        .into_values()
        .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
        .collect::<Vec<_>>();
    calls.sort_by(|left, right| {
        left.from
            .uri
            .as_str()
            .cmp(right.from.uri.as_str())
            .then_with(|| left.from.name.cmp(&right.from.name))
    });
    calls
}

fn outgoing_call_hierarchy(
    documents: &[WorkspaceDocument],
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyOutgoingCall> {
    let Some(document) = documents.iter().find(|document| document.uri == item.uri) else {
        return Vec::new();
    };
    let index = symbol_index(document.uri.path(), &document.text);
    let Some(caller) = index
        .definitions()
        .iter()
        .find(|definition| {
            definition.kind == RssSymbolKind::Function
                && definition.name == item.name
                && span_to_range(&document.text, &definition.span) == item.selection_range
        })
        .or_else(|| {
            index.definitions().iter().find(|definition| {
                definition.kind == RssSymbolKind::Function && definition.name == item.name
            })
        })
    else {
        return Vec::new();
    };
    let mut calls_by_function: HashMap<(Url, String), (CallHierarchyItem, Vec<Range>)> =
        HashMap::new();
    let caller_end_line = next_function_line(&index, caller).unwrap_or(usize::MAX);
    for reference in index.references().iter().filter(|reference| {
        !reference.is_type
            && reference.name != item.name
            && reference.span.line > caller.span.line
            && reference.span.line < caller_end_line
    }) {
        let Some((callee_document, callee_definition)) =
            find_function_definition_with_document(documents, &reference.name)
        else {
            continue;
        };
        let callee_item = to_call_hierarchy_item(
            &callee_document.text,
            &callee_document.uri,
            &callee_definition,
        );
        calls_by_function
            .entry((callee_document.uri.clone(), callee_item.name.clone()))
            .or_insert_with(|| (callee_item, Vec::new()))
            .1
            .push(span_to_range(&document.text, &reference.span));
    }
    let mut calls = calls_by_function
        .into_values()
        .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
        .collect::<Vec<_>>();
    calls.sort_by(|left, right| {
        left.to
            .uri
            .as_str()
            .cmp(right.to.uri.as_str())
            .then_with(|| left.to.name.cmp(&right.to.name))
    });
    calls
}

fn find_function_definition(documents: &[WorkspaceDocument], name: &str) -> Option<Definition> {
    documents.iter().find_map(|document| {
        let index = symbol_index(document.uri.path(), &document.text);
        index
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == RssSymbolKind::Function && definition.name == name
            })
            .cloned()
    })
}

fn find_function_definition_with_document<'a>(
    documents: &'a [WorkspaceDocument],
    name: &str,
) -> Option<(&'a WorkspaceDocument, Definition)> {
    documents.iter().find_map(|document| {
        let index = symbol_index(document.uri.path(), &document.text);
        index
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == RssSymbolKind::Function && definition.name == name
            })
            .cloned()
            .map(|definition| (document, definition))
    })
}

fn enclosing_function_definition<'a>(
    index: &'a rsscript::SymbolIndex,
    reference: &Reference,
) -> Option<&'a Definition> {
    index
        .definitions()
        .iter()
        .filter(|definition| {
            definition.kind == RssSymbolKind::Function && definition.span.line < reference.span.line
        })
        .max_by_key(|definition| definition.span.line)
}

fn to_call_hierarchy_item(source: &str, uri: &Url, definition: &Definition) -> CallHierarchyItem {
    let selection_range = span_to_range(source, &semantic_definition_span(source, definition));
    CallHierarchyItem {
        name: definition.name.clone(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        detail: definition.detail.clone(),
        uri: uri.clone(),
        range: selection_range,
        selection_range,
        data: None,
    }
}

fn next_function_line(index: &rsscript::SymbolIndex, function: &Definition) -> Option<usize> {
    index
        .definitions()
        .iter()
        .filter(|definition| {
            definition.kind == RssSymbolKind::Function && definition.span.line > function.span.line
        })
        .map(|definition| definition.span.line)
        .min()
}

fn signature_information(
    definition: &Definition,
    active_parameter: usize,
) -> Option<SignatureInformation> {
    let label = definition.detail.as_ref()?.clone();
    let parameters = signature_parameter_labels(&label)
        .into_iter()
        .map(|parameter| ParameterInformation {
            label: ParameterLabel::Simple(parameter),
            documentation: None,
        })
        .collect::<Vec<_>>();
    Some(SignatureInformation {
        label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: Some(active_parameter as u32),
    })
}

fn signature_parameter_labels(label: &str) -> Vec<String> {
    let Some(open) = label.find('(') else {
        return Vec::new();
    };
    let Some(close) = find_matching_paren_in_str(label, open) else {
        return Vec::new();
    };
    split_top_level_commas(&label[open + 1..close])
}

fn find_matching_paren_in_str(value: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in value.char_indices().skip_while(|(index, _)| *index < open) {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let part = value[start..index].trim();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    let part = value[start..].trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
    parts
}

fn workspace_reference_locations(
    documents: &[WorkspaceDocument],
    lookup: &SymbolLookup,
    include_declaration: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for document in documents {
        let index = symbol_index(document.uri.path(), &document.text);
        if include_declaration {
            for definition in index
                .definitions()
                .iter()
                .filter(|definition| definition_matches_lookup(definition, lookup))
            {
                locations.push(Location {
                    uri: document.uri.clone(),
                    range: span_to_range(&document.text, &definition.span),
                });
            }
        }
        for reference in index
            .references()
            .iter()
            .filter(|reference| unresolved_reference_matches_lookup(reference, lookup))
        {
            locations.push(Location {
                uri: document.uri.clone(),
                range: span_to_range(&document.text, &reference.span),
            });
        }
    }
    locations
}

fn reference_locations_for_position(
    uri: &Url,
    position: Position,
    open_documents: &HashMap<Url, Document>,
    include_declaration: bool,
) -> Vec<Location> {
    let Some(document) = open_documents.get(uri) else {
        return Vec::new();
    };
    let (line, column) = char_position(&document.text, position);
    let index = symbol_index(uri.path(), &document.text);
    let Some(lookup) = index.lookup_at(line, column) else {
        return Vec::new();
    };
    if lookup.local_definition.is_some() {
        return index
            .references_at(line, column, include_declaration)
            .into_iter()
            .map(|span| Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, &span),
            })
            .collect();
    }
    let workspace_documents = workspace_documents_for_uri(uri, open_documents);
    workspace_reference_locations(&workspace_documents, &lookup, include_declaration)
}

fn rename_target(
    uri: &Url,
    position: Position,
    open_documents: &HashMap<Url, Document>,
) -> Option<(Range, String)> {
    let document = open_documents.get(uri)?;
    let (line, column) = char_position(&document.text, position);
    let index = symbol_index(uri.path(), &document.text);
    index.lookup_at(line, column)?;
    let symbol = index.symbol_at(line, column)?;
    Some((span_to_range(&document.text, &symbol.span), symbol.name))
}

fn rename_workspace_edit(
    uri: &Url,
    position: Position,
    new_name: &str,
    open_documents: &HashMap<Url, Document>,
) -> Option<WorkspaceEdit> {
    let document = open_documents.get(uri)?;
    let (line, column) = char_position(&document.text, position);
    let index = symbol_index(uri.path(), &document.text);
    let lookup = index.lookup_at(line, column)?;
    let symbol = index.symbol_at(line, column)?;
    let locations = if lookup.local_definition.is_some()
        && matches!(symbol.kind, RssSymbolKind::Param | RssSymbolKind::Local)
    {
        index
            .references_at(line, column, true)
            .into_iter()
            .map(|span| Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, &span),
            })
            .collect::<Vec<_>>()
    } else {
        let workspace_documents = workspace_documents_for_uri(uri, open_documents);
        workspace_reference_locations(&workspace_documents, &lookup, true)
    };
    let changes = rename_changes(locations, new_name);
    if changes.is_empty() {
        None
    } else {
        Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        })
    }
}

fn rename_changes(locations: Vec<Location>, new_name: &str) -> HashMap<Url, Vec<TextEdit>> {
    let mut seen = HashSet::new();
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for location in locations {
        let key = (
            location.uri.clone(),
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        );
        if !seen.insert(key) {
            continue;
        }
        changes.entry(location.uri).or_default().push(TextEdit {
            range: location.range,
            new_text: new_name.to_string(),
        });
    }
    changes
}

fn valid_rename_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
fn diagnostics_for_uri(uri: &Url, open_documents: &HashMap<Url, Document>) -> Vec<RsDiagnostic> {
    diagnostics_for_uri_cancellable(uri, open_documents, &PackageInputCache::default(), || false)
        .unwrap_or_default()
}

fn diagnostics_for_uri_cancellable(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
    package_inputs: &PackageInputCache,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Vec<RsDiagnostic>> {
    let document = open_documents.get(uri)?;
    if cancelled() {
        return None;
    }
    let Some(package_root) = package_root_for_uri(uri) else {
        let mut diagnostics = analyze_source_with_core(uri.path(), &document.text);
        if cancelled() {
            return None;
        }
        diagnostics.extend(lint_source(uri.path(), &document.text));
        return (!cancelled()).then_some(diagnostics);
    };

    let package_documents = package_inputs.documents_for_root(&package_root);
    if cancelled() {
        return None;
    }
    let workspace_documents = workspace_documents_from_base(&package_documents, open_documents);
    let mut diagnostics =
        package_frontend_diagnostics_cancellable(&workspace_documents, &mut cancelled)?;
    diagnostics.retain(|diagnostic| diagnostic.span.file == uri.path());
    Some(diagnostics)
}

#[cfg(test)]
fn lsp_diagnostics_for_uri(uri: &Url, open_documents: &HashMap<Url, Document>) -> Vec<Diagnostic> {
    let diagnostics = diagnostics_for_uri(uri, open_documents);
    lsp_diagnostics_from_diagnostics(uri, open_documents, &diagnostics)
}

fn lsp_diagnostics_from_diagnostics(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
    diagnostics: &[RsDiagnostic],
) -> Vec<Diagnostic> {
    let text = open_documents
        .get(uri)
        .map(|document| document.text.as_ref())
        .unwrap_or("");
    diagnostics
        .iter()
        .map(|diagnostic| to_lsp_diagnostic(text, diagnostic))
        .collect()
}

#[cfg(test)]
fn single_file_diagnostics(path: &str, text: &str) -> Vec<RsDiagnostic> {
    let mut diagnostics = analyze_source_with_core(path, text);
    diagnostics.extend(lint_source(path, text));
    diagnostics
}

fn package_frontend_diagnostics_cancellable(
    documents: &[WorkspaceDocument],
    cancelled: &mut impl FnMut() -> bool,
) -> Option<Vec<RsDiagnostic>> {
    if cancelled() {
        return None;
    }
    let interfaces = documents
        .iter()
        .filter(|document| document.kind == Some(PackageReviewFileKind::Interface))
        .map(|document| (document.uri.path(), document.text.as_ref()))
        .collect::<Vec<_>>();
    let sources = documents
        .iter()
        .filter(|document| document.kind == Some(PackageReviewFileKind::Source))
        .map(|document| (document.uri.path(), document.text.as_ref()))
        .collect::<Vec<_>>();

    let mut diagnostics = Vec::new();
    for (path, contents) in &interfaces {
        if cancelled() {
            return None;
        }
        let visible_interfaces = interfaces
            .iter()
            .filter(|(interface_path, _)| interface_path != path)
            .map(|(interface_path, interface_contents)| (*interface_path, *interface_contents))
            .collect::<Vec<_>>();
        diagnostics.extend(analyze_source_with_interfaces(
            path,
            contents,
            &visible_interfaces,
        ));
    }
    if cancelled() {
        return None;
    }
    diagnostics.extend(analyze_sources_with_interfaces(&sources, &interfaces));
    for document in documents {
        if cancelled() {
            return None;
        }
        diagnostics.extend(lint_source(document.uri.path(), &document.text));
    }
    if cancelled() {
        return None;
    }
    dedup_diagnostics(&mut diagnostics);
    Some(diagnostics)
}

fn dedup_diagnostics(diagnostics: &mut Vec<RsDiagnostic>) {
    let mut seen = std::collections::HashSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.code.clone(),
            diagnostic.summary.clone(),
            diagnostic.span.file.clone(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.span.length,
        ))
    });
}

fn workspace_documents_for_uri(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
) -> Vec<WorkspaceDocument> {
    let mut documents = package_documents_for_uri(uri);
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

fn workspace_documents_from_base(
    base: &HashMap<Url, WorkspaceDocument>,
    open_documents: &HashMap<Url, Document>,
) -> Vec<WorkspaceDocument> {
    let mut documents = base.clone();
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

fn workspace_documents(open_documents: &HashMap<Url, Document>) -> Vec<WorkspaceDocument> {
    let mut documents = HashMap::new();
    for uri in open_documents.keys() {
        documents.extend(package_documents_for_uri(uri));
    }
    overlay_open_documents(&mut documents, open_documents);
    documents.into_values().collect()
}

fn overlay_open_documents(
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
            },
        );
    }
}

fn package_documents_for_uri(uri: &Url) -> HashMap<Url, WorkspaceDocument> {
    let Some(package_dir) = package_root_for_uri(uri) else {
        return HashMap::new();
    };
    load_package_documents(&package_dir)
}

fn load_package_documents(package_dir: &Path) -> HashMap<Url, WorkspaceDocument> {
    let Ok(sources) = package_sources_with_dependency_interfaces(package_dir) else {
        return HashMap::new();
    };
    sources
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
                        },
                    )
                })
        })
        .collect()
}

fn infer_document_kind(uri: &Url) -> Option<PackageReviewFileKind> {
    let path = uri.path();
    if path.ends_with(".rssi") {
        Some(PackageReviewFileKind::Interface)
    } else if path.ends_with(".rss") {
        Some(PackageReviewFileKind::Source)
    } else {
        None
    }
}

fn analysis_key_for_uri(uri: &Url) -> AnalysisKey {
    if let Some(package_root) = package_root_for_uri(uri) {
        return AnalysisKey::Package(package_root);
    }
    if uri.to_file_path().is_ok() {
        return AnalysisKey::Workspace;
    }
    AnalysisKey::Uri(uri.clone())
}

fn package_root_for_uri(uri: &Url) -> Option<PathBuf> {
    let path = uri.to_file_path().ok()?;
    find_package_root(&path)
}

fn find_package_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() { path } else { path.parent()? };
    loop {
        if current.join("rsspkg.toml").is_file() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn definition_matches_lookup(definition: &Definition, lookup: &SymbolLookup) -> bool {
    definition.name == lookup.name
        && if lookup.is_type {
            definition.kind == RssSymbolKind::Type
        } else {
            matches!(
                definition.kind,
                RssSymbolKind::Function | RssSymbolKind::Const | RssSymbolKind::Variant
            )
        }
}

fn unresolved_reference_matches_lookup(reference: &Reference, lookup: &SymbolLookup) -> bool {
    reference.definition.is_none()
        && reference.name == lookup.name
        && reference.is_type == lookup.is_type
}

#[allow(deprecated)]
fn to_lsp_symbol_information(
    uri: &Url,
    source: &str,
    definition: &Definition,
) -> SymbolInformation {
    SymbolInformation {
        name: definition.name.clone(),
        kind: to_lsp_symbol_kind(definition.kind),
        tags: None,
        deprecated: None,
        location: Location {
            uri: uri.clone(),
            range: span_to_range(source, &definition.span),
        },
        container_name: None,
    }
}

#[allow(deprecated)]
fn to_lsp_document_symbol(source: &str, symbol: RssDocumentSymbol) -> DocumentSymbol {
    DocumentSymbol {
        name: symbol.name,
        detail: symbol.detail,
        kind: to_lsp_symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range: span_to_range(source, &symbol.span),
        selection_range: span_to_range(source, &symbol.selection_span),
        children: if symbol.children.is_empty() {
            None
        } else {
            Some(
                symbol
                    .children
                    .into_iter()
                    .map(|child| to_lsp_document_symbol(source, child))
                    .collect(),
            )
        },
    }
}

const TOKEN_FUNCTION: u32 = 0;
const TOKEN_TYPE: u32 = 1;
const TOKEN_CONST: u32 = 2;
const TOKEN_PARAM: u32 = 3;
const TOKEN_LOCAL: u32 = 4;
const TOKEN_FIELD: u32 = 5;
const TOKEN_VARIANT: u32 = 6;
const TOKEN_RESOURCE: u32 = 7;
const TOKEN_EFFECT: u32 = 8;
const TOKEN_CAPABILITY: u32 = 9;
const TOKEN_NATIVE: u32 = 10;
const TOKEN_KEYWORD: u32 = 11;

const MOD_DEFINITION: u32 = 1;
const MOD_READONLY: u32 = 1 << 1;
const MOD_ASYNC: u32 = 1 << 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct RawSemanticToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::FUNCTION,
            SemanticTokenType::TYPE,
            SemanticTokenType::new("const"),
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::new("resource"),
            SemanticTokenType::new("effect"),
            SemanticTokenType::new("capability"),
            SemanticTokenType::new("native"),
            SemanticTokenType::KEYWORD,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DEFINITION,
            SemanticTokenModifier::READONLY,
            SemanticTokenModifier::ASYNC,
        ],
    }
}

fn semantic_tokens_for_source(path: &str, source: &str) -> SemanticTokens {
    let index = symbol_index(path, source);
    let mut raw = Vec::new();
    for definition in index.definitions() {
        let span = semantic_definition_span(source, definition);
        push_span_token(
            source,
            &mut raw,
            &span,
            semantic_token_type_for_symbol(definition.kind),
            MOD_DEFINITION | semantic_modifiers_for_symbol(definition.kind),
        );
    }
    for reference in index.references() {
        let token_type = index
            .symbol_at(reference.span.line, reference.span.column)
            .map(|symbol| semantic_token_type_for_symbol(symbol.kind))
            .unwrap_or(if reference.is_type {
                TOKEN_TYPE
            } else {
                TOKEN_LOCAL
            });
        push_span_token(source, &mut raw, &reference.span, token_type, 0);
    }
    push_keyword_tokens(source, &mut raw);
    raw.sort();
    raw.dedup_by(|left, right| {
        left.line == right.line && left.start == right.start && left.length == right.length
    });
    SemanticTokens {
        result_id: None,
        data: encode_semantic_tokens(raw),
    }
}

fn semantic_definition_span(source: &str, definition: &Definition) -> Span {
    let Some(line) = source.lines().nth(definition.span.line.saturating_sub(1)) else {
        return definition.span.clone();
    };
    let start_char = definition.span.column.saturating_sub(1);
    let start_byte = byte_offset_for_char(line, start_char);
    let Some(relative_byte) = line[start_byte..].find(&definition.name) else {
        return definition.span.clone();
    };
    let before_name = &line[..start_byte + relative_byte];
    Span {
        file: definition.span.file.clone(),
        line: definition.span.line,
        column: before_name.chars().count() + 1,
        length: definition.name.chars().count(),
    }
}

fn byte_offset_for_char(value: &str, chars: usize) -> usize {
    value
        .char_indices()
        .nth(chars)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn semantic_token_type_for_symbol(kind: RssSymbolKind) -> u32 {
    match kind {
        RssSymbolKind::Function => TOKEN_FUNCTION,
        RssSymbolKind::Type => TOKEN_TYPE,
        RssSymbolKind::Const => TOKEN_CONST,
        RssSymbolKind::Param => TOKEN_PARAM,
        RssSymbolKind::Local => TOKEN_LOCAL,
        RssSymbolKind::Field => TOKEN_FIELD,
        RssSymbolKind::Variant => TOKEN_VARIANT,
    }
}

fn semantic_modifiers_for_symbol(kind: RssSymbolKind) -> u32 {
    match kind {
        RssSymbolKind::Const => MOD_READONLY,
        _ => 0,
    }
}

fn push_span_token(
    source: &str,
    raw: &mut Vec<RawSemanticToken>,
    span: &Span,
    token_type: u32,
    modifiers: u32,
) {
    let range = span_to_range(source, span);
    if range.start.line != range.end.line || range.end.character <= range.start.character {
        return;
    }
    raw.push(RawSemanticToken {
        line: range.start.line,
        start: range.start.character,
        length: range.end.character - range.start.character,
        token_type,
        modifiers,
    });
}

fn push_keyword_tokens(source: &str, raw: &mut Vec<RawSemanticToken>) {
    for (line_index, line) in source.lines().enumerate() {
        let mut chars = line.char_indices().peekable();
        let mut in_string = false;
        while let Some((byte, character)) = chars.next() {
            if !in_string && character == '/' && chars.peek().is_some_and(|(_, next)| *next == '/')
            {
                break;
            }
            if character == '"' {
                in_string = !in_string;
                continue;
            }
            if in_string || !(character.is_ascii_alphabetic() || character == '_') {
                continue;
            }
            let start_byte = byte;
            let mut end_byte = byte + character.len_utf8();
            while let Some((next_byte, next)) = chars.peek().copied() {
                if !(next.is_ascii_alphanumeric() || next == '_') {
                    break;
                }
                chars.next();
                end_byte = next_byte + next.len_utf8();
            }
            let word = &line[start_byte..end_byte];
            let Some((token_type, modifiers)) = semantic_keyword_token(word) else {
                continue;
            };
            raw.push(RawSemanticToken {
                line: line_index as u32,
                start: utf16_len(&line[..start_byte]),
                length: utf16_len(word),
                token_type,
                modifiers,
            });
        }
    }
}

fn semantic_keyword_token(word: &str) -> Option<(u32, u32)> {
    match word {
        "resource" => Some((TOKEN_RESOURCE, 0)),
        "effects" | "effect" => Some((TOKEN_EFFECT, 0)),
        "read" | "mut" | "take" | "fresh" | "owned" | "noescape" | "with" => {
            Some((TOKEN_CAPABILITY, 0))
        }
        "native" | "unsafe" => Some((TOKEN_NATIVE, 0)),
        "async" => Some((TOKEN_KEYWORD, MOD_ASYNC)),
        "fn" | "struct" | "sum" | "protocol" | "const" | "let" | "return" | "if" | "else"
        | "match" | "while" | "for" | "in" | "as" | "await" | "task_group" | "impl" => {
            Some((TOKEN_KEYWORD, 0))
        }
        _ => None,
    }
}

fn utf16_len(value: &str) -> u32 {
    value
        .chars()
        .map(|character| character.len_utf16() as u32)
        .sum()
}

fn encode_semantic_tokens(raw: Vec<RawSemanticToken>) -> Vec<SemanticToken> {
    let mut encoded = Vec::with_capacity(raw.len());
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;
    for token in raw {
        let delta_line = token.line.saturating_sub(previous_line);
        let delta_start = if delta_line == 0 {
            token.start.saturating_sub(previous_start)
        } else {
            token.start
        };
        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.modifiers,
        });
        previous_line = token.line;
        previous_start = token.start;
    }
    encoded
}

fn to_lsp_symbol_kind(kind: RssSymbolKind) -> SymbolKind {
    match kind {
        RssSymbolKind::Function => SymbolKind::FUNCTION,
        RssSymbolKind::Type => SymbolKind::STRUCT,
        RssSymbolKind::Const => SymbolKind::CONSTANT,
        RssSymbolKind::Param => SymbolKind::VARIABLE,
        RssSymbolKind::Local => SymbolKind::VARIABLE,
        RssSymbolKind::Field => SymbolKind::FIELD,
        RssSymbolKind::Variant => SymbolKind::ENUM_MEMBER,
    }
}

fn symbol_kind_label(kind: RssSymbolKind) -> &'static str {
    match kind {
        RssSymbolKind::Function => "function",
        RssSymbolKind::Type => "type",
        RssSymbolKind::Const => "const",
        RssSymbolKind::Param => "parameter",
        RssSymbolKind::Local => "local",
        RssSymbolKind::Field => "field",
        RssSymbolKind::Variant => "variant",
    }
}

fn to_lsp_diagnostic(source: &str, diagnostic: &RsDiagnostic) -> Diagnostic {
    let range = span_to_range(source, &diagnostic.span);
    let location = Location {
        uri: Url::from_file_path(&diagnostic.span.file).unwrap_or_else(|_| {
            Url::parse("file:///rsscript-diagnostic-source-unavailable")
                .expect("fallback diagnostic URL is valid")
        }),
        range,
    };
    let mut related_information = diagnostic
        .causes
        .iter()
        .map(|cause| DiagnosticRelatedInformation {
            location: location.clone(),
            message: format!("cause: {cause}"),
        })
        .collect::<Vec<_>>();
    related_information.extend(
        diagnostic
            .fixes
            .iter()
            .map(|fix| DiagnosticRelatedInformation {
                location: location.clone(),
                message: format!("fix: {}", fix.title),
            }),
    );
    if let Some(explanation) = explain_diagnostic_code(&diagnostic.code) {
        related_information.push(DiagnosticRelatedInformation {
            location: location.clone(),
            message: format!("{}: {}", explanation.title, explanation.explanation),
        });
    }
    let data = diagnostic_data(diagnostic);

    Diagnostic {
        range,
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diagnostic.code.clone())),
        source: Some("rsscript".to_string()),
        message: if diagnostic.label.is_empty() {
            diagnostic.summary.clone()
        } else {
            format!("{}\n{}", diagnostic.summary, diagnostic.label)
        },
        related_information: if related_information.is_empty() {
            None
        } else {
            Some(related_information)
        },
        data: Some(data),
        ..Diagnostic::default()
    }
}

fn diagnostic_data(diagnostic: &RsDiagnostic) -> serde_json::Value {
    let explanation = explain_diagnostic_code(&diagnostic.code).map(|explanation| {
        json!({
            "code": explanation.code,
            "title": explanation.title,
            "explanation": explanation.explanation,
        })
    });
    json!({
        "schema": "rsscript.lsp.diagnostic.v1",
        "code": diagnostic.code,
        "severity": match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
        "summary": diagnostic.summary,
        "label": diagnostic.label,
        "span": {
            "file": diagnostic.span.file,
            "line": diagnostic.span.line,
            "column": diagnostic.span.column,
            "length": diagnostic.span.length,
        },
        "causes": diagnostic.causes,
        "fixes": diagnostic.fixes.iter().map(|fix| {
            json!({
                "kind": fix.kind,
                "title": fix.title,
                "applicability": fix.applicability,
            })
        }).collect::<Vec<_>>(),
        "explanation": explanation,
    })
}

/// Map a checker [`Span`] (1-based line/column counted in `char`s) to an LSP
/// [`Range`] (0-based line, UTF-16 code units).
fn span_to_range(source: &str, span: &Span) -> Range {
    let line_index = span.line.saturating_sub(1);
    let line_text = source.lines().nth(line_index).unwrap_or("");
    let start_char = span.column.saturating_sub(1);
    let end_char = start_char + span.length;

    let utf16_column = |chars: usize| -> u32 {
        line_text
            .chars()
            .take(chars)
            .map(|character| character.len_utf16())
            .sum::<usize>() as u32
    };

    let line = line_index as u32;
    Range {
        start: Position {
            line,
            character: utf16_column(start_char),
        },
        end: Position {
            line,
            character: utf16_column(end_char),
        },
    }
}

fn full_document_range(text: &str) -> Range {
    let last_line = text.lines().count().saturating_sub(1) as u32;
    let last_column = text
        .lines()
        .last()
        .map(|line| line.chars().map(|c| c.len_utf16()).sum::<usize>() as u32)
        .unwrap_or(0);
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            // Cover a possible trailing newline by extending one line past the
            // last content line when the text ends with '\n'.
            line: if text.ends_with('\n') {
                last_line + 1
            } else {
                last_line
            },
            character: if text.ends_with('\n') { 0 } else { last_column },
        },
    }
}

/// Convert an LSP [`Position`] (0-based line, UTF-16 column) to the checker's
/// 1-based line / 1-based `char` column.
fn char_position(source: &str, position: Position) -> (usize, usize) {
    let line_text = source.lines().nth(position.line as usize).unwrap_or("");
    let mut utf16 = 0u32;
    let mut chars = 0usize;
    for character in line_text.chars() {
        if utf16 >= position.character {
            break;
        }
        utf16 += character.len_utf16() as u32;
        chars += 1;
    }
    (position.line as usize + 1, chars + 1)
}

/// Apply one incremental (or full) content change to `text` in place.
fn apply_change(text: &mut String, change: &TextDocumentContentChangeEvent) {
    match change.range {
        Some(range) => {
            let start = byte_offset(text, range.start);
            let end = byte_offset(text, range.end);
            text.replace_range(start..end, &change.text);
        }
        None => *text = change.text.clone(),
    }
}

/// Byte offset of an LSP [`Position`] in `text` (line is 0-based, column is in
/// UTF-16 code units). Clamps past-the-end positions to the text length.
fn byte_offset(text: &str, position: Position) -> usize {
    let mut line_start = 0usize;
    if position.line > 0 {
        let mut current_line = 0u32;
        let mut found = false;
        for (index, character) in text.char_indices() {
            if character == '\n' {
                current_line += 1;
                if current_line == position.line {
                    line_start = index + 1;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return text.len();
        }
    }

    let mut utf16 = 0u32;
    let mut offset = line_start;
    for character in text[line_start..].chars() {
        if utf16 >= position.character || character == '\n' {
            break;
        }
        utf16 += character.len_utf16() as u32;
        offset += character.len_utf8();
    }
    offset
}

fn position_in_range(position: Position, range: &Range) -> bool {
    let after_start =
        (position.line, position.character) >= (range.start.line, range.start.character);
    let before_end = (position.line, position.character) <= (range.end.line, range.end.character);
    after_start && before_end
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::oneshot;

    fn file_url(name: &str) -> Url {
        Url::parse(&format!("file:///workspace/{name}")).expect("valid file URL")
    }

    fn document(text: &str) -> Document {
        Document {
            text: Arc::from(text),
            diagnostics: Arc::new(Vec::new()),
            revision: 0,
            version: 0,
        }
    }

    #[test]
    fn analysis_jobs_share_immutable_document_snapshots() {
        let uri = file_url("snapshot.rss");
        let mut documents = DocumentStore::new();
        let first = open_document(&mut documents, uri.clone(), "first".to_owned(), 1)
            .expect("document should open");
        let first_text = Arc::clone(
            &first
                .open_documents
                .get(&uri)
                .expect("snapshot contains document")
                .text,
        );

        let replacement = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "second".to_owned(),
        };
        let second = change_document(&mut documents, uri.clone(), 2, &[replacement])
            .expect("new version should produce a job");

        assert_eq!(
            first.open_documents.get(&uri).unwrap().text.as_ref(),
            "first"
        );
        assert_eq!(
            second.open_documents.get(&uri).unwrap().text.as_ref(),
            "second"
        );
        assert!(Arc::ptr_eq(
            &first_text,
            &first.open_documents.get(&uri).unwrap().text
        ));
    }

    #[tokio::test]
    async fn replacing_pending_analysis_aborts_superseded_task() {
        let uri = file_url("debounce.rss");
        let analysis_key = analysis_key_for_uri(&uri);
        let mut pending = HashMap::new();
        let first_cancellation = Arc::new(AnalysisCancellation::default());
        let first = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        replace_pending_analysis(
            &mut pending,
            analysis_key.clone(),
            PendingAnalysis {
                task: first.abort_handle(),
                cancellation: Arc::clone(&first_cancellation),
            },
        );

        let second_cancellation = Arc::new(AnalysisCancellation::default());
        let second = tokio::spawn(async {});
        replace_pending_analysis(
            &mut pending,
            analysis_key,
            PendingAnalysis {
                task: second.abort_handle(),
                cancellation: Arc::clone(&second_cancellation),
            },
        );

        assert!(first_cancellation.is_cancelled());
        assert!(!second_cancellation.is_cancelled());
        assert!(
            first
                .await
                .expect_err("superseded task should abort")
                .is_cancelled()
        );
        second.await.expect("latest task should complete");
    }

    #[tokio::test]
    async fn package_edits_cancel_superseded_jobs_and_keep_latest_generation() {
        let package_dir = unique_temp_dir("rss-lsp-package-generation");
        fs::create_dir_all(package_dir.join("src")).expect("create package src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"generation\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write package manifest");

        let mut documents = DocumentStore::new();
        let mut pending = HashMap::new();
        let mut cancellations = Vec::new();
        let mut tasks = Vec::new();
        let mut first_job_state = None;
        let mut latest_job_state = None;

        for index in 0..32 {
            let uri = Url::from_file_path(package_dir.join("src").join(format!("{index}.rss")))
                .expect("source URL");
            let job = open_document(
                &mut documents,
                uri.clone(),
                format!("fn value_{index}() -> Int {{ return {index} }}\n"),
                1,
            )
            .expect("new package document should schedule analysis");
            if index == 0 {
                first_job_state = Some((
                    uri.clone(),
                    job.revision,
                    job.version,
                    job.generation,
                    job.analysis_key.clone(),
                ));
            }
            latest_job_state = Some((
                uri,
                job.revision,
                job.version,
                job.generation,
                job.analysis_key.clone(),
            ));
            cancellations.push(Arc::clone(&job.cancellation));
            let task = tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
            });
            replace_pending_analysis(
                &mut pending,
                job.analysis_key,
                PendingAnalysis {
                    task: task.abort_handle(),
                    cancellation: job.cancellation,
                },
            );
            tasks.push(task);
        }

        assert_eq!(pending.len(), 1);
        assert!(cancellations[..31].iter().all(|item| item.is_cancelled()));
        assert!(!cancellations[31].is_cancelled());

        let (first_uri, first_revision, first_version, first_generation, analysis_key) =
            first_job_state.expect("first job state");
        let (latest_uri, latest_revision, latest_version, latest_generation, latest_key) =
            latest_job_state.expect("latest job state");
        assert_eq!(analysis_key, latest_key);
        assert!(latest_generation > first_generation);
        let current_generation = documents.generation(&analysis_key);
        assert!(!commit_diagnostics_if_current(
            &mut documents,
            &first_uri,
            first_revision,
            first_version,
            first_generation,
            current_generation,
            Vec::new(),
        ));
        assert!(commit_diagnostics_if_current(
            &mut documents,
            &latest_uri,
            latest_revision,
            latest_version,
            latest_generation,
            current_generation,
            Vec::new(),
        ));

        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[tokio::test]
    async fn blocking_work_is_bounded_under_stress() {
        let permits = Arc::new(Semaphore::new(MAX_BLOCKING_ANALYSES));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for _ in 0..16 {
            let permits = Arc::clone(&permits);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            tasks.push(tokio::spawn(async move {
                run_bounded_blocking(permits, move || {
                    let current = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                    peak.fetch_max(current, AtomicOrdering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    active.fetch_sub(1, AtomicOrdering::SeqCst);
                })
                .await
                .expect("blocking task should finish");
            }));
        }

        for task in tasks {
            task.await.expect("bounded task should finish");
        }
        assert_eq!(peak.load(AtomicOrdering::SeqCst), MAX_BLOCKING_ANALYSES);
    }

    #[tokio::test]
    async fn feature_snapshot_releases_document_lock_during_symbol_scans() {
        let uri = file_url("lock-release.rss");
        let source = (0..512)
            .map(|index| format!("fn value_{index}() -> Int {{ return {index} }}\n"))
            .collect::<String>();
        let mut initial = DocumentStore::new();
        open_document(&mut initial, uri.clone(), source, 1).expect("document should open");
        let documents = Arc::new(tokio::sync::Mutex::new(initial));
        let snapshot = snapshot_documents(&documents).await;
        let scan = tokio::task::spawn_blocking(move || {
            for _ in 0..16 {
                let document = snapshot.get(&uri).expect("snapshot document");
                assert!(
                    !symbol_index(uri.path(), &document.text)
                        .definitions()
                        .is_empty()
                );
            }
        });

        for _ in 0..32 {
            let guard = tokio::time::timeout(Duration::from_millis(100), documents.lock())
                .await
                .expect("feature scan must not retain the document lock");
            drop(guard);
            tokio::task::yield_now().await;
        }
        scan.await.expect("symbol scan should finish");
    }

    #[test]
    fn blocking_analysis_stops_at_cooperative_checkpoint() {
        let documents = (0..8)
            .map(|index| WorkspaceDocument {
                uri: file_url(&format!("cancel-{index}.rss")),
                text: Arc::from("fn broken( -> Unit {}\n"),
                kind: Some(PackageReviewFileKind::Source),
            })
            .collect::<Vec<_>>();
        let mut checkpoints = 0;

        let diagnostics = package_frontend_diagnostics_cancellable(&documents, &mut || {
            checkpoints += 1;
            checkpoints >= 3
        });

        assert!(diagnostics.is_none());
        assert_eq!(checkpoints, 3);
    }

    #[test]
    fn cancelled_snapshot_cannot_replace_existing_diagnostics() {
        let uri = file_url("cancelled-stale.rss");
        let mut current = document("fn current() -> Unit {}\n");
        current.revision = 2;
        current.version = 2;
        let mut documents = HashMap::from([(uri.clone(), current)]);
        let snapshot = HashMap::from([(
            uri.clone(),
            Document {
                text: Arc::from("fn stale( -> Unit {}\n"),
                diagnostics: Arc::new(Vec::new()),
                revision: 1,
                version: 1,
            },
        )]);
        let cancellation = AnalysisCancellation::default();
        cancellation.cancel();

        let result =
            diagnostics_for_uri_cancellable(&uri, &snapshot, &PackageInputCache::default(), || {
                cancellation.is_cancelled()
            });

        assert!(result.is_none());
        assert!(!commit_diagnostics_if_current(
            &mut documents,
            &uri,
            1,
            1,
            1,
            2,
            Vec::new(),
        ));
        let current = documents.get(&uri).expect("current document remains");
        assert_eq!(current.revision, 2);
        assert_eq!(current.version, 2);
    }

    #[test]
    fn stale_analysis_cannot_replace_newer_diagnostics() {
        let uri = file_url("stale.rss");
        let mut documents = HashMap::from([(uri.clone(), document("new source"))]);
        documents
            .get_mut(&uri)
            .expect("document should exist")
            .revision = 2;

        assert!(!commit_diagnostics_if_current(
            &mut documents,
            &uri,
            1,
            0,
            1,
            2,
            Vec::new(),
        ));
        assert_eq!(
            documents
                .get(&uri)
                .expect("document should remain")
                .revision,
            2
        );
    }

    #[test]
    fn analysis_for_stale_version_cannot_replace_diagnostics() {
        let uri = file_url("stale-version.rss");
        let mut documents = HashMap::from([(uri.clone(), document("new source"))]);
        let document = documents.get_mut(&uri).expect("document should exist");
        document.revision = 2;
        document.version = 3;

        assert!(!commit_diagnostics_if_current(
            &mut documents,
            &uri,
            2,
            2,
            2,
            2,
            Vec::new(),
        ));
        assert_eq!(
            documents.get(&uri).expect("document should remain").version,
            3
        );
    }

    #[test]
    fn workspace_generation_prevents_cross_document_stale_publish() {
        let uri = file_url("generation.rss");
        let mut documents = HashMap::from([(uri.clone(), document("unchanged target"))]);

        assert!(!commit_diagnostics_if_current(
            &mut documents,
            &uri,
            0,
            0,
            4,
            5,
            Vec::new(),
        ));
        assert!(
            documents
                .get(&uri)
                .expect("target remains open")
                .diagnostics
                .is_empty()
        );
    }

    #[tokio::test]
    async fn concurrent_incremental_changes_apply_to_the_committed_version() {
        let uri = file_url("concurrent.rss");
        let mut initial = DocumentStore::new();
        open_document(&mut initial, uri.clone(), "a".to_string(), 1)
            .expect("initial document should open");
        let documents = Arc::new(tokio::sync::Mutex::new(initial));
        let (version_two_done, wait_for_version_two) = oneshot::channel();

        let first_documents = Arc::clone(&documents);
        let first_uri = uri.clone();
        let version_two = tokio::spawn(async move {
            let change = TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 1), Position::new(0, 1))),
                range_length: None,
                text: "b".to_string(),
            };
            let job = {
                let mut documents = first_documents.lock().await;
                change_document(&mut documents, first_uri, 2, &[change])
            };
            version_two_done
                .send(())
                .expect("version three should still be waiting");
            job
        });

        let second_documents = Arc::clone(&documents);
        let second_uri = uri.clone();
        let version_three = tokio::spawn(async move {
            wait_for_version_two
                .await
                .expect("version two should complete");
            let change = TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 2), Position::new(0, 2))),
                range_length: None,
                text: "c".to_string(),
            };
            let mut documents = second_documents.lock().await;
            change_document(&mut documents, second_uri, 3, &[change])
        });

        assert!(
            version_two
                .await
                .expect("version two task should finish")
                .is_some()
        );
        assert!(
            version_three
                .await
                .expect("version three task should finish")
                .is_some()
        );

        let documents = documents.lock().await;
        let document = documents.get(&uri).expect("document should remain open");
        assert_eq!(document.text.as_ref(), "abc");
        assert_eq!(document.version, 3);
        assert_eq!(document.revision, 3);
        assert_eq!(documents.next_revision, 4);
    }

    #[tokio::test]
    async fn late_out_of_order_change_is_ignored_without_allocating_revision() {
        let uri = file_url("out-of-order.rss");
        let mut initial = DocumentStore::new();
        open_document(&mut initial, uri.clone(), "initial".to_string(), 1)
            .expect("initial document should open");
        let documents = Arc::new(tokio::sync::Mutex::new(initial));
        let (newer_done, wait_for_newer) = oneshot::channel();

        let newer_documents = Arc::clone(&documents);
        let newer_uri = uri.clone();
        let newer = tokio::spawn(async move {
            let change = TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "newest".to_string(),
            };
            let job = {
                let mut documents = newer_documents.lock().await;
                change_document(&mut documents, newer_uri, 3, &[change])
            };
            newer_done
                .send(())
                .expect("older change should still be waiting");
            job
        });

        let older_documents = Arc::clone(&documents);
        let older_uri = uri.clone();
        let older = tokio::spawn(async move {
            wait_for_newer.await.expect("newer change should complete");
            let change = TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "stale".to_string(),
            };
            let mut documents = older_documents.lock().await;
            change_document(&mut documents, older_uri, 2, &[change])
        });

        assert!(newer.await.expect("newer task should finish").is_some());
        assert!(older.await.expect("older task should finish").is_none());

        let documents = documents.lock().await;
        let document = documents.get(&uri).expect("document should remain open");
        assert_eq!(document.text.as_ref(), "newest");
        assert_eq!(document.version, 3);
        assert_eq!(document.revision, 2);
        assert_eq!(documents.next_revision, 3);
    }

    #[test]
    fn workspace_definition_resolves_unresolved_call_in_open_document() {
        let callee_uri = file_url("callee.rss");
        let caller_uri = file_url("caller.rss");
        let mut documents = HashMap::new();
        documents.insert(
            callee_uri.clone(),
            document("fn helper(value: Int) -> Int {\n    return value\n}\n"),
        );
        documents.insert(
            caller_uri.clone(),
            document("fn run() -> Int {\n    return helper(value: 1)\n}\n"),
        );
        let caller = documents.get(&caller_uri).expect("caller document");
        let index = symbol_index("/workspace/caller.rss", &caller.text);
        let lookup = index.lookup_at(2, 12).expect("helper lookup");
        let workspace = workspace_documents(&documents);

        let location =
            workspace_definition_location(&workspace, &lookup).expect("workspace definition");

        assert_eq!(location.uri, callee_uri);
        assert_eq!(location.range.start.line, 0);
    }

    #[test]
    fn workspace_references_collect_unresolved_cross_file_calls() {
        let callee_uri = file_url("callee.rss");
        let caller_uri = file_url("caller.rss");
        let mut documents = HashMap::new();
        documents.insert(
            callee_uri.clone(),
            document("fn helper(value: Int) -> Int {\n    return value\n}\n"),
        );
        documents.insert(
            caller_uri.clone(),
            document("fn run() -> Int {\n    return helper(value: 1)\n}\n"),
        );
        let caller = documents.get(&caller_uri).expect("caller document");
        let index = symbol_index("/workspace/caller.rss", &caller.text);
        let lookup = index.lookup_at(2, 12).expect("helper lookup");
        let workspace = workspace_documents(&documents);

        let locations = workspace_reference_locations(&workspace, &lookup, true);

        assert!(locations.iter().any(|location| location.uri == callee_uri));
        assert!(locations.iter().any(|location| location.uri == caller_uri));
    }

    #[test]
    fn document_highlight_locations_stay_in_current_document() {
        let uri = file_url("highlight-local.rss");
        let source = concat!(
            "fn run(value: Int) -> Int {\n",
            "    let next = value\n",
            "    return next\n",
            "}\n",
        );
        let mut documents = HashMap::new();
        documents.insert(uri.clone(), document(source));

        let locations = reference_locations_for_position(
            &uri,
            Position {
                line: 1,
                character: 15,
            },
            &documents,
            true,
        );

        assert_eq!(locations.len(), 2);
        assert!(locations.iter().all(|location| location.uri == uri));
        assert!(
            locations
                .iter()
                .any(|location| location.range.start.line == 0)
        );
        assert!(
            locations
                .iter()
                .any(|location| location.range.start.line == 1)
        );
    }

    #[test]
    fn prepare_rename_returns_symbol_range_and_placeholder() {
        let uri = file_url("prepare-rename.rss");
        let source = "fn run(value: Int) -> Int {\n    return value\n}\n";
        let mut documents = HashMap::new();
        documents.insert(uri.clone(), document(source));

        let (range, placeholder) = rename_target(
            &uri,
            Position {
                line: 1,
                character: 12,
            },
            &documents,
        )
        .expect("rename target");

        assert_eq!(placeholder, "value");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 7);
        assert_eq!(range.end.character, 12);
    }

    #[test]
    fn semantic_tokens_mark_review_relevant_language_roles() {
        let source = concat!(
            "native fn read_file(path: read String) -> String effects(file_read) {\n",
            "    return path\n",
            "}\n",
        );

        let tokens = semantic_tokens_for_source("/workspace/review.rss", source);

        assert!(
            tokens
                .data
                .iter()
                .any(|token| token.token_type == TOKEN_FUNCTION
                    && token.token_modifiers_bitset & MOD_DEFINITION != 0)
        );
        assert!(
            tokens
                .data
                .iter()
                .any(|token| token.token_type == TOKEN_NATIVE)
        );
        assert!(
            tokens
                .data
                .iter()
                .any(|token| token.token_type == TOKEN_CAPABILITY)
        );
        assert!(
            tokens
                .data
                .iter()
                .any(|token| token.token_type == TOKEN_EFFECT)
        );
    }

    #[test]
    fn call_hierarchy_reports_incoming_and_outgoing_calls() {
        let uri = file_url("call-hierarchy.rss");
        let source = concat!(
            "fn leaf() -> Int {\n",
            "    return 1\n",
            "}\n",
            "\n",
            "fn caller() -> Int {\n",
            "    return leaf()\n",
            "}\n",
        );
        let workspace = vec![WorkspaceDocument {
            uri: uri.clone(),
            text: Arc::from(source),
            kind: Some(PackageReviewFileKind::Source),
        }];
        let (_, leaf_definition) =
            find_function_definition_with_document(&workspace, "leaf").expect("leaf definition");
        let (_, caller_definition) = find_function_definition_with_document(&workspace, "caller")
            .expect("caller definition");
        let leaf_item = to_call_hierarchy_item(source, &uri, &leaf_definition);
        let caller_item = to_call_hierarchy_item(source, &uri, &caller_definition);

        let incoming = incoming_call_hierarchy(&workspace, &leaf_item);
        let outgoing = outgoing_call_hierarchy(&workspace, &caller_item);

        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].from.name, "caller");
        assert_eq!(incoming[0].from_ranges[0].start.line, 5);
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].to.name, "leaf");
        assert_eq!(outgoing[0].from_ranges[0].start.line, 5);
    }

    #[test]
    fn workspace_definition_loads_package_sources_from_disk() {
        let package_dir = unique_temp_dir("rss-lsp-package-definition");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::write(
            package_dir.join("src/helper.rss"),
            "fn helper(value: Int) -> Int {\n    return value\n}\n",
        )
        .expect("write helper");
        let caller_text = "fn run() -> Int {\n    return helper(value: 1)\n}\n";
        let caller_path = package_dir.join("src/main.rss");
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let helper_uri =
            Url::from_file_path(package_dir.join("src/helper.rss")).expect("helper URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri.clone(), document(caller_text));
        let index = symbol_index(caller_uri.path(), caller_text);
        let lookup = index.lookup_at(2, 12).expect("helper lookup");
        let workspace = workspace_documents_for_uri(&caller_uri, &documents);

        let location =
            workspace_definition_location(&workspace, &lookup).expect("package definition");

        assert_eq!(location.uri, helper_uri);

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn package_input_cache_reuses_and_invalidates_immutable_inputs() {
        let package_dir = unique_temp_dir("rss-lsp-package-input-cache");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"cache\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        let source_path = package_dir.join("src/main.rss");
        fs::write(&source_path, "fn old() -> Unit {}\n").expect("write source");
        let source_uri = Url::from_file_path(&source_path).expect("source URL");
        let cache = PackageInputCache::default();

        let first = cache.documents_for_root(&package_dir);
        fs::write(&source_path, "fn new() -> Unit {}\n").expect("rewrite source");
        let cached = cache.documents_for_root(&package_dir);
        assert!(Arc::ptr_eq(&first, &cached));
        assert_eq!(
            cached
                .get(&source_uri)
                .expect("cached source")
                .text
                .as_ref(),
            "fn old() -> Unit {}\n"
        );

        cache.invalidate(&package_dir);
        let refreshed = cache.documents_for_root(&package_dir);
        assert!(!Arc::ptr_eq(&cached, &refreshed));
        assert_eq!(
            refreshed
                .get(&source_uri)
                .expect("refreshed source")
                .text
                .as_ref(),
            "fn new() -> Unit {}\n"
        );

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn workspace_symbols_include_package_sources_from_disk() {
        let package_dir = unique_temp_dir("rss-lsp-package-symbols");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::write(
            package_dir.join("src/helper.rss"),
            "fn helper(value: Int) -> Int {\n    return value\n}\n",
        )
        .expect("write helper");
        let caller_path = package_dir.join("src/main.rss");
        let caller_text = "fn run() -> Int {\n    return helper(value: 1)\n}\n";
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri, document(caller_text));

        let workspace = workspace_documents(&documents);

        assert!(
            workspace
                .iter()
                .any(|document| document.uri.path().ends_with("helper.rss"))
        );
        assert!(
            workspace
                .iter()
                .any(|document| document.uri.path().ends_with("main.rss"))
        );

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn package_diagnostics_use_interface_sources() {
        let package_dir = unique_temp_dir("rss-lsp-package-diagnostics-interface");
        fs::create_dir_all(package_dir.join("interface")).expect("create interface");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::write(package_dir.join("interface/api.rssi"), "struct Widget\n")
            .expect("write interface");
        let caller_text = "struct Holder {\n    value: Widget\n}\n";
        let caller_path = package_dir.join("src/main.rss");
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri.clone(), document(caller_text));

        let single_file = single_file_diagnostics(caller_uri.path(), caller_text);
        let package = diagnostics_for_uri(&caller_uri, &documents);

        assert!(
            single_file
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unknown type `Widget`"))
        );
        assert!(
            !package
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unknown type `Widget`"))
        );

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn package_diagnostics_use_dependency_interface_sources() {
        let workspace_dir = unique_temp_dir("rss-lsp-package-dependency-interface");
        let dependency_dir = workspace_dir.join("dep");
        let package_dir = workspace_dir.join("app");
        fs::create_dir_all(dependency_dir.join("interface")).expect("create dependency interface");
        fs::create_dir_all(package_dir.join("src")).expect("create package src");
        fs::write(
            dependency_dir.join("rsspkg.toml"),
            concat!(
                "[package]\n",
                "name = \"dep\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "\n",
                "[interfaces]\n",
                "paths = [\"interface\"]\n",
            ),
        )
        .expect("write dependency manifest");
        fs::write(
            dependency_dir.join("interface/api.rssi"),
            "pub fn Dep.helper(value: read Int) -> Int\n",
        )
        .expect("write dependency interface");
        fs::write(
            package_dir.join("rsspkg.toml"),
            concat!(
                "[package]\n",
                "name = \"demo\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "\n",
                "[dependencies]\n",
                "dep = { path = \"../dep\" }\n",
            ),
        )
        .expect("write package manifest");
        let caller_text = "fn run() -> Int {\n    return Dep.helper(value: read 1)\n}\n";
        let caller_path = package_dir.join("src/main.rss");
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri.clone(), document(caller_text));

        let single_file = single_file_diagnostics(caller_uri.path(), caller_text);
        let package = diagnostics_for_uri(&caller_uri, &documents);

        assert!(
            single_file
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("Dep.helper"))
        );
        assert!(
            !package
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("Dep.helper"))
        );

        fs::remove_dir_all(workspace_dir).expect("cleanup workspace");
    }

    #[test]
    fn package_diagnostics_overlay_open_interface_document() {
        let package_dir = unique_temp_dir("rss-lsp-package-diagnostics-overlay");
        fs::create_dir_all(package_dir.join("interface")).expect("create interface");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        let interface_path = package_dir.join("interface/api.rssi");
        fs::write(&interface_path, "struct OldWidget\n").expect("write interface");
        let caller_text = "struct Holder {\n    value: Widget\n}\n";
        let caller_path = package_dir.join("src/main.rss");
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let interface_uri = Url::from_file_path(&interface_path).expect("interface URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri.clone(), document(caller_text));
        documents.insert(interface_uri, document("struct Widget\n"));

        let package = diagnostics_for_uri(&caller_uri, &documents);

        assert!(
            !package
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("unknown type `Widget`"))
        );

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn pull_diagnostics_use_structured_lsp_diagnostic_data() {
        let uri = file_url("pull-diagnostics.rss");
        let source = "fn run() -> Int {\n    return missing\n}\n";
        let mut documents = HashMap::new();
        documents.insert(uri.clone(), document(source));

        let diagnostics = lsp_diagnostics_for_uri(&uri, &documents);

        assert_eq!(diagnostics.len(), 1);
        let data = diagnostics[0].data.as_ref().expect("diagnostic data");
        assert_eq!(data["schema"], "rsscript.lsp.diagnostic.v1");
        assert_eq!(data["code"], "RS0026");
        assert_eq!(data["span"]["file"], uri.path());
    }

    #[test]
    fn hover_symbol_info_uses_package_definition_detail() {
        let package_dir = unique_temp_dir("rss-lsp-package-hover");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::write(
            package_dir.join("src/helper.rss"),
            "fn helper(value: read Int) -> Int effects(pure) {\n    return value\n}\n",
        )
        .expect("write helper");
        let caller_text = "fn run() -> Int {\n    return helper(value: read 1)\n}\n";
        let caller_path = package_dir.join("src/main.rss");
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri.clone(), document(caller_text));
        let index = symbol_index(caller_uri.path(), caller_text);

        let symbol =
            hover_symbol_info(&caller_uri, &documents, &index, 2, 12).expect("helper hover symbol");
        let markdown = symbol_hover_markdown(&symbol);

        assert_eq!(symbol.name, "helper");
        assert!(markdown.contains("fn(value: read Int) -> Int effects(pure)"));

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn call_context_tracks_active_parameter() {
        let source = "fn run() -> Unit {\n    helper(first: read 1, second: read value\n}\n";
        let context = call_context_at(
            source,
            Position {
                line: 1,
                character: 40,
            },
        )
        .expect("call context");

        assert_eq!(context.callee, "helper");
        assert_eq!(context.active_parameter, 1);
    }

    #[test]
    fn signature_help_uses_package_function_detail() {
        let package_dir = unique_temp_dir("rss-lsp-package-signature");
        fs::create_dir_all(package_dir.join("src")).expect("create src");
        fs::write(
            package_dir.join("rsspkg.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::write(
            package_dir.join("src/helper.rss"),
            concat!(
                "fn helper(first: read Int, second: read String) -> Unit effects(pure) {\n",
                "    return\n",
                "}\n",
            ),
        )
        .expect("write helper");
        let caller_path = package_dir.join("src/main.rss");
        let caller_text = "fn run() -> Unit {\n    helper(first: read 1, second: read \"x\")\n}\n";
        fs::write(&caller_path, caller_text).expect("write caller");
        let caller_uri = Url::from_file_path(&caller_path).expect("caller URL");
        let mut documents = HashMap::new();
        documents.insert(caller_uri.clone(), document(caller_text));
        let workspace = workspace_documents_for_uri(&caller_uri, &documents);
        let context = call_context_at(
            caller_text,
            Position {
                line: 1,
                character: 35,
            },
        )
        .expect("call context");
        let definition =
            workspace_function_definition(&workspace, &context.callee).expect("helper definition");

        let signature = signature_information(&definition, context.active_parameter)
            .expect("signature information");

        assert!(
            signature
                .label
                .contains("fn(first: read Int, second: read String)")
        );
        assert_eq!(signature.active_parameter, Some(1));
        let parameters = signature.parameters.expect("parameters");
        assert_eq!(parameters.len(), 2);

        fs::remove_dir_all(package_dir).expect("cleanup package");
    }

    #[test]
    fn rename_local_symbol_stays_in_current_scope() {
        let uri = file_url("rename-local.rss");
        let source = concat!(
            "fn first(value: Int) -> Int {\n",
            "    return value\n",
            "}\n",
            "\n",
            "fn second(value: Int) -> Int {\n",
            "    return value\n",
            "}\n",
        );
        let mut documents = HashMap::new();
        documents.insert(uri.clone(), document(source));

        let edit = rename_workspace_edit(
            &uri,
            Position {
                line: 1,
                character: 12,
            },
            "amount",
            &documents,
        )
        .expect("rename edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("local edits");

        assert_eq!(edits.len(), 2);
        assert!(edits.iter().any(|edit| edit.range.start.line == 0));
        assert!(edits.iter().any(|edit| edit.range.start.line == 1));
        assert!(!edits.iter().any(|edit| edit.range.start.line == 4));
        assert!(!edits.iter().any(|edit| edit.range.start.line == 5));
    }

    #[test]
    fn rename_top_level_symbol_updates_package_references() {
        let callee_uri = file_url("rename-callee.rss");
        let caller_uri = file_url("rename-caller.rss");
        let mut documents = HashMap::new();
        documents.insert(
            callee_uri.clone(),
            document("fn helper(value: Int) -> Int {\n    return value\n}\n"),
        );
        documents.insert(
            caller_uri.clone(),
            document("fn run() -> Int {\n    return helper(value: 1)\n}\n"),
        );

        let edit = rename_workspace_edit(
            &caller_uri,
            Position {
                line: 1,
                character: 12,
            },
            "compute",
            &documents,
        )
        .expect("rename edit");
        let changes = edit.changes.expect("changes");
        let callee_edits = changes.get(&callee_uri).expect("callee edits");
        let caller_edits = changes.get(&caller_uri).expect("caller edits");

        assert_eq!(callee_edits.len(), 1);
        assert_eq!(caller_edits.len(), 1);
        assert_eq!(callee_edits[0].new_text, "compute");
        assert_eq!(caller_edits[0].new_text, "compute");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    #[test]
    fn lsp_diagnostic_carries_related_causes_fixes_and_explanation() {
        let diagnostic = RsDiagnostic::error(
            "RS0026",
            "unknown value binding `missing`.",
            Span {
                file: "/workspace/main.rss".to_string(),
                line: 2,
                column: 12,
                length: 7,
            },
            "unknown binding",
        )
        .with_cause("RSScript values must resolve before Rust lowering.")
        .with_fix(
            "declare_binding",
            "Declare `missing` before using it.",
            "manual",
        );

        let lsp = to_lsp_diagnostic("fn run() -> Unit {\n    return missing\n}\n", &diagnostic);
        let related = lsp
            .related_information
            .expect("related diagnostic information");

        assert!(
            related
                .iter()
                .any(|info| info.message.starts_with("cause:"))
        );
        assert!(related.iter().any(|info| info.message.starts_with("fix:")));
        assert!(
            related
                .iter()
                .any(|info| info.message.contains("unknown binding"))
        );
        let data = lsp.data.expect("structured diagnostic data");
        assert_eq!(data["schema"], "rsscript.lsp.diagnostic.v1");
        assert_eq!(data["code"], "RS0026");
        assert_eq!(data["severity"], "error");
        assert_eq!(
            data["causes"][0],
            "RSScript values must resolve before Rust lowering."
        );
        assert_eq!(data["fixes"][0]["kind"], "declare_binding");
        assert_eq!(data["fixes"][0]["applicability"], "manual");
        assert_eq!(data["explanation"]["code"], "RS0026");
    }
}
