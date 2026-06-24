use crate::diagnostic::Span;

use super::types::RustSourceMapEntry;

pub fn parse_source_map_json(source_map_json: &str) -> Result<Vec<RustSourceMapEntry>, String> {
    serde_json::from_str(source_map_json)
        .map_err(|error| format!("failed to parse RSScript source map JSON: {error}"))
}

pub(super) fn push_source_marker(
    out: &mut String,
    indent: usize,
    kind: &str,
    span: &Span,
) -> RustSourceMapEntry {
    let marker = format!(
        "{}// rss:span kind={kind} file={} line={} column={} length={}\n",
        "    ".repeat(indent),
        source_marker_value(&span.file),
        span.line,
        span.column,
        span.length
    );
    let generated = generated_span_at_end(out, "src/lib.rs", &marker);
    out.push_str(&marker);
    RustSourceMapEntry {
        kind: kind.to_string(),
        source: span.clone(),
        generated,
        ..Default::default()
    }
}

pub(super) fn generated_span_at_end(out: &str, file: &str, text: &str) -> Span {
    let (line, column) = generated_position(out);
    Span {
        file: file.to_string(),
        line,
        column,
        length: text.trim_end_matches('\n').chars().count().max(1),
    }
}

fn generated_position(out: &str) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for ch in out.chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn source_marker_value(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\n' | '\r' | '\t' => ['_'].into_iter().collect::<Vec<_>>(),
            _ => [character].into_iter().collect(),
        })
        .collect()
}
