#![forbid(unsafe_code)]

//! Revisioned, cached editor-facing RSScript language service.
//!
//! This crate is the only compiler-facing dependency of the LSP. Its API is
//! deliberately document-oriented so editor clients do not couple themselves to
//! analyzer databases, runtime values, VM registers, or optional backends.
//! Parsing-adjacent queries are cached independently, while semantic diagnostics
//! retain an explicit dependency edge to imported interface modules. Editing an
//! unrelated interface therefore does not invalidate formatting, symbols, lint,
//! or diagnostics for unaffected documents.

use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationContext};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use rsscript_compiler::{
    analyze_source_result_with_operation, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_with_interfaces_result_with_operation, analyze_sources_with_interfaces,
};
pub use rsscript_diagnostics::{
    Diagnostic, DiagnosticExplanation, Severity, Span, explain_diagnostic_code,
};
pub use rsscript_semantics::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
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
    text: Arc<str>,
}

#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    pub path: String,
    pub revision: u64,
    pub kind: DocumentKind,
    pub text: Arc<str>,
}

#[derive(Default)]
pub struct LanguageService {
    documents: BTreeMap<String, Document>,
    diagnostic_cache: BTreeMap<(String, u64), Arc<[Diagnostic]>>,
    lint_cache: BTreeMap<(String, u64), Arc<[Diagnostic]>>,
    format_cache: BTreeMap<(String, u64), Arc<str>>,
    symbol_cache: BTreeMap<(String, u64), Arc<SymbolIndex>>,
    document_symbol_cache: BTreeMap<(String, u64), Arc<[RssDocumentSymbol]>>,
    dependency_cache: BTreeMap<(String, u64), Arc<[String]>>,
    query_stats: BTreeMap<QueryKind, QueryStats>,
    cache_hits: u64,
    cache_misses: u64,
    invalidations: u64,
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

impl LanguageService {
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
        let mut changed_modules = BTreeSet::new();
        if let Some(document) = &previous
            && document.kind == DocumentKind::Interface
        {
            changed_modules.extend(interface_modules(&path, &document.text));
        }
        if kind == DocumentKind::Interface {
            changed_modules.extend(interface_modules(&path, &text));
        }
        self.documents.insert(
            path.clone(),
            Document {
                revision,
                kind,
                text,
            },
        );
        self.invalidate_document_queries(&path);
        if previous
            .as_ref()
            .is_some_and(|document| document.kind == DocumentKind::Interface)
            || kind == DocumentKind::Interface
        {
            self.invalidate_interface_dependents(&changed_modules, &path);
        }
    }

    pub fn remove_file(&mut self, path: &str) -> bool {
        let removed = self.documents.remove(path);
        self.invalidate_document_queries(path);
        if let Some(document) = &removed
            && document.kind == DocumentKind::Interface
        {
            let modules = interface_modules(path, &document.text);
            self.invalidate_interface_dependents(&modules, path);
        }
        removed.is_some()
    }

    fn invalidate_document_queries(&mut self, path: &str) {
        let mut removed = 0u64;
        removed += retain_other_paths(&mut self.diagnostic_cache, path);
        removed += retain_other_paths(&mut self.lint_cache, path);
        removed += retain_other_paths(&mut self.format_cache, path);
        removed += retain_other_paths(&mut self.symbol_cache, path);
        removed += retain_other_paths(&mut self.document_symbol_cache, path);
        removed += retain_other_paths(&mut self.dependency_cache, path);
        self.invalidations = self.invalidations.saturating_add(removed);
    }

