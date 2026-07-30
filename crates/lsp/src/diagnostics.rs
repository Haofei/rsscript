//! Checker execution and conversion of checker diagnostics to LSP diagnostics.

use std::collections::HashMap;

use rsscript::{Diagnostic as RsDiagnostic, *};
use serde_json::json;
use tower_lsp::lsp_types::{Diagnostic as LspDiagnostic, *};

use crate::documents::*;
use crate::text::*;
use crate::workspace::*;

#[cfg(test)]
pub(crate) fn diagnostics_for_uri(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
) -> Vec<RsDiagnostic> {
    diagnostics_for_uri_cancellable(uri, open_documents, &PackageInputCache::default(), || false)
        .unwrap_or_default()
}

pub(crate) fn diagnostics_for_uri_cancellable(
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
pub(crate) fn lsp_diagnostics_for_uri(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
) -> Vec<LspDiagnostic> {
    let diagnostics = diagnostics_for_uri(uri, open_documents);
    lsp_diagnostics_from_diagnostics(uri, open_documents, &diagnostics)
}

pub(crate) fn lsp_diagnostics_from_diagnostics(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
    diagnostics: &[RsDiagnostic],
) -> Vec<LspDiagnostic> {
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
pub(crate) fn single_file_diagnostics(path: &str, text: &str) -> Vec<RsDiagnostic> {
    let mut diagnostics = analyze_source_with_core(path, text);
    diagnostics.extend(lint_source(path, text));
    diagnostics
}

pub(crate) fn package_frontend_diagnostics_cancellable(
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

pub(crate) fn dedup_diagnostics(diagnostics: &mut Vec<RsDiagnostic>) {
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

pub(crate) fn to_lsp_diagnostic(source: &str, diagnostic: &RsDiagnostic) -> LspDiagnostic {
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

    LspDiagnostic {
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
        ..LspDiagnostic::default()
    }
}

pub(crate) fn diagnostic_data(diagnostic: &RsDiagnostic) -> serde_json::Value {
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
