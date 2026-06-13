//! review tests not yet categorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn checker_does_not_resolve_qualified_calls_by_short_name() {
    let source = r#"
fn run(value: read Int) -> Int {
    return value
}

fn caller(value: read Int) -> Int {
    return Mystery.run(value: read value)
}
"#;
    let diagnostics = analyze_source("qualified-short-name.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0206" && diagnostic.summary.contains("Mystery.run")
    }));
}
