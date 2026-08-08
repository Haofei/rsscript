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

#[test]
fn checker_rejects_empty_and_multi_scalar_char_literals() {
    // Regression: `''` (empty) used to panic in `decode_char_token`, and `'ab'`
    // (multi-scalar) silently truncated to `'a'` in release builds — both now a
    // clean RS0038 frontend diagnostic.
    let empty = analyze_source(
        "char-empty.rss",
        "fn main() -> Unit {\n    let c = ''\n    return Unit\n}\n",
    );
    assert!(
        empty.iter().any(|d| d.code == "RS0038"),
        "empty char literal must report RS0038, got {empty:?}"
    );

    let multi = analyze_source(
        "char-multi.rss",
        "fn main() -> Unit {\n    let c = 'ab'\n    return Unit\n}\n",
    );
    assert!(
        multi.iter().any(|d| d.code == "RS0038"),
        "multi-scalar char literal must report RS0038, got {multi:?}"
    );

    // A well-formed single-scalar literal (including an escape) is accepted.
    let ok = analyze_source(
        "char-ok.rss",
        "fn main() -> Unit {\n    let a = 'x'\n    let b = '\\n'\n    return Unit\n}\n",
    );
    assert!(
        !ok.iter().any(|d| d.code == "RS0038"),
        "single-scalar char literals must not report RS0038, got {ok:?}"
    );
}
