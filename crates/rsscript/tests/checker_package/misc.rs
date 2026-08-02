//! package tests not yet categorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn docs_do_not_reintroduce_legacy_gc_runtime_surface() {
    let root = common::workspace_root();
    let legacy_runtime_name = ["runtime ", "G", "c"].concat();
    let legacy_runtime_path = ["rsscript_runtime::", "G", "c"].concat();
    let legacy_review_category = ["safe", "_to_", "skip"].concat();

    for doc_path in [
        root.join("README.md"),
        common::language_spec_path(),
        common::package_reference_path(),
    ] {
        let relative_path = doc_path
            .strip_prefix(&root)
            .unwrap_or(&doc_path)
            .display()
            .to_string();
        let source = fs::read_to_string(&doc_path)
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
fn package_reference_uses_current_http_and_env_facade_shapes() {
    let spec = fs::read_to_string(common::package_reference_path())
        .expect("package reference should be readable");

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
            "package reference should not reference stale facade shape `{stale}`"
        );
    }
    for current in [
        "pub fn Http.get(\n    url: read Url,\n) -> Result<fresh HttpResponse, HttpError>",
        "pub fn HttpResponse.text(\n    response: read HttpResponse,\n) -> fresh String",
        "pub fn Env.get(name: read String) -> Option<fresh String>",
        "pub fn Env.get_or_default(",
    ] {
        assert!(
            spec.contains(current),
            "package reference should document current facade shape `{current}`"
        );
    }
}

#[test]
fn rss_spec_documents_external_binding_dispatch_and_rejects_implicit_dyn() {
    // Dynamic dispatch is implemented ONLY as the explicit `Dyn<Protocol>`
    // form (§20.2-2). The spec must document that, and must still reject implicit
    // protocol-typed values / Rust-style `dyn` coercion as non-goals.
    let spec = fs::read_to_string(common::language_spec_path())
        .unwrap_or_else(|error| panic!("RSScript spec should read: {error}"));

    for forbidden in [
        "RSScript admits protocol-typed dynamic dispatch",
        "implicit dyn coercion is supported",
        "form is admitted, not excluded",
        "protocol_dynamic_dispatch",
    ] {
        assert!(
            !spec.contains(forbidden),
            "implicit dynamic dispatch must stay a non-goal, found `{forbidden}`"
        );
    }
    assert!(spec.contains("Dynamic dispatch (explicit external_binding form implemented §20.2-2)"));
    assert!(
        spec.contains("`Dyn<Protocol>` form (with the `external_binding Protocol` keyword sugar)")
    );
    assert!(spec.contains("Rust-style `dyn Trait` vtable coercion"));
    assert!(spec.contains("`Protocol.method(...)` dispatch backed by an explicit generic bound"));
}
