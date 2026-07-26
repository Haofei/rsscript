//! Spec §3/§9 — native boundary lowering and bindings
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_maps_native_call_boundaries() {
    let source = r#"
features: native

native fn host_emit(message: read String) -> Unit
    effects(native)

pub fn run() -> Unit {
    host_emit(message: read "host")
    Log.write(message: read "core")
}
"#;
    let package = lower_sources_to_rust_package_with_options(
        &[("native.rss".to_string(), source.to_string())],
        "Native Example.rss",
        "/workspace/rsscript/runtime",
        &[],
        &[NativeRustDependency {
            crate_name: "host_native".to_string(),
            path: "/workspace/host-native".to_string(),
            cargo_features: Vec::new(),
            default_features: true,
            bindings: BTreeMap::from([("host_emit".to_string(), "host_native::emit".to_string())]),
        }],
    )
    .expect("source should lower with native binding");
    let source_map: Vec<rsscript::RustSourceMapEntry> =
        serde_json::from_str(&package.source_map_json).expect("source map should parse");
    let native_calls = source_map
        .iter()
        .filter(|entry| entry.kind == "native_call")
        .collect::<Vec<_>>();

    assert_eq!(native_calls.len(), 2);
    assert!(native_calls.iter().any(|entry| entry.source.line == 8));
    assert!(native_calls.iter().any(|entry| entry.source.line == 9));
    assert!(
        package
            .lib_rs
            .contains("host_native::emit(&(\"host\".to_string()));")
    );
}

#[test]
fn rust_lowering_maps_receiver_native_binding_with_receiver_argument() {
    let source = r#"
features: native

opaque struct Alpha

native fn Alpha.open() -> Alpha
    effects(native)

native fn Alpha.describe(self: read Alpha) -> String
    effects(native)

pub fn run() -> Unit {
    let alpha = Alpha.open()
    Log.write(message: read alpha.describe())
}
"#;
    let package = lower_sources_to_rust_package_with_options(
        &[("receiver-native.rss".to_string(), source.to_string())],
        "Receiver Native Example.rss",
        "/workspace/rsscript/runtime",
        &[],
        &[NativeRustDependency {
            crate_name: "alpha_native".to_string(),
            path: "/workspace/alpha-native".to_string(),
            cargo_features: Vec::new(),
            default_features: true,
            bindings: BTreeMap::from([
                ("Alpha.open".to_string(), "alpha_native::open".to_string()),
                (
                    "Alpha.describe".to_string(),
                    "alpha_native::describe".to_string(),
                ),
            ]),
        }],
    )
    .expect("source should lower with receiver native binding");

    assert!(
        package.lib_rs.contains("alpha_native::open()"),
        "qualified native call should use bound target, got:\n{}",
        package.lib_rs
    );
    assert!(
        package.lib_rs.contains("alpha_native::describe(&alpha)"),
        "receiver native call should pass receiver as first argument to bound target, got:\n{}",
        package.lib_rs
    );
    assert!(
        !package.lib_rs.contains("Alpha::describe(&alpha)"),
        "receiver native binding should not fall back to generated qualified call, got:\n{}",
        package.lib_rs
    );
}

#[test]
fn rust_lowering_can_emit_native_wrapper_path_dependencies() {
    let source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let package = lower_sources_to_rust_package_with_options(
        &[("main.rss".to_string(), source.to_string())],
        "Native Example.rss",
        "/workspace/rsscript/runtime",
        &[],
        &[NativeRustDependency {
            crate_name: "rss_json_native".to_string(),
            path: "/workspace/rss-json/native/rust".to_string(),
            cargo_features: Vec::new(),
            default_features: true,
            bindings: BTreeMap::new(),
        }],
    )
    .expect("source should lower into package with native dependency");

    assert!(
        package.cargo_toml.contains(
            "\"rss_json_native\" = { path = \"/workspace/rss-json/native/rust\", default-features = true }"
        )
    );
}