    fn invalidate_interface_dependents(&mut self, modules: &BTreeSet<String>, changed_path: &str) {
        let mut affected_modules = modules.clone();
        loop {
            let mut changed = false;
            for (path, document) in &self.documents {
                if path == changed_path || document.kind != DocumentKind::Interface {
                    continue;
                }
                let dependencies = document_dependencies(path, &document.text);
                if dependencies.iter().any(|dependency| {
                    affected_modules
                        .iter()
                        .any(|module| dependency_matches_module(dependency, module))
                }) {
                    for module in interface_modules(path, &document.text) {
                        changed |= affected_modules.insert(module);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let dependents = self
            .documents
            .iter()
            .filter(|(path, _)| path.as_str() != changed_path)
            .filter(|(path, document)| {
                let dependencies = document_dependencies(path, &document.text);
                affected_modules.is_empty()
                    || dependencies.iter().any(|dependency| {
                        affected_modules
                            .iter()
                            .any(|module| dependency_matches_module(dependency, module))
                    })
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        for dependent in dependents {
            let removed = retain_other_paths(&mut self.diagnostic_cache, &dependent);
            self.invalidations = self.invalidations.saturating_add(removed);
        }
    }

    pub fn snapshot(&self, path: &str) -> Option<DocumentSnapshot> {
        self.documents.get(path).map(|document| DocumentSnapshot {
            path: path.to_string(),
            revision: document.revision,
            kind: document.kind,
            text: Arc::clone(&document.text),
        })
    }

    pub fn diagnostics(&mut self, path: &str) -> Vec<Diagnostic> {
        self.diagnostics_with(path, LanguageRequest::default())
            .unwrap_or_default()
    }

    pub fn diagnostics_with(
        &mut self,
        path: &str,
        request: LanguageRequest<'_>,
    ) -> Result<Vec<Diagnostic>, LanguageServiceError> {
        check_request(request)?;
        let Some(document) = self.documents.get(path).cloned() else {
            return Ok(Vec::new());
        };
        let cache_key = (path.to_string(), document.revision);
        if let Some(cached) = self.diagnostic_cache.get(&cache_key).cloned() {
            self.cache_hits += 1;
            self.record_hit(QueryKind::Diagnostics);
            return Ok(cached
                .iter()
                .take(request.max_diagnostics)
                .cloned()
                .collect());
        }
        self.cache_misses += 1;
        self.record_miss(QueryKind::Diagnostics);
        let dependencies = self.dependencies(path);
        let visible_paths = visible_interface_paths(&self.documents, path, &dependencies);
        let interfaces = self
            .documents
            .iter()
            .filter(|(_, candidate)| candidate.kind == DocumentKind::Interface)
            .filter(|(interface_path, _)| visible_paths.contains(interface_path.as_str()))
            .map(|(path, candidate)| (path.as_str(), candidate.text.as_ref()))
            .collect::<Vec<_>>();
        let operation = OperationContext {
            cancellation: request.cancellation.cloned(),
            deadline: request.deadline,
            ..OperationContext::default()
        };
        let mut diagnostics = match document.kind {
            DocumentKind::Source if interfaces.is_empty() => {
                analyze_source_result_with_operation(path, &document.text, &operation)
                    .into_diagnostics()
            }
            DocumentKind::Source => analyze_source_with_interfaces_result_with_operation(
                path,
                &document.text,
                &interfaces,
                &operation,
            )
            .into_diagnostics(),
            DocumentKind::Interface => {
                let visible = interfaces
                    .iter()
                    .copied()
                    .filter(|(candidate, _)| *candidate != path)
                    .collect::<Vec<_>>();
                analyze_source_with_interfaces_result_with_operation(
                    path,
                    &document.text,
                    &visible,
                    &operation,
                )
                .into_diagnostics()
            }
        };
        diagnostics.extend(self.lint(path));
        check_request(request)?;
        self.diagnostic_cache
            .insert(cache_key, Arc::from(diagnostics.clone()));
        diagnostics.truncate(request.max_diagnostics);
        Ok(diagnostics)
    }

    pub fn format(&mut self, path: &str) -> Option<String> {
        let document = self.documents.get(path)?.clone();
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.format_cache.get(&key) {
            let value = cached.to_string();
            self.record_hit(QueryKind::Format);
            return Some(value);
        }
        let value: Arc<str> = format_source(path, &document.text).into();
        self.format_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::Format);
        Some(value.to_string())
    }

    pub fn symbols(&mut self, path: &str) -> Option<SymbolIndex> {
        let document = self.documents.get(path)?.clone();
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.symbol_cache.get(&key) {
            let value = cached.as_ref().clone();
            self.record_hit(QueryKind::Symbols);
            return Some(value);
        }
        let value = Arc::new(symbol_index(path, &document.text));
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
        let value: Arc<[RssDocumentSymbol]> = document_symbols(path, &document.text).into();
        self.document_symbol_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::DocumentSymbols);
        value.to_vec()
    }

    fn lint(&mut self, path: &str) -> Vec<Diagnostic> {
        let Some(document) = self.documents.get(path).cloned() else {
            return Vec::new();
        };
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.lint_cache.get(&key) {
            let value = cached.to_vec();
            self.record_hit(QueryKind::Lint);
            return value;
        }
        let value: Arc<[Diagnostic]> = lint_source(path, &document.text).into();
        self.lint_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::Lint);
        value.to_vec()
    }

    fn dependencies(&mut self, path: &str) -> Arc<[String]> {
        let Some(document) = self.documents.get(path).cloned() else {
            return Arc::from([]);
        };
        let key = (path.to_string(), document.revision);
        if let Some(cached) = self.dependency_cache.get(&key) {
            let value = Arc::clone(cached);
            self.record_hit(QueryKind::Dependencies);
            return value;
        }
        let value: Arc<[String]> = document_dependencies(path, &document.text)
            .into_iter()
            .collect::<Vec<_>>()
            .into();
        self.dependency_cache.insert(key, Arc::clone(&value));
        self.record_miss(QueryKind::Dependencies);
        value
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

fn retain_other_paths<V>(cache: &mut BTreeMap<(String, u64), V>, path: &str) -> u64 {
    let before = cache.len();
    cache.retain(|(cached_path, _), _| cached_path != path);
    u64::try_from(before.saturating_sub(cache.len())).unwrap_or(u64::MAX)
}

fn declaration_target(line: &str, keyword: &str) -> Option<String> {
    let line = line.trim();
    let remainder = line.strip_prefix(keyword)?.trim_start();
    if remainder.is_empty() {
        return None;
    }
    let target = remainder
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '.' | '*' | '{' | '}' | ',')
        })
        .collect::<String>();
    (!target.is_empty()).then_some(target)
}

fn interface_modules(path: &str, text: &str) -> BTreeSet<String> {
    let declared = text
        .lines()
        .filter_map(|line| declaration_target(line, "module"))
        .collect::<BTreeSet<_>>();
    if !declared.is_empty() {
        return declared;
    }
    let fallback = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".rssi")
        .trim_end_matches(".rss");
    (!fallback.is_empty())
        .then(|| fallback.to_string())
        .into_iter()
        .collect()
}

fn document_dependencies(_path: &str, text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| declaration_target(line, "use"))
        .collect()
}

