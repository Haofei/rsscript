//! LSP protocol adapter and request orchestration.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use rsscript_language_service::{document_symbols, explain_diagnostic_code, format_source};
use serde_json::json;
use tokio::sync::Semaphore;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics::*;
use crate::documents::*;
use crate::features::*;
use crate::publication::*;
use crate::scheduler::*;
use crate::scope::*;
use crate::text::*;
use crate::workspace::*;

pub(crate) struct Backend {
    pub(crate) client: Client,
    pub(crate) documents: Arc<tokio::sync::Mutex<DocumentStore>>,
    pub(crate) diagnostics_publications: DiagnosticsPublisher,
    pub(crate) pending_analysis: tokio::sync::Mutex<HashMap<AnalysisKey, PendingAnalysis>>,
    pub(crate) package_inputs: Arc<PackageInputCache>,
    pub(crate) blocking_analysis_permits: Arc<Semaphore>,
}

impl Backend {
    pub(crate) fn new(client: Client) -> Self {
        let diagnostics_publications = spawn_diagnostics_publisher(client.clone());
        Self {
            client,
            documents: Arc::new(tokio::sync::Mutex::new(DocumentStore::new())),
            diagnostics_publications,
            pending_analysis: tokio::sync::Mutex::new(HashMap::new()),
            package_inputs: Arc::new(PackageInputCache::default()),
            blocking_analysis_permits: Arc::new(Semaphore::new(MAX_BLOCKING_ANALYSES)),
        }
    }

    pub(crate) async fn cancel_pending_analysis(&self, analysis_key: &AnalysisKey) {
        if let Some(pending) = self.pending_analysis.lock().await.remove(analysis_key) {
            pending.cancellation.cancel();
            pending.task.abort();
        }
    }

