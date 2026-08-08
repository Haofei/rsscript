//! Spec §6.4/§7.3 — resource & with-scope lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn checker_allows_arithmetic_with_typed_builtin_call_operands() {
    let source = r#"
fn main() -> Unit {
    let value = 20 + String.len(value: read "rss")
    return Unit
}
"#;
    let diagnostics = analyze_source_with_core("typed-arithmetic.rss", source);

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS1001"),
        "{diagnostics:?}"
    );
}
