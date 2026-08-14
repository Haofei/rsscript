#![forbid(unsafe_code)]

//! Revisioned, cached editor-facing RSScript language service.
//!
//! Its API is deliberately document-oriented so editor clients do not couple
//! themselves to analyzer databases, runtime values, VM registers, or optional
//! backends.
//! Parsing-adjacent queries are cached independently. Semantic diagnostics are
//! cached only by the shared [`CompilationSession`], so editor clients never
//! maintain a competing interface-dependency cache.

use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationContext};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use rsscript_diagnostics::{
    Diagnostic, DiagnosticExplanation, Severity, Span, explain_diagnostic_code,
};
pub use rsscript_semantics::{
    CompilationSession, Definition, FrontendInputSnapshot, Reference, RssDocumentSymbol,
    SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup, document_symbols,
    document_symbols_from_program, symbol_index, symbol_index_from_program,
};
pub use rsscript_syntax::{format_source, lint_source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Source,
    Interface,
}

#[derive(Debug, Clone)]
struct Document {
    revision: u64,
    kind: DocumentKind,
}

#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    pub path: String,
    pub revision: u64,
    pub kind: DocumentKind,
    pub text: Arc<str>,
}

pub struct LanguageService {
    documents: BTreeMap<String, Document>,
    frontend: CompilationSession,
    lint_cache: BTreeMap<(String, u64), Arc<[Diagnostic]>>,
    format_cache: BTreeMap<(String, u64), Arc<str>>,
    symbol_cache: BTreeMap<(String, u64), Arc<SymbolIndex>>,
    document_symbol_cache: BTreeMap<(String, u64), Arc<[RssDocumentSymbol]>>,
    query_stats: BTreeMap<QueryKind, QueryStats>,
    cache_hits: u64,
    cache_misses: u64,
    invalidations: u64,
}

