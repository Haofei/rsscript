//! package tests not yet categorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn docs_do_not_reintroduce_legacy_gc_runtime_surface() {
    let root = common::workspace_root();
    let legacy_runtime_name = ["runtime ", "G", "c"].concat();
    let legacy_runtime_path = ["rsscript_runtime::", "G", "c"].concat();
    let legacy_review_category = ["safe", "_to_", "skip"].concat();

    for relative_path in [
        "README.md",
        "docs/RSScript_v0.6_Spec.md",
        "docs/RSScript_Package_Manager_Design_v0.6.md",
    ] {
        let source = fs::read_to_string(root.join(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should read: {error}"));

        assert!(
            !source.contains(&legacy_runtime_name),
            "{relative_path} must describe managed runtime values as Managed<T>, not Gc"
        );
        assert!(
            !source.contains(&legacy_runtime_path),
            "{relative_path} must not expose legacy managed runtime aliases"
        );
        assert!(
            !source.contains(&legacy_review_category),
            "{relative_path} must emit low_semantic_risk instead of legacy review categories"
        );
    }
}

#[test]
fn package_manager_spec_uses_current_http_and_env_facade_shapes() {
    let root = common::workspace_root();
    let spec = fs::read_to_string(root.join("docs/RSScript_Package_Manager_Design_v0.6.md"))
        .expect("package manager spec should be readable");

    for stale in [
        "Http.HttpClient",
        "Http.Response",
        "Http.HttpError",
        "Http.Url",
        "Http.body_text",
        "Env.EnvError",
        "Result<String, Env.EnvError>",
    ] {
        assert!(
            !spec.contains(stale),
            "package manager spec should not reference stale facade shape `{stale}`"
        );
    }
    for current in [
        "pub native fn Http.get(\n    url: read Url,\n) -> Result<fresh HttpResponse, HttpError>",
        "pub native fn HttpResponse.text(\n    response: read HttpResponse,\n) -> fresh String",
        "pub native fn Env.get(name: read String) -> Option<fresh String>",
        "pub native fn Env.get_or_default(",
    ] {
        assert!(
            spec.contains(current),
            "package manager spec should document current facade shape `{current}`"
        );
    }
}

#[test]
fn rss_spec_keeps_protocol_dynamic_dispatch_deferred() {
    let root = common::workspace_root();
    let spec = fs::read_to_string(root.join("docs/RSScript_v0.6_Spec.md"))
        .unwrap_or_else(|error| panic!("RSScript spec should read: {error}"));

    for forbidden in [
        "Dynamic dispatch (admitted",
        "RSScript admits protocol-typed dynamic dispatch",
        "The design decision is settled: dynamic dispatch is supported",
        "form is admitted, not excluded",
        "protocol_dynamic_dispatch",
    ] {
        assert!(
            !spec.contains(forbidden),
            "v0.6 protocol dynamic dispatch must remain deferred, found `{forbidden}`"
        );
    }
    assert!(spec.contains("Dynamic dispatch (deferred, not admitted in v0.6)"));
    assert!(spec.contains("The only implemented and specified protocol call form is"));
    assert!(spec.contains("explicit `Protocol.method(...)` dispatch"));
}
