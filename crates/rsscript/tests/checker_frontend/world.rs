//! Spec §7 — bindings and world boundaries
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn fresh_return_allows_inline_field_of_clean_local() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

struct Metadata

struct DecodeResult {
    image: Image
    metadata: Metadata
}

fn decode(path: read Path) -> fresh DecodeResult

fn load_image(path: read Path) -> fresh Image {
    local decoded = decode(path: read path)
    return decoded.image
}
"#;

    assert_eq!(analyze_source("fresh-inline-field.rss", source), Vec::new());
}

#[test]
fn fresh_return_allows_wrapped_inline_field_of_clean_local() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

struct Metadata

struct DecodeResult {
    image: Image
    metadata: Metadata
}

fn decode(path: read Path) -> fresh DecodeResult

fn load_image(path: read Path) -> Result<fresh Image, ImageError> {
    local decoded = decode(path: read path)
    return Ok(read decoded.image)
}
"#;

    assert_eq!(
        analyze_source("fresh-inline-field-wrapper.rss", source),
        Vec::new()
    );
}

#[test]
fn fresh_return_rejects_handle_field_of_clean_local() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

struct ImageBox {
    image: handle Image
}

fn load_box(path: read Path) -> fresh ImageBox

fn bad_image(path: read Path) -> fresh Image {
    local boxed = load_box(path: read path)
    return boxed.image
}
"#;
    let codes = analyze_source("fresh-handle-field.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0601".to_string()));
}

#[test]
fn checker_accepts_fresh_list_for_loop() {
    let source = r#"
fn names() -> fresh List<String> {
    return ["a", "b"]
}

fn run() -> String {
    for name in names() {
        return name
    }
    return ""
}
"#;
    let diagnostics = analyze_source("fresh-list-for.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("fresh-list-for.rss", source).expect("for should lower");
    assert!(lowered.contains("for name in (names()).iter()"));
}