fn dependency_matches_module(dependency: &str, module: &str) -> bool {
    dependency == module
        || dependency
            .strip_prefix(module)
            .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('{'))
}

fn visible_interface_paths(
    documents: &BTreeMap<String, Document>,
    current_path: &str,
    root_dependencies: &[String],
) -> BTreeSet<String> {
    let mut dependencies = root_dependencies.iter().cloned().collect::<BTreeSet<_>>();
    let mut visible = BTreeSet::new();
    loop {
        let mut changed = false;
        for (path, document) in documents {
            if path == current_path || document.kind != DocumentKind::Interface {
                continue;
            }
            let selected = interface_modules(path, &document.text)
                .iter()
                .any(|module| {
                    dependencies
                        .iter()
                        .any(|dependency| dependency_matches_module(dependency, module))
                });
            if selected && visible.insert(path.clone()) {
                dependencies.extend(document_dependencies(path, &document.text));
                changed = true;
            }
        }
        if !changed {
            return visible;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn revisions_replace_snapshots_and_removal_is_explicit() {
        let mut service = LanguageService::default();
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
        let mut service = LanguageService::default();
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
        let mut service = LanguageService::default();
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
    fn unrelated_interface_edit_preserves_semantic_query_cache() {
        let mut service = LanguageService::default();
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
        assert_eq!(service.query_stats(QueryKind::Diagnostics).misses, 1);
        assert_eq!(service.query_stats(QueryKind::Diagnostics).hits, 1);
    }

    #[test]
    fn interface_edit_invalidates_semantics_but_reuses_local_queries() {
        let mut service = LanguageService::default();
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
        assert!(service.stats().invalidations >= 1);
    }

    #[test]
    fn transitive_interface_dependency_invalidates_importing_source() {
        let mut service = LanguageService::default();
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
    fn cancelled_and_expired_requests_do_not_enter_analysis() {
        let mut service = LanguageService::default();
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
        let mut service = LanguageService::default();
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
}
