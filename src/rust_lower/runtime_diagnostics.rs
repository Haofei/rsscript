use crate::diagnostic::{Diagnostic, Span, code};

const RUNTIME_DIAGNOSTIC_PREFIX: &str = "RSSCRIPT_RUNTIME_DIAGNOSTIC:";

#[derive(serde::Deserialize)]
struct RuntimeDiagnosticJson {
    code: Option<String>,
    severity: Option<String>,
    summary: String,
    file: String,
    line: usize,
    column: usize,
    length: usize,
    label: String,
    kind: Option<String>,
}

pub fn parse_runtime_diagnostics(stderr: &str) -> Vec<Diagnostic> {
    stderr
        .lines()
        .filter_map(parse_runtime_diagnostic_line)
        .collect()
}

fn parse_runtime_diagnostic_line(line: &str) -> Option<Diagnostic> {
    let start = line.find(RUNTIME_DIAGNOSTIC_PREFIX)? + RUNTIME_DIAGNOSTIC_PREFIX.len();
    let payload = &line[start..];
    let wire: RuntimeDiagnosticJson = serde_json::from_str(payload).ok()?;
    let code = wire
        .code
        .unwrap_or_else(|| code::RUNTIME_DIAGNOSTIC.to_string());
    let span = Span {
        file: wire.file,
        line: wire.line,
        column: wire.column,
        length: wire.length,
    };
    let mut diagnostic = match wire.severity.as_deref() {
        Some("warning") => Diagnostic::warning(&code, wire.summary, span, wire.label),
        _ => Diagnostic::error(&code, wire.summary, span, wire.label),
    };
    if let Some(kind) = wire.kind {
        diagnostic = diagnostic.with_cause(format!("runtime error kind: {kind}"));
    }
    Some(diagnostic)
}
