//! Data contracts owned by the experimental Rust/AOT backend.
//!
//! The Core compiler only reaches this crate behind its explicit `aot-rust`
//! feature. These types are not part of the reviewed SDK surface.

use rsscript_diagnostics::{Diagnostic, Severity, Span, code};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRustPackage {
    pub package_name: String,
    pub cargo_toml: String,
    pub lib_rs: String,
    pub main_rs: Option<String>,
    pub source_map_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredRust {
    pub rust_source: String,
    pub source_map: Vec<RustSourceMapEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RustSourceMapEntry {
    pub kind: String,
    pub source: Span,
    pub generated: Span,
    /// The enclosing RSScript source symbol, stamped per function so a backend
    /// error can name the declaration it maps to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The Rust symbol produced for the enclosing RSScript function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowered_symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemappedRustcDiagnostic {
    pub diagnostic: Diagnostic,
    pub mapped: bool,
}

/// One deterministic coverage partition for an experimental AOT capability.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageBucket {
    pub all: Vec<String>,
    pub supported: Vec<String>,
    pub missing: Vec<String>,
}

impl CoverageBucket {
    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn supported_count(&self) -> usize {
        self.supported.len()
    }

    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }
}

/// Experimental AOT coverage facts, intentionally separate from the Core
/// language feature matrix.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LowerCoverageReport {
    pub runtime_intrinsics: CoverageBucket,
    pub ast_statements: CoverageBucket,
    pub ast_expressions: CoverageBucket,
    pub function_kinds: CoverageBucket,
}

/// Builds a stable partition: every input is sorted/deduplicated, only known
/// supported entries survive, and `missing` is the exact complement.
pub fn coverage_bucket(
    all: impl IntoIterator<Item = String>,
    supported: BTreeSet<String>,
) -> CoverageBucket {
    let mut all = all.into_iter().collect::<Vec<_>>();
    all.sort();
    all.dedup();
    let all_set = all.iter().cloned().collect::<BTreeSet<_>>();
    let mut supported = supported
        .into_iter()
        .filter(|item| all_set.contains(item))
        .collect::<Vec<_>>();
    supported.sort();
    let supported_set = supported.iter().cloned().collect::<BTreeSet<_>>();
    let missing = all
        .iter()
        .filter(|item| !supported_set.contains(*item))
        .cloned()
        .collect();
    CoverageBucket {
        all,
        supported,
        missing,
    }
}

/// Parses a serialized generated-Rust source map.
pub fn parse_source_map_json(source_map_json: &str) -> Result<Vec<RustSourceMapEntry>, String> {
    serde_json::from_str(source_map_json)
        .map_err(|error| format!("failed to parse RSScript source map JSON: {error}"))
}

/// Maps one rustc JSON diagnostic back to an RSScript source-map entry.
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
        return Ok(Some(RemappedRustcDiagnostic {
            diagnostic: Diagnostic {
                code: code::RUSTC_DIAGNOSTIC_MAPPED.to_string(),
                severity,
                summary,
                span: entry.source.clone(),
                label: "backend diagnostic maps to this RSScript construct".to_string(),
                causes,
                fixes: Vec::new(),
            },
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
    Ok(Some(RemappedRustcDiagnostic {
        diagnostic: Diagnostic {
            code: code::RUSTC_DIAGNOSTIC_UNMAPPABLE.to_string(),
            severity: rustc_severity(&rustc.level),
            summary: format!("unmappable backend diagnostic: {}", rustc.message),
            span: generated,
            label: "generated Rust diagnostic could not be mapped to RSScript source".to_string(),
            causes: vec![format!("rustc code: {backend_code}"), rustc.message],
            fixes: Vec::new(),
        },
        mapped: false,
    }))
}

/// Maps each non-empty rustc JSON line in order.
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

/// Parses structured diagnostics emitted by a generated AOT program on stderr.
pub fn parse_runtime_diagnostics(stderr: &str) -> Vec<Diagnostic> {
    stderr
        .lines()
        .filter_map(parse_runtime_diagnostic_line)
        .collect()
}

fn parse_runtime_diagnostic_line(line: &str) -> Option<Diagnostic> {
    const PREFIX: &str = "RSSCRIPT_RUNTIME_DIAGNOSTIC:";
    let start = line.find(PREFIX)? + PREFIX.len();
    let wire: RuntimeDiagnosticJson = serde_json::from_str(&line[start..]).ok()?;
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
    (value.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-message"))
        .then(|| value.get("message"))
        .flatten()
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

#[cfg(test)]
mod tests {
    use super::{
        RustSourceMapEntry, parse_runtime_diagnostics, parse_source_map_json,
        remap_rustc_diagnostic_json,
    };
    use rsscript_diagnostics::Span;

    #[test]
    fn source_map_contract_round_trips() {
        let entry = RustSourceMapEntry {
            kind: "call".to_string(),
            source: Span {
                file: "main.rss".to_string(),
                line: 2,
                column: 3,
                length: 4,
            },
            generated: Span {
                file: "src/lib.rs".to_string(),
                line: 8,
                column: 5,
                length: 9,
            },
            symbol: Some("main".to_string()),
            lowered_symbol: Some("rss_main".to_string()),
        };
        let encoded = serde_json::to_string(&entry).expect("model must serialize");
        let decoded: RustSourceMapEntry =
            serde_json::from_str(&encoded).expect("model must deserialize");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn remaps_rustc_diagnostic_through_source_map() {
        let entry = RustSourceMapEntry {
            kind: "call".to_string(),
            source: Span {
                file: "main.rss".to_string(),
                line: 2,
                column: 3,
                length: 4,
            },
            generated: Span {
                file: "src/lib.rs".to_string(),
                line: 8,
                column: 5,
                length: 2,
            },
            symbol: Some("main".to_string()),
            lowered_symbol: None,
        };
        let report = remap_rustc_diagnostic_json(
            &[entry],
            r#"{"message":"broken","level":"error","code":{"code":"E0001"},"spans":[{"file_name":"src/lib.rs","line_start":8,"column_start":5,"column_end":6,"is_primary":true}]}"#,
        )
        .expect("valid rustc JSON")
        .expect("error diagnostic");
        assert!(report.mapped);
        assert_eq!(report.diagnostic.span.file, "main.rss");
        assert_eq!(report.diagnostic.span.line, 2);
    }

    #[test]
    fn source_map_parser_rejects_invalid_json() {
        assert!(parse_source_map_json("not json").is_err());
    }

    #[test]
    fn runtime_diagnostics_keep_kind_and_warning_severity() {
        let diagnostics = parse_runtime_diagnostics(
            "prefix RSSCRIPT_RUNTIME_DIAGNOSTIC:{\"severity\":\"warning\",\"summary\":\"slow\",\"file\":\"main.rss\",\"line\":2,\"column\":3,\"length\":1,\"label\":\"wait\",\"kind\":\"deadline\"}\n",
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].severity,
            rsscript_diagnostics::Severity::Warning
        );
        assert!(
            diagnostics[0]
                .causes
                .iter()
                .any(|cause| cause == "runtime error kind: deadline")
        );
    }

    #[test]
    fn coverage_bucket_is_sorted_and_filters_unknown_support() {
        let bucket = super::coverage_bucket(
            ["b".to_string(), "a".to_string(), "a".to_string()],
            ["a".to_string(), "missing".to_string()]
                .into_iter()
                .collect(),
        );
        assert_eq!(bucket.all, ["a", "b"]);
        assert_eq!(bucket.supported, ["a"]);
        assert_eq!(bucket.missing, ["b"]);
    }
}