#[test]
fn checker_reports_malformed_bindings_and_arguments_as_unsupported() {
    let source = r#"
fn main() -> Unit {
    let missing =
    print(value:)
}
"#;
    let diagnostics = analyze_source("malformed-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "malformed statement")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported expression")
    );
}

#[test]
fn review_reports_new_unsafe_native_usage() {
    let old_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic)
{
    Bytes.checksum(data: read data)
}
"#;
    let new_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic, unsafe, native)
{
    Native.checksum(data: read data)
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let unsafe_finding = findings
        .iter()
        .find(|finding| finding.code == "RSR012")
        .expect("expected unsafe review finding");
    let native_finding = findings
        .iter()
        .find(|finding| finding.code == "RSR015")
        .expect("expected native review finding");

    assert_eq!(unsafe_finding.risk, ReviewRisk::Unsafe);
    assert_eq!(unsafe_finding.before.as_deref(), Some("<none>"));
    assert_eq!(unsafe_finding.after.as_deref(), Some("unsafe"));
    assert_eq!(native_finding.risk, ReviewRisk::Boundary);
    assert_eq!(native_finding.before.as_deref(), Some("<none>"));
    assert_eq!(native_finding.after.as_deref(), Some("native"));
    assert!(
        format_review_human(&findings)
            .contains("RSR012[unsafe]: function `checksum` added unsafe boundary.")
    );
    assert!(
        format_review_human(&findings)
            .contains("RSR015[boundary]: function `checksum` added native boundary.")
    );

    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR012" && item["risk"] == "unsafe")
    }));
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR015" && item["risk"] == "boundary")
    }));
}

#[test]
fn review_reports_native_fn_as_native_boundary() {
    let old_source = r#"
fn host_emit(message: read String) -> Unit
{
    Log.write(message: read message)
}
"#;
    let new_source = r#"
native fn host_emit(message: read String) -> Unit
    effects(native)
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let native_finding = findings
        .iter()
        .find(|finding| finding.code == "RSR015")
        .expect("expected native boundary review finding");

    assert_eq!(native_finding.risk, ReviewRisk::Boundary);
    assert_eq!(native_finding.before.as_deref(), Some("<none>"));
    assert_eq!(native_finding.after.as_deref(), Some("native"));
    assert!(!findings.iter().any(|finding| finding.code == "RSR012"));
}

#[test]
fn review_map_marks_native_calls_inside_noescape_callbacks() {
    let source = r#"
features: native

native fn Native.echo(message: read String) -> String
    effects(native)

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn caller(message: read String) -> Unit {
    apply(callback: || {
        Native.echo(message: read message)
    })
    return Unit
}
"#;
    let map = review_map_sources(vec![("callback-native.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "caller")
        .expect("expected caller region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "native call `Native.echo`"),
        "{region:?}"
    );
}

#[test]
fn review_map_selfhost_classifier_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/selfhost-review-classifier.rss");
    let source = common::read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert!(map.summary.total_functions >= 40, "{map:?}");
    assert!(map.summary.total_lines >= 622, "{map:?}");
    assert_eq!(map.files[0].risk, ReviewMapFileRisk::Elevated);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert!(map.summary.review_required.functions >= 27, "{map:?}");

    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
    assert!(
        json["summary"]["must_review"]["functions"]
            .as_u64()
            .is_some_and(|count| count >= 27)
    );
    assert!(
        json["summary"]["low_semantic_risk"]["functions"]
            .as_u64()
            .is_some_and(|count| count >= 13)
    );
}

#[test]
fn parser_expands_native_module_declarations_to_native_functions() {
    let source = r#"
features: native

struct Path
resource File
struct IOError

native module File {
    fn open(path: read Path) -> Result<File, IOError>
}
"#;
    let diagnostics = analyze_source("native-module.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let map = review_map_sources(vec![("native-module.rss", source)]);
    assert!(map.files[0].regions.iter().any(|region| {
        region
            .reasons
            .iter()
            .any(|reason| reason == "native boundary")
    }));
}
