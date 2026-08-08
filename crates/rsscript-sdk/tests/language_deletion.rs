use rsscript_sdk::syntax::ast::Item;
use rsscript_sdk::syntax::parse_source;
use rsscript_sdk::{Severity, analyze_source, analyze_source_with_interfaces};

fn has_error(diagnostics: &[rsscript_sdk::Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

#[test]
fn local_and_async_need_no_file_feature() {
    let local = "fn main() -> Unit { local value = [1]; let _ = take value; return Unit }";
    assert!(!has_error(&analyze_source("local.rss", local)));

    let asynchronous = "async fn work() -> Unit { return Unit }";
    assert!(!has_error(&analyze_source("async.rss", asynchronous)));
}

#[test]
fn source_rejects_bodyless_function_but_interface_accepts_it() {
    let declaration = "pub fn Host.emit(message: read String) -> Unit\n";
    let source_diagnostics = analyze_source("main.rss", declaration);
    assert!(source_diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "bodyless source function"
    }));

    let program = parse_source("host.rssi", declaration);
    assert!(matches!(&program.items[0], Item::Function(function) if !function.has_body));
    let consumer = "fn main() -> Unit { Host.emit(message: read \"ok\"); return Unit }";
    assert!(!has_error(&analyze_source_with_interfaces(
        "main.rss",
        consumer,
        &[("host.rssi", declaration)],
    )));
}

#[test]
fn retains_is_structured_and_checked() {
    let interface = "pub fn Store.put(value: read String) -> Unit retains(value)\n";
    let program = parse_source("store.rssi", interface);
    let Item::Function(function) = &program.items[0] else {
        panic!("expected function declaration");
    };
    assert_eq!(function.retained_params, ["value"]);

    let invalid = "pub fn Store.put(value: read String) -> Unit retains(missing)\n";
    assert!(has_error(&analyze_source("store.rssi", invalid)));
}

#[test]
fn removed_declaration_syntax_is_rejected() {
    for (file, source) in [
        (
            "feature.rss",
            "features: local\nfn main() -> Unit { return Unit }",
        ),
        (
            "profile.rss",
            "profile: retired\nfn main() -> Unit { return Unit }",
        ),
        (
            "effect.rss",
            "fn main() -> Unit effects(pure) { return Unit }",
        ),
        ("native.rss", "native fn Host.emit() -> Unit"),
        ("unsafe.rss", "unsafe fn main() -> Unit { return Unit }"),
    ] {
        assert!(has_error(&analyze_source(file, source)), "{file}");
    }
}
