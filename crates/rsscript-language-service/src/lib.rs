#![forbid(unsafe_code)]

//! Editor-facing RSScript language service.
//!
//! This crate is the only compiler-facing dependency of the LSP. Its API is
//! deliberately document-oriented so editor clients do not couple themselves to
//! analyzer databases, runtime values, VM registers, or optional backends.

use std::collections::BTreeMap;
use std::sync::Arc;

pub use rsscript::{
    Definition, Diagnostic, DiagnosticExplanation, PackageReviewFileKind, Reference,
    RssDocumentSymbol, Severity, Span, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    analyze_source_with_core, analyze_source_with_interfaces, analyze_sources_with_interfaces,
    document_symbols, explain_diagnostic_code, format_source, lint_source,
    package_sources_with_dependency_interfaces, symbol_index,
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
}

impl LanguageService {
    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        revision: u64,
        kind: DocumentKind,
        text: impl Into<Arc<str>>,
    ) {
        self.documents.insert(
            path.into(),
            Document {
                revision,
                kind,
                text: text.into(),
            },
        );
    }

    pub fn remove_file(&mut self, path: &str) -> bool {
        self.documents.remove(path).is_some()
    }

    pub fn snapshot(&self, path: &str) -> Option<DocumentSnapshot> {
        self.documents.get(path).map(|document| DocumentSnapshot {
            path: path.to_string(),
            revision: document.revision,
            kind: document.kind,
            text: Arc::clone(&document.text),
        })
    }

    pub fn diagnostics(&self, path: &str) -> Vec<Diagnostic> {
        let Some(document) = self.documents.get(path) else {
            return Vec::new();
        };
        let interfaces = self
            .documents
            .iter()
            .filter(|(_, candidate)| candidate.kind == DocumentKind::Interface)
            .map(|(path, candidate)| (path.as_str(), candidate.text.as_ref()))
            .collect::<Vec<_>>();
        let mut diagnostics = match document.kind {
            DocumentKind::Source if interfaces.is_empty() => {
                analyze_source_with_core(path, &document.text)
            }
            DocumentKind::Source => {
                analyze_source_with_interfaces(path, &document.text, &interfaces)
            }
            DocumentKind::Interface => {
                let visible = interfaces
                    .iter()
                    .copied()
                    .filter(|(candidate, _)| *candidate != path)
                    .collect::<Vec<_>>();
                analyze_source_with_interfaces(path, &document.text, &visible)
            }
        };
        diagnostics.extend(lint_source(path, &document.text));
        diagnostics
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