impl Default for LanguageService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryKind {
    Dependencies,
    Diagnostics,
    Lint,
    Format,
    Symbols,
    DocumentSymbols,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryStats {
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct LanguageRequest<'a> {
    pub cancellation: Option<&'a CancellationToken>,
    pub deadline: Option<MonotonicDeadline>,
    pub max_diagnostics: usize,
}

impl Default for LanguageRequest<'static> {
    fn default() -> Self {
        Self {
            cancellation: None,
            deadline: None,
            max_diagnostics: 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LanguageServiceStats {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub invalidations: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageServiceError {
    Cancelled,
    DeadlineExceeded,
}

impl LanguageServiceError {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
        }
    }
}

impl From<rsscript_operation::OperationAbort> for LanguageServiceError {
    fn from(abort: rsscript_operation::OperationAbort) -> Self {
        match abort {
            rsscript_operation::OperationAbort::Cancelled => Self::Cancelled,
            rsscript_operation::OperationAbort::DeadlineExceeded => Self::DeadlineExceeded,
        }
    }
}

impl LanguageService {
    /// Create a language service over the semantic-owned workspace query.
    /// Editor clients retain only document overlays and LSP protocol
    /// adaptation; all multi-file inputs and diagnostics remain session-owned.
    pub fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
            frontend: CompilationSession::default(),
            lint_cache: BTreeMap::new(),
            format_cache: BTreeMap::new(),
            symbol_cache: BTreeMap::new(),
            document_symbol_cache: BTreeMap::new(),
            query_stats: BTreeMap::new(),
            cache_hits: 0,
            cache_misses: 0,
            invalidations: 0,
        }
    }

    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        revision: u64,
        kind: DocumentKind,
        text: impl Into<Arc<str>>,
    ) {
        let path = path.into();
        if self
            .documents
            .get(&path)
            .is_some_and(|document| revision <= document.revision)
        {
            return;
        }
        let previous = self.documents.get(&path).cloned();
        let text = text.into();
        self.documents
            .insert(path.clone(), Document { revision, kind });
        if !path.is_empty() {
            if let Some(document) = &previous
                && document.kind != kind
            {
                match document.kind {
                    DocumentKind::Source => {
                        self.frontend.remove_file(&path);
                    }
                    DocumentKind::Interface => {
                        self.frontend.remove_interface(&path);
                    }
                }
            }
            let _ = match kind {
                DocumentKind::Source => self.frontend.set_file(path.clone(), text.as_ref()),
                DocumentKind::Interface => self.frontend.set_interface(path.clone(), text.as_ref()),
            };
        }
        self.invalidate_document_queries(&path);
    }

    pub fn remove_file(&mut self, path: &str) -> bool {
        let removed = self.documents.remove(path);
        if let Some(document) = &removed {
            match document.kind {
                DocumentKind::Source => {
                    self.frontend.remove_file(path);
                }
                DocumentKind::Interface => {
                    self.frontend.remove_interface(path);
                }
            }
        }
        self.invalidate_document_queries(path);
        removed.is_some()
    }

    fn invalidate_document_queries(&mut self, path: &str) {
        let mut removed = 0u64;
        removed += retain_other_paths(&mut self.lint_cache, path);
        removed += retain_other_paths(&mut self.format_cache, path);
        removed += retain_other_paths(&mut self.symbol_cache, path);
        removed += retain_other_paths(&mut self.document_symbol_cache, path);
        self.invalidations = self.invalidations.saturating_add(removed);
    }

    pub fn snapshot(&self, path: &str) -> Option<DocumentSnapshot> {
        let document = self.documents.get(path)?;
        self.session_document_snapshot(path, document)
    }

    pub fn diagnostics(&mut self, path: &str) -> Vec<Diagnostic> {
        self.diagnostics_with(path, LanguageRequest::default())
            .unwrap_or_default()
    }

    /// Diagnose every source and interface currently held by this revisioned
    /// workspace. Editor adapters supply VFS and unsaved overlays through
    /// [`set_file`](Self::set_file); they must not reconstruct a second module
    /// graph or call compiler analysis entry points themselves.
    pub fn workspace_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.workspace_diagnostics_with(LanguageRequest::default())
            .unwrap_or_default()
    }

    pub fn workspace_diagnostics_with(
        &mut self,
        request: LanguageRequest<'_>,
    ) -> Result<Vec<Diagnostic>, LanguageServiceError> {
        check_request(request)?;
        let operation = OperationContext {
            cancellation: request.cancellation.cloned(),
            deadline: request.deadline,
            ..OperationContext::default()
        };
        let semantic_diagnostics = self.semantic_workspace_diagnostics(&operation, true)?;
        let mut diagnostics = semantic_diagnostics.to_vec();
        for path in self.documents.keys().cloned().collect::<Vec<_>>() {
            check_request(request)?;
            diagnostics.extend(
                self.lint_with_operation(&path, &operation)
                    .map_err(LanguageServiceError::from)?,
            );
        }
        deduplicate_diagnostics(&mut diagnostics);
        check_request(request)?;
        diagnostics.truncate(request.max_diagnostics);
        Ok(diagnostics)
    }

    pub fn diagnostics_with(
        &mut self,
        path: &str,
        request: LanguageRequest<'_>,
    ) -> Result<Vec<Diagnostic>, LanguageServiceError> {
        check_request(request)?;
        if !self.documents.contains_key(path) {
            return Ok(Vec::new());
        }
        let operation = OperationContext {
            cancellation: request.cancellation.cloned(),
            deadline: request.deadline,
            ..OperationContext::default()
        };
        let mut diagnostics = self
            .semantic_workspace_diagnostics(&operation, true)?
            .iter()
            .filter(|diagnostic| diagnostic.span.file == path)
            .cloned()
            .collect::<Vec<_>>();
        diagnostics.extend(
            self.lint_with_operation(path, &operation)
                .map_err(LanguageServiceError::from)?,
        );
        operation.check().map_err(LanguageServiceError::from)?;
        diagnostics.truncate(request.max_diagnostics);
        Ok(diagnostics)
    }

    /// Query semantic diagnostics once for the immutable session input. Both
    /// whole-workspace and single-document requests consume this same result;
    /// document queries only filter by span and add their local lint facts.
    fn semantic_workspace_diagnostics(
        &mut self,
        operation: &OperationContext,
        record_query_stats: bool,
    ) -> Result<Arc<[Diagnostic]>, LanguageServiceError> {
        let before = self.frontend.stats();
        let diagnostics = self
            .frontend
            .semantic_workspace_diagnostics_with_operation(operation)
            .map_err(LanguageServiceError::from)?;
        let after = self.frontend.stats();
        if record_query_stats {
            if after.workspace_diagnostic_cache_hits > before.workspace_diagnostic_cache_hits {
                self.cache_hits += 1;
                self.record_hit(QueryKind::Diagnostics);
            } else {
                self.cache_misses += 1;
                self.record_miss(QueryKind::Diagnostics);
            }
        }
        Ok(diagnostics)
    }

    pub fn format(&mut self, path: &str) -> Option<String> {
        let document = self.documents.get(path)?.clone();
        let snapshot = self.session_document_snapshot(path, &document)?;
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.format_cache.get(&key) {
            let value = cached.to_string();
            self.record_hit(QueryKind::Format);
            return Some(value);
        }
        let value: Arc<str> = format_source(path, &snapshot.text).into();
        self.format_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::Format);
        Some(value.to_string())
    }

    pub fn symbols(&mut self, path: &str) -> Option<SymbolIndex> {
        let document = self.documents.get(path)?.clone();
        let snapshot = self.session_document_snapshot(path, &document)?;
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.symbol_cache.get(&key) {
            let value = cached.as_ref().clone();
            self.record_hit(QueryKind::Symbols);
            return Some(value);
        }
        let program = match document.kind {
            DocumentKind::Source => self.frontend.parse_file(path),
            DocumentKind::Interface => self.frontend.parse_interface(path),
        }?;
        let value = Arc::new(symbol_index_from_program(&snapshot.text, &program));
        self.symbol_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::Symbols);
        Some(value.as_ref().clone())
    }

    pub fn document_symbols(&mut self, path: &str) -> Vec<RssDocumentSymbol> {
        let Some(document) = self.documents.get(path).cloned() else {
            return Vec::new();
        };
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.document_symbol_cache.get(&key) {
            let value = cached.to_vec();
            self.record_hit(QueryKind::DocumentSymbols);
            return value;
        }
        let program = match document.kind {
            DocumentKind::Source => self.frontend.parse_file(path),
            DocumentKind::Interface => self.frontend.parse_interface(path),
        };
        let value: Arc<[RssDocumentSymbol]> = program
            .map(|program| document_symbols_from_program(&program).into())
            .unwrap_or_else(|| Arc::from([]));
        self.document_symbol_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::DocumentSymbols);
        value.to_vec()
    }

    fn lint_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Vec<Diagnostic>, rsscript_operation::OperationAbort> {
        operation.check()?;
        let Some(document) = self.documents.get(path).cloned() else {
            return Ok(Vec::new());
        };
        let Some(snapshot) = self.session_document_snapshot(path, &document) else {
            return Ok(Vec::new());
        };
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.lint_cache.get(&key) {
            let value = cached.to_vec();
            self.record_hit(QueryKind::Lint);
            operation.check()?;
            return Ok(value);
        }
        let value: Arc<[Diagnostic]> = lint_source(path, &snapshot.text).into();
        self.lint_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::Lint);
        operation.check()?;
        Ok(value.to_vec())
    }

    #[cfg(test)]
    fn dependencies_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Arc<[String]>, rsscript_operation::OperationAbort> {
        operation.check()?;
        let Some(document) = self.documents.get(path).cloned() else {
            return Ok(Arc::from([]));
        };
        let before = self.frontend.stats();
        let graph = self
            .frontend
            .workspace_module_graph_with_operation(operation)?;
        let after = self.frontend.stats();
        let imports = match document.kind {
            DocumentKind::Source => graph.source(path),
            DocumentKind::Interface => graph.interface(path),
        }
        .map(|node| node.imports().to_vec().into())
        .unwrap_or_else(|| Arc::from([]));
        if after.workspace_module_graph_cache_hits > before.workspace_module_graph_cache_hits {
            self.record_hit(QueryKind::Dependencies);
        } else {
            self.record_miss(QueryKind::Dependencies);
        }
        operation.check()?;
        Ok(imports)
    }

    fn record_hit(&mut self, query: QueryKind) {
        self.query_stats.entry(query).or_default().hits += 1;
    }

    fn record_miss(&mut self, query: QueryKind) {
        self.query_stats.entry(query).or_default().misses += 1;
    }

    pub fn query_stats(&self, query: QueryKind) -> QueryStats {
        self.query_stats.get(&query).copied().unwrap_or_default()
    }

    pub fn stats(&self) -> LanguageServiceStats {
        LanguageServiceStats {
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            invalidations: self.invalidations,
        }
    }

    fn session_document_snapshot(
        &self,
        path: &str,
        document: &Document,
    ) -> Option<DocumentSnapshot> {
        let file = match document.kind {
            DocumentKind::Source => self.frontend.source_file_snapshot(path),
            DocumentKind::Interface => self.frontend.interface_file_snapshot(path),
        }?;
        Some(DocumentSnapshot {
            path: path.to_string(),
            revision: document.revision,
            kind: document.kind,
            text: file.text_arc(),
        })
    }
}

