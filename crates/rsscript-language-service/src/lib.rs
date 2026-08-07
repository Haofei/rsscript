#![forbid(unsafe_code)]

//! Revisioned, cached editor-facing RSScript language service.
//!
//! This crate is the only compiler-facing dependency of the LSP. Its API is
//! deliberately document-oriented so editor clients do not couple themselves to
//! analyzer databases, runtime values, VM registers, or optional backends.
//! Cache invalidation is document/interface-granular; this crate does not claim a
//! query-level incremental semantic engine.

use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationContext};
use std::collections::BTreeMap;
use std::sync::Arc;

pub use rsscript_compiler::language::{
    Definition, Diagnostic, DiagnosticExplanation, Reference, RssDocumentSymbol, Severity, Span,
    SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup, analyze_source_result_with_operation,
    analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_with_interfaces_result_with_operation, analyze_sources_with_interfaces,
    document_symbols, explain_diagnostic_code, format_source, lint_source, symbol_index,
};

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
    diagnostic_cache: BTreeMap<(String, u64, u64), Arc<[Diagnostic]>>,
    cache_hits: u64,
    cache_misses: u64,
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
        let invalidates_interfaces = kind == DocumentKind::Interface
            || self
                .documents
                .get(&path)
                .is_some_and(|document| document.kind == DocumentKind::Interface);
        self.documents.insert(
            path.clone(),
            Document {
                revision,
                kind,
                text: text.into(),
            },
        );
        if invalidates_interfaces {
            self.diagnostic_cache.clear();
        } else {
            self.diagnostic_cache
                .retain(|(cached_path, _, _), _| cached_path != &path);
        }
    }

    pub fn remove_file(&mut self, path: &str) -> bool {
        let removed = self.documents.remove(path);
        if removed
            .as_ref()
            .is_some_and(|document| document.kind == DocumentKind::Interface)
        {
            self.diagnostic_cache.clear();
        } else {
            self.diagnostic_cache
                .retain(|(cached_path, _, _), _| cached_path != path);
        }
        removed.is_some()
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
        let Some(document) = self.documents.get(path) else {
            return Ok(Vec::new());
        };
        let interface_revision = interface_revision(&self.documents);
        let cache_key = (path.to_string(), document.revision, interface_revision);
        if let Some(cached) = self.diagnostic_cache.get(&cache_key) {
            self.cache_hits += 1;
            return Ok(cached
                .iter()
                .take(request.max_diagnostics)
                .cloned()
                .collect());
        }
        self.cache_misses += 1;
        let interfaces = self
            .documents
            .iter()
            .filter(|(_, candidate)| candidate.kind == DocumentKind::Interface)
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
        diagnostics.extend(lint_source(path, &document.text));
        check_request(request)?;
        self.diagnostic_cache
            .insert(cache_key, Arc::from(diagnostics.clone()));
        diagnostics.truncate(request.max_diagnostics);
        Ok(diagnostics)
    }

    pub fn format(&self, path: &str) -> Option<String> {
        let document = self.documents.get(path)?;
        Some(format_source(path, &document.text))
    }

    pub fn symbols(&self, path: &str) -> Option<SymbolIndex> {
        let document = self.documents.get(path)?;
        Some(symbol_index(path, &document.text))
    }

    pub fn document_symbols(&self, path: &str) -> Vec<RssDocumentSymbol> {
        self.documents
            .get(path)
            .map_or_else(Vec::new, |document| document_symbols(path, &document.text))
    }

    pub fn stats(&self) -> LanguageServiceStats {
        LanguageServiceStats {
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
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

fn interface_revision(documents: &BTreeMap<String, Document>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for (path, document) in documents
        .iter()
        .filter(|(_, document)| document.kind == DocumentKind::Interface)
    {
        for byte in path.as_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        hash = (hash ^ document.revision).wrapping_mul(0x100000001b3);
    }
    hash
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
