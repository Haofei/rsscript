//! Spec §4.3 — source maps and diagnostic remapping
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn runtime_diagnostic_lines_parse_to_rsscript_diagnostics() {
    let stderr = r#"thread 'main' panicked at runtime/src/lib.rs:1:1:
RSSCRIPT_RUNTIME_DIAGNOSTIC:{"code":"RS1201","severity":"error","summary":"RSScript runtime error: resource pool has no available resources","file":"pool.rss","line":8,"column":10,"length":24,"label":"resource pool has no available resources","kind":"resource_pool_empty"}
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"#;

    let diagnostics = parse_runtime_diagnostics(stderr);

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "RS1201");
    assert_eq!(
        diagnostics[0].summary,
        "RSScript runtime error: resource pool has no available resources"
    );
    assert_eq!(diagnostics[0].span.file, "pool.rss");
    assert_eq!(diagnostics[0].span.line, 8);
    assert!(
        diagnostics[0]
            .causes
            .iter()
            .any(|cause| cause == "runtime error kind: resource_pool_empty")
    );
}

#[test]
fn rust_lowering_is_gated_by_diagnostics() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn bad(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    Image.inspect(image: read image)
}
"#;
    let diagnostics = lower_source_to_rust("bad.rss", source).expect_err("source should fail");
    let codes = diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0401".to_string()));
}