fn check_request(request: LanguageRequest<'_>) -> Result<(), LanguageServiceError> {
    if request
        .cancellation
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(LanguageServiceError::Cancelled);
    }
    if request.deadline.is_some_and(MonotonicDeadline::is_expired) {
        return Err(LanguageServiceError::DeadlineExceeded);
    }
    Ok(())
}

fn deduplicate_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
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

fn retain_other_paths<V>(cache: &mut BTreeMap<(String, u64), V>, path: &str) -> u64 {
    let before = cache.len();
    cache.retain(|(cached_path, _), _| cached_path != path);
    u64::try_from(before.saturating_sub(cache.len())).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn service() -> LanguageService {
        LanguageService::new()
    }

    #[test]
    fn revisions_replace_snapshots_and_removal_is_explicit() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit {}\n",
        );
        service.set_file(
            "main.rss",
            2,
            DocumentKind::Source,
            "fn main() -> Int { 1 }\n",
        );
        assert_eq!(service.snapshot("main.rss").unwrap().revision, 2);
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit {}\n",
        );
        assert_eq!(service.snapshot("main.rss").unwrap().revision, 2);
        assert!(service.remove_file("main.rss"));
        assert!(service.snapshot("main.rss").is_none());
    }

    #[test]
    fn service_reuses_diagnostics_formatting_and_symbols() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main()->Int{return 1}\n",
        );
        let diagnostics = service.diagnostics("main.rss");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "unexpected errors: {diagnostics:?}"
        );
        assert_eq!(
            service.format("main.rss").unwrap(),
            "fn main() -> Int {\n    return 1\n}\n"
        );
        assert_eq!(service.document_symbols("main.rss")[0].name, "main");
    }

    #[test]
    fn diagnostics_cache_is_revisioned_and_interfaces_invalidate_dependents() {
        let mut service = service();
        service.set_file(
            "host.rssi",
            1,
            DocumentKind::Interface,
            "module host\npub fn value() -> Int\n",
        );
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.*\nfn main() -> Int { return value() }\n",
        );
        service.diagnostics("main.rss");
        service.diagnostics("main.rss");
        assert_eq!(service.stats().cache_misses, 1);
        assert_eq!(service.stats().cache_hits, 1);

        service.set_file(
            "host.rssi",
            2,
            DocumentKind::Interface,
            "module host\npub fn value() -> String\n",
        );
        service.diagnostics("main.rss");
        assert_eq!(service.stats().cache_misses, 2);
    }

    #[test]
    fn workspace_diagnostics_own_the_multi_document_analysis_query() {
        let mut service = service();
        service.set_file(
            "host.rssi",
            1,
            DocumentKind::Interface,
            "module host\npub fn value() -> Int\n",
        );
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.*\nfn main() -> Int { return value() }\n",
        );

        let diagnostics = service.workspace_diagnostics();
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "unexpected workspace diagnostics: {diagnostics:?}"
        );
        service.workspace_diagnostics();
        assert_eq!(service.query_stats(QueryKind::Diagnostics).misses, 1);
        assert_eq!(service.query_stats(QueryKind::Diagnostics).hits, 1);

        service.set_file(
            "host.rssi",
            2,
            DocumentKind::Interface,
            "module host\npub fn replacement() -> Int\n",
        );
        let diagnostics = service.workspace_diagnostics();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
    }

    #[test]
    fn unrelated_interface_edit_recomputes_session_semantic_diagnostics() {
        let mut service = service();
        service.set_file(
            "host.rssi",
            1,
            DocumentKind::Interface,
            "module host\npub fn value() -> Int\n",
        );
        service.set_file(
            "other.rssi",
            1,
            DocumentKind::Interface,
            "module other\npub fn ignored() -> Int\n",
        );
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.*\nfn main() -> Int { return value() }\n",
        );
        service.diagnostics("main.rss");
        service.set_file(
            "other.rssi",
            2,
            DocumentKind::Interface,
            "module other\npub fn ignored() -> String\n",
        );
        service.diagnostics("main.rss");
        assert_eq!(service.query_stats(QueryKind::Diagnostics).misses, 2);
        assert_eq!(service.query_stats(QueryKind::Diagnostics).hits, 0);
    }

    #[test]
    fn interface_edit_invalidates_semantics_but_reuses_local_queries() {
        let mut service = service();
        service.set_file(
            "host.rssi",
            1,
            DocumentKind::Interface,
            "module host\npub fn value() -> Int\n",
        );
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.*\nfn main()->Int{return value()}\n",
        );
        service.diagnostics("main.rss");
        service.format("main.rss");
        service.symbols("main.rss");
        service.document_symbols("main.rss");

        service.set_file(
            "host.rssi",
            2,
            DocumentKind::Interface,
            "module host\npub fn value() -> String\n",
        );
        service.diagnostics("main.rss");
        service.format("main.rss");
        service.symbols("main.rss");
        service.document_symbols("main.rss");

        assert_eq!(service.query_stats(QueryKind::Diagnostics).misses, 2);
        assert_eq!(service.query_stats(QueryKind::Lint).misses, 1);
        assert_eq!(service.query_stats(QueryKind::Lint).hits, 1);
        for query in [
            QueryKind::Format,
            QueryKind::Symbols,
            QueryKind::DocumentSymbols,
        ] {
            assert_eq!(
                service.query_stats(query),
                QueryStats { hits: 1, misses: 1 }
            );
        }
        // Semantic diagnostics are owned by the shared session cache. The
        // service keeps its document-local formatting, symbol, and lint
        // caches because an interface edit cannot change their local facts.
        assert_eq!(service.stats().invalidations, 0);
    }

    #[test]
    fn transitive_interface_dependency_invalidates_importing_source() {
        let mut service = service();
        service.set_file(
            "base.rssi",
            1,
            DocumentKind::Interface,
            "module base\npub fn value() -> Int\n",
        );
        service.set_file(
            "host.rssi",
            1,
            DocumentKind::Interface,
            "module host\nuse base.*\npub fn forwarded() -> Int\n",
        );
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.*\nfn main() -> Int { return forwarded() }\n",
        );
        service.diagnostics("main.rss");
        service.set_file(
            "base.rssi",
            2,
            DocumentKind::Interface,
            "module base\npub fn value() -> String\n",
        );
        service.diagnostics("main.rss");
        assert_eq!(service.query_stats(QueryKind::Diagnostics).misses, 2);
    }

    #[test]
    fn dependency_graph_comes_from_parsed_items_not_text_lines() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            r#"
            // use ignored.*
            const note = "use also_ignored.*"
            use host.api as host
            fn main() -> Unit {}
        "#,
        );
        assert_eq!(
            service
                .dependencies_with_operation("main.rss", &OperationContext::default())
                .unwrap()
                .as_ref(),
            ["host.api"]
        );
    }

    #[test]
    fn dependencies_consume_the_compilation_session_workspace_graph_query() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "module app\nuse host.api as host\nfn main() -> Unit {}\n",
        );

        assert_eq!(
            service
                .dependencies_with_operation("main.rss", &OperationContext::default())
                .unwrap()
                .as_ref(),
            ["host.api"]
        );
        service.invalidate_document_queries("main.rss");
        assert_eq!(
            service
                .dependencies_with_operation("main.rss", &OperationContext::default())
                .unwrap()
                .as_ref(),
            ["host.api"]
        );
        let stats = service.frontend.stats();
        assert_eq!(stats.module_header_cache_misses, 1);
        assert_eq!(stats.module_header_cache_hits, 0);
        assert_eq!(stats.workspace_module_graph_cache_misses, 1);
        assert_eq!(stats.workspace_module_graph_cache_hits, 1);
    }

    #[test]
    fn editor_symbols_reuse_the_session_parse_cache() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit { return Unit }\n",
        );

        assert!(service.symbols("main.rss").is_some());
        assert_eq!(service.document_symbols("main.rss").len(), 1);
        let stats = service.frontend.stats();
        assert_eq!(stats.parse_cache_misses, 1);
        assert_eq!(stats.parse_cache_hits, 1);
    }

    #[test]
    fn cancelled_and_expired_requests_do_not_enter_analysis() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit { return Unit }\n",
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            service.diagnostics_with(
                "main.rss",
                LanguageRequest {
                    cancellation: Some(&cancelled),
                    ..LanguageRequest::default()
                },
            ),
            Err(LanguageServiceError::Cancelled)
        );
        assert_eq!(
            service.diagnostics_with(
                "main.rss",
                LanguageRequest {
                    deadline: Some(MonotonicDeadline::at(
                        std::time::Instant::now() - Duration::from_millis(1),
                    )),
                    ..LanguageRequest::default()
                },
            ),
            Err(LanguageServiceError::DeadlineExceeded)
        );
        assert_eq!(LanguageServiceError::Cancelled.as_str(), "cancelled");
        assert_eq!(service.stats(), LanguageServiceStats::default());
    }

    #[test]
    fn response_budget_does_not_truncate_the_revision_cache() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn first() -> Missing { return absent }\nfn second() -> Missing { return absent }\n",
        );
        let limited = service
            .diagnostics_with(
                "main.rss",
                LanguageRequest {
                    max_diagnostics: 1,
                    ..LanguageRequest::default()
                },
            )
            .unwrap();
        assert_eq!(limited.len(), 1);

        let complete = service
            .diagnostics_with(
                "main.rss",
                LanguageRequest {
                    max_diagnostics: 100,
                    ..LanguageRequest::default()
                },
            )
            .unwrap();
        assert!(complete.len() > limited.len());
        assert_eq!(service.stats().cache_hits, 1);
    }

    #[test]
    fn cached_document_diagnostics_obey_cancellation_and_deadline() {
        let mut service = service();
        service.set_file(
            "main.rss",
            1,
            DocumentKind::Source,
            "fn main() -> Unit { return Unit }\n",
        );
        service
            .diagnostics_with("main.rss", LanguageRequest::default())
            .expect("warm diagnostic cache");

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            service.diagnostics_with(
                "main.rss",
                LanguageRequest {
                    cancellation: Some(&cancelled),
                    ..LanguageRequest::default()
                },
            ),
            Err(LanguageServiceError::Cancelled)
        );
        assert_eq!(
            service.diagnostics_with(
                "main.rss",
                LanguageRequest {
                    deadline: Some(MonotonicDeadline::at(
                        std::time::Instant::now() - Duration::from_millis(1),
                    )),
                    ..LanguageRequest::default()
                },
            ),
            Err(LanguageServiceError::DeadlineExceeded)
        );
        assert_eq!(service.stats().cache_hits, 0);
    }
}