    /// Debounce analysis for one package/workspace and cancel any superseded task.
    pub(crate) async fn schedule_analysis(&self, job: AnalysisJob) {
        let analysis_key = job.analysis_key.clone();
        let client = self.client.clone();
        let documents = Arc::clone(&self.documents);
        let diagnostics_publications = self.diagnostics_publications.clone();
        let package_inputs = Arc::clone(&self.package_inputs);
        let blocking_analysis_permits = Arc::clone(&self.blocking_analysis_permits);
        let cancellation = Arc::clone(&job.cancellation);
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Self::analyze_and_publish(
                client,
                documents,
                diagnostics_publications,
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
    pub(crate) async fn analyze_and_publish(
        client: Client,
        documents: Arc<tokio::sync::Mutex<DocumentStore>>,
        diagnostics_publications: DiagnosticsPublisher,
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
            enqueue_diagnostics(
                &diagnostics_publications,
                uri,
                lsp_diagnostics,
                Some(version),
            );
        }
    }
}

pub(crate) async fn snapshot_documents(
    documents: &tokio::sync::Mutex<DocumentStore>,
) -> HashMap<Url, Document> {
    documents
        .lock()
        .await
        .documents
        .iter()
        .filter(|(_, document)| document.sync_state == DocumentSyncState::Synchronized)
        .map(|(uri, document)| (uri.clone(), document.clone()))
        .collect()
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
        if let Err(error) = self
            .client
            .register_capability(vec![Registration {
                id: "rsscript-package-input-watcher".to_owned(),
                method: "workspace/didChangeWatchedFiles".to_owned(),
                register_options: Some(json!({
                    "watchers": [{
                        "globPattern": "**/{rsspkg.toml,rsspkg.lock,*.rss,*.rssi}"
                    }]
                })),
            }])
            .await
        {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("RSScript package file watching unavailable: {error}"),
                )
                .await;
        }
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
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let analysis_key = analysis_key_for_uri(&uri);
        let outcome = {
            let mut documents = self.documents.lock().await;
            let outcome = change_document(
                &mut documents,
                uri.clone(),
                version,
                &params.content_changes,
            );
            if matches!(outcome, ChangeOutcome::Desynchronized(_)) {
                enqueue_diagnostics(
                    &self.diagnostics_publications,
                    uri.clone(),
                    Vec::new(),
                    Some(version),
                );
            }
            outcome
        };
        match outcome {
            ChangeOutcome::Applied(job) => {
                self.cancel_pending_analysis(&analysis_key).await;
                self.schedule_analysis(*job).await;
            }
            ChangeOutcome::IgnoredStale => {}
            ChangeOutcome::Desynchronized(reason) => {
                self.cancel_pending_analysis(&analysis_key).await;
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!(
                            "RSScript document synchronization lost ({reason:?}); \
                             semantic results are suspended until a full-text change"
                        ),
                    )
                    .await;
            }
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let analysis_key = analysis_key_for_uri(&params.text_document.uri);
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
        self.cancel_pending_analysis(&analysis_key).await;
        {
            let mut documents = self.documents.lock().await;
            documents.allocate_revision(&analysis_key);
            documents.remove(&params.text_document.uri);
            enqueue_diagnostics(
                &self.diagnostics_publications,
                params.text_document.uri,
                Vec::new(),
                None,
            );
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut affected_roots = HashSet::new();
        for change in params.changes {
            let Ok(path) = change.uri.to_file_path() else {
                continue;
            };
            affected_roots.extend(self.package_inputs.invalidate_path(&path));
            if let Some(package_root) = package_root_for_path(&path) {
                affected_roots.insert(package_root);
            }
        }

        if affected_roots.is_empty() {
            return;
        }

        let jobs = {
            let mut documents = self.documents.lock().await;
            let uris = documents
                .iter()
                .filter(|(uri, document)| {
                    document.sync_state == DocumentSyncState::Synchronized
                        && package_root_for_uri(uri)
                            .is_some_and(|root| affected_roots.contains(&root))
                })
                .map(|(uri, _)| uri.clone())
                .collect::<Vec<_>>();
            uris.into_iter()
                .filter_map(|uri| {
                    let document = documents.get(&uri)?.clone();
                    let analysis_key = analysis_key_for_uri(&uri);
                    let revision = documents.allocate_revision(&analysis_key);
                    let current = documents.get_mut(&uri)?;
                    current.revision = revision;
                    current.diagnostics = Arc::new(Vec::new());
                    Some(analysis_job(&documents, uri, revision, document.version))
                })
                .collect::<Vec<_>>()
        };

        for job in jobs {
            self.cancel_pending_analysis(&job.analysis_key).await;
            self.schedule_analysis(job).await;
        }
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
        let index = document.symbol_index(uri.path());
        let Some(symbol) =
            hover_symbol_info(&uri, &documents, &index, line, column, &self.package_inputs)
        else {
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
        let index = document.symbol_index(uri.path());
        if let Some(span) = index.definition_at(line, column) {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, span),
            })));
        }

        let Some(lookup) = index.lookup_at(line, column) else {
            return Ok(None);
        };
        let workspace_documents =
            workspace_documents_for_uri(&uri, &documents, &self.package_inputs);
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
            &self.package_inputs,
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

        let locations = reference_locations_for_position(
            &uri,
            position,
            &documents,
            true,
            &self.package_inputs,
        );
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
            let index = document.symbol_index(uri.path());
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
            semantic_tokens_for_index(&document.text, &document.symbol_index(uri.path())),
        )))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let documents = snapshot_documents(&self.documents).await;
        let workspace_documents =
            workspace_documents_for_uri(&uri, &documents, &self.package_inputs);
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
        let workspace_documents =
            workspace_documents_for_uri(&params.item.uri, &documents, &self.package_inputs);
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
        let workspace_documents =
            workspace_documents_for_uri(&params.item.uri, &documents, &self.package_inputs);
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
        for document in workspace_documents(&documents, &self.package_inputs) {
            let index = document.symbol_index();
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
        let workspace_documents =
            workspace_documents_for_uri(&uri, &documents, &self.package_inputs);
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
            &self.package_inputs,
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
