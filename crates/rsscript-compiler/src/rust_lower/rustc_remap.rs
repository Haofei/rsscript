use crate::diagnostic::{Diagnostic, Severity, Span, code};

use super::types::{RemappedRustcDiagnostic, RustSourceMapEntry};

pub fn remap_rustc_diagnostic_json(
    source_map: &[RustSourceMapEntry],
    rustc_json: &str,
) -> Result<Option<RemappedRustcDiagnostic>, String> {
    let value: serde_json::Value = serde_json::from_str(rustc_json)
        .map_err(|error| format!("failed to parse rustc JSON line: {error}"))?;
    let Some(value) = rustc_diagnostic_value(&value) else {
        return Ok(None);
    };
    let rustc: RustcJsonDiagnostic = serde_json::from_value(value.clone())
        .map_err(|error| format!("failed to parse rustc JSON diagnostic: {error}"))?;
    if !matches!(rustc.level.as_str(), "error" | "warning") {
        return Ok(None);
    }

    let rustc_span = rustc
        .spans
        .iter()
        .find(|span| span.is_primary)
        .or_else(|| rustc.spans.first());
    let backend_code = rustc
        .code
        .as_ref()
        .map(|code| code.code.as_str())
        .unwrap_or("<none>");

    if let Some(rustc_span) = rustc_span
        && let Some(entry) = best_source_map_entry(
            source_map,
            &rustc_span.file_name,
            rustc_span.line_start,
            rustc_span.column_start,
        )
    {
        let severity = rustc_severity(&rustc.level);
        let summary = format!("backend diagnostic mapped to RSScript: {}", rustc.message);
        let mut causes = vec![format!("rustc code: {backend_code}")];
        if let Some(symbol) = &entry.symbol {
            let lowered = entry.lowered_symbol.as_deref().unwrap_or(symbol);
            causes.push(format!("RSScript symbol: {symbol} (lowered: {lowered})"));
        }
        causes.push(format!(
            "generated Rust: {}:{}:{}",
            rustc_span.file_name, rustc_span.line_start, rustc_span.column_start
        ));
        causes.push(rustc.message);
        let diagnostic = Diagnostic {
            code: code::RUSTC_DIAGNOSTIC_MAPPED.to_string(),
            severity,
            summary,
            span: entry.source.clone(),
            label: "backend diagnostic maps to this RSScript construct".to_string(),
            causes,
            fixes: Vec::new(),
        };
        return Ok(Some(RemappedRustcDiagnostic {
            diagnostic,
            mapped: true,
        }));
    }

    let generated = rustc_span
        .map(generated_span_from_rustc)
        .unwrap_or_else(|| Span {
            file: "<rustc-json>".to_string(),
            line: 1,
            column: 1,
            length: 1,
        });
    let diagnostic = Diagnostic {
        code: code::RUSTC_DIAGNOSTIC_UNMAPPABLE.to_string(),
        severity: rustc_severity(&rustc.level),
        summary: format!("unmappable backend diagnostic: {}", rustc.message),
        span: generated,
        label: "generated Rust diagnostic could not be mapped to RSScript source".to_string(),
        causes: vec![format!("rustc code: {backend_code}"), rustc.message],
        fixes: Vec::new(),
    };
    Ok(Some(RemappedRustcDiagnostic {
        diagnostic,
        mapped: false,
    }))
}

pub fn remap_rustc_diagnostic_json_lines(
    source_map: &[RustSourceMapEntry],
    rustc_json_lines: &str,
) -> Result<Vec<RemappedRustcDiagnostic>, String> {
    let mut diagnostics = Vec::new();
    for line in rustc_json_lines
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        if let Some(diagnostic) = remap_rustc_diagnostic_json(source_map, line)? {
            diagnostics.push(diagnostic);
        }
    }
    Ok(diagnostics)
}

fn best_source_map_entry<'a>(
    source_map: &'a [RustSourceMapEntry],
    file: &str,
    line: usize,
    column: usize,
) -> Option<&'a RustSourceMapEntry> {
    source_map
        .iter()
        .filter(|entry| generated_file_matches(&entry.generated.file, file))
        .filter(|entry| generated_span_contains(&entry.generated, line, column))
        .max_by_key(|entry| {
            (
                entry.generated.line,
                entry.generated.column,
                entry.kind.len(),
            )
        })
}

fn generated_file_matches(left: &str, right: &str) -> bool {
    left.replace('\\', "/") == right.replace('\\', "/")
}

fn generated_span_contains(span: &Span, line: usize, column: usize) -> bool {
    if line < span.line {
        return false;
    }
    if line == span.line {
        return column >= span.column;
    }
    line.saturating_sub(span.line) < span.length
}

fn rustc_severity(level: &str) -> Severity {
    if level == "warning" {
        Severity::Warning
    } else {
        Severity::Error
    }
}

fn rustc_diagnostic_value(value: &serde_json::Value) -> Option<&serde_json::Value> {
    if value.get("level").is_some() && value.get("message").is_some() {
        return Some(value);
    }
    if value.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-message") {
        return value.get("message");
    }
    None
}

fn generated_span_from_rustc(span: &RustcJsonSpan) -> Span {
    Span {
        file: span.file_name.clone(),
        line: span.line_start,
        column: span.column_start,
        length: span.column_end.saturating_sub(span.column_start).max(1),
    }
}

#[derive(serde::Deserialize)]
struct RustcJsonDiagnostic {
    message: String,
    level: String,
    code: Option<RustcJsonCode>,
    #[serde(default)]
    spans: Vec<RustcJsonSpan>,
}

#[derive(serde::Deserialize)]
struct RustcJsonCode {
    code: String,
}

#[derive(serde::Deserialize)]
struct RustcJsonSpan {
    file_name: String,
    line_start: usize,
    column_start: usize,
    column_end: usize,
    #[serde(default)]
    is_primary: bool,
}
