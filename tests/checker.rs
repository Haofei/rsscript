use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript::syntax::ast::{EffectDecl, Expr, Item};
use rsscript::syntax::parse_source;
use rsscript::{
    NativeRustDependency, ReviewMapClassification, ReviewMapFileRisk, ReviewRisk, analyze_source,
    analyze_source_with_core, analyze_source_with_interfaces, check_generated_rust_package,
    check_package_dir, core_interfaces, diff_package_dirs, diff_package_locks,
    explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_json,
    format_package_lock_toml, format_review_human, format_review_json, format_review_map_human,
    format_review_map_json, lint_source, lock_package_dir, lower_source_to_rust,
    lower_source_to_rust_package, lower_source_to_rust_with_map,
    lower_sources_to_rust_package_with_options, package_lowering_input, package_metadata,
    package_tree, parse_runtime_diagnostics, publish_package_dry_run, remap_rustc_diagnostic_json,
    remap_rustc_diagnostic_json_lines, review_map_sources, review_package_dir, review_sources,
    vendor_package_dir, write_generated_rust_package,
};
use serde_json::Value;

const REQUIRED_SPEC_DIAGNOSTICS: &[(&str, &str)] = &[
    ("use after manage", "RS0401"),
    ("managed -> local attempt", "RS0301"),
    ("missing named argument", "RS0204"),
    ("missing read/mut/take effect", "RS0202"),
    ("call argument type mismatch", "RS0207"),
    ("return type mismatch", "RS0208"),
    ("control-flow type mismatch", "RS0209"),
    ("operator type mismatch", "RS0210"),
    ("same-call place conflict", "RS0302"),
    ("constructor/variant call-like conflict", "RS0203"),
    ("handle-field same-call conflict", "RS0303"),
    ("retaining local value", "RS0501"),
    ("managed closure capturing local/resource", "RS0801"),
    (
        "managed closure capture retention in retained contexts",
        "RS0801",
    ),
    ("fresh function returning aliased value", "RS0601"),
    ("mut/take of unbound fresh expression", "RS0604"),
    ("resource escaping with", "RS0702"),
    ("resource wrapped in Ok/Some and escaping", "RS0702"),
    (
        "resource-producing expression used outside resource context",
        "RS0702",
    ),
    (
        "Result-returning resource producer missing explicit ?",
        "RS0706",
    ),
    (
        "invalid resource type in ordinary Result/Option/container context",
        "RS0704",
    ),
    ("ResourcePool factory contract violation", "RS0707"),
    ("ResourcePool max_size not a positive Int literal", "RS0708"),
    ("ResourcePool active lease conflict", "RS0709"),
    ("local captured by managed closure", "RS0801"),
    ("Fd used outside native/resource internals", "RS0023"),
    ("noescape callback escape", "RS0802"),
    ("local closure escape", "RS0803"),
    ("noescape closure consuming a captured local", "RS0804"),
    ("take of handle field", "RS0901"),
    (
        "weak field initialized without explicit weak handle",
        "RS0904",
    ),
    ("weak field used without explicit upgrade", "RS0903"),
    ("implicit conversion attempt", "RS1002"),
    ("operator overload attempt", "RS1001"),
    ("feature violation", "RS0101"),
    ("unsupported syntax", "RS0015"),
    (
        "async body / await / spawn used in v0.5 executable lowering",
        "RS0015",
    ),
    ("async call not consumed by await or spawn", "RS0022"),
    ("unmappable rustc diagnostic", "RS1102"),
    ("package feature resolution violation", "PKG0101"),
    ("package review policy violation", "PKG0501"),
    ("package native binding metadata violation", "PKG0601"),
];

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn pass_fixtures_have_no_diagnostics() {
    for path in fixture_paths("tests/fixtures/pass") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn fail_fixtures_report_expected_diagnostic_codes() {
    for path in fixture_paths("tests/fixtures/fail") {
        let source = read_fixture(&path);
        let expected = expected_codes(&source);
        let actual: Vec<String> = analyze_source(path.to_str().unwrap(), &source)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect();

        for code in expected {
            assert!(
                actual.contains(&code),
                "{} expected {code}, got {actual:?}",
                path.display()
            );
        }
    }
}

#[test]
fn required_spec_diagnostics_have_regression_coverage() {
    let fixture_codes = fail_fixture_expected_code_set();
    let dedicated_test_codes = BTreeSet::from([
        "RS1102",  // rustc_diagnostics_report_unmappable_generated_spans
        "RS1201",  // runtime_diagnostic_lines_parse_to_rsscript_diagnostics
        "PKG0101", // package feature resolution diagnostics
        "PKG0501", // package review policy diagnostics
        "PKG0601", // package native binding diagnostics
    ]);

    for &(spec_class, code) in REQUIRED_SPEC_DIAGNOSTICS {
        assert!(
            explain_diagnostic_code(code).is_some(),
            "{spec_class} maps to {code}, but the code has no explanation"
        );
        assert!(
            fixture_codes.contains(code) || dedicated_test_codes.contains(code),
            "{spec_class} maps to {code}, but no fail fixture or dedicated regression test covers it"
        );
    }
}

#[test]
fn core_interface_files_have_no_diagnostics() {
    for path in recursive_fixture_paths("core") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn examples_have_no_diagnostics_and_lower_to_runnable_packages() {
    for path in fixture_paths("examples") {
        let source = read_fixture(&path);
        let diagnostics = analyze_source_with_core(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());

        let package = lower_source_to_rust_package(
            path.to_str().unwrap(),
            &source,
            path.file_stem().and_then(|stem| stem.to_str()).unwrap(),
            "/workspace/rsscript/runtime",
        )
        .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", path.display()));

        assert!(
            package.main_rs.is_some(),
            "{} should lower to a runnable Rust package",
            path.display()
        );
        assert!(
            !package.lib_rs.contains("todo!"),
            "{} generated library should not contain todo! fallbacks",
            path.display()
        );
        if let Some(main_rs) = &package.main_rs {
            assert!(
                !main_rs.contains("todo!"),
                "{} generated main should not contain todo! fallbacks",
                path.display()
            );
        }
    }
}

#[test]
fn checked_in_generated_fixtures_have_no_executable_fallbacks() {
    for path in recursive_paths_with_extension("tests/generated", "rs") {
        let source = read_fixture(&path);
        assert!(
            !source.contains("todo!"),
            "{} should not contain generated todo! fallbacks",
            path.display()
        );
        assert!(
            !source.contains("RSScript declaration has no generated implementation"),
            "{} should not contain declaration panic fallbacks",
            path.display()
        );
    }
}

#[test]
fn bundled_core_interfaces_are_available_to_checker() {
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/test/assert.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/log/log.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/cache/cache.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/collections/buffer.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/collections/list.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/os/os.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/cache/image_cache.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/counter/counter.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/config/rules.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/interpreter/interpreter.rssi")
    );
    assert!(
        core_interfaces()
            .iter()
            .any(|(path, _)| *path == "core/weak/weak.rssi")
    );

    let source = r#"
fn check_label(actual: read String, expected: read String) -> Unit {
    Assert.equal(left: read actual, right: read expected)
    Log.write(message: read actual)
}
"#;

    assert_eq!(
        analyze_source_with_core("assert-use.rss", source),
        Vec::new()
    );
}

#[test]
fn bundled_core_interfaces_report_call_contract_errors() {
    let source = r#"
fn check_label(actual: read String, expected: read String) -> Unit {
    Assert.equal(value: read actual, right: read expected)
}
"#;
    let codes = analyze_source_with_core("assert-use.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0203".to_string()));
    assert!(codes.contains(&"RS0204".to_string()));
}

#[test]
fn bundled_interpreter_function_object_new_does_not_retain_closure() {
    let source = r#"
features: local

fn build() -> Unit {
    local env = Environment.root()
    let function = FunctionObject.new(closure: read env)
    return Unit
}
"#;

    assert_eq!(
        analyze_source_with_core("interpreter-weak.rss", source),
        Vec::new()
    );
}

#[test]
fn take_with_resource_reports_resource_escape_not_plain_take_error() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn consume_file(file: take File) -> Unit {
}

fn bad_take(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        consume_file(file: take file)
    }
}
"#;
    let codes = analyze_source("resource-take.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0702".to_string()));
    assert!(!codes.contains(&"RS0308".to_string()));
}

#[test]
fn managed_closure_capture_makes_fresh_local_unclean() {
    let source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn bad_fresh(path: read Path) -> fresh Image {
    local image = Image.load(path: read path)

    let callback = || {
        Image.inspect(image: read image)
    }

    return image
}
"#;
    let codes = analyze_source("fresh-closure-capture.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0801".to_string()));
    assert!(codes.contains(&"RS0601".to_string()));
}

#[test]
fn checker_rejects_resource_capture_in_wrapped_managed_closure() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn File.open(path: read Path) -> File
fn File.read_all(file: mut File) -> String

fn bad_capture(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        let callback = Some(|| {
            File.read_all(file: mut file)
        })
    }
}
"#;
    let diagnostics = analyze_source("resource-closure-wrapper.rss", source);

    assert!(
        diagnostics.iter().any(
            |diagnostic| diagnostic.code == "RS0702" && diagnostic.label == "resource captured"
        ),
        "{diagnostics:?}"
    );
}

#[test]
fn retained_closure_capture_makes_fresh_local_unclean() {
    let source = r#"
features: local

class Scheduler {
    callbacks: List<Callback>
}

struct Image {
    pixels: Buffer
}

fn schedule(scheduler: mut Scheduler, callback: read Callback) -> Unit
    effects(retains(callback))
{
}

fn bad_schedule(scheduler: mut Scheduler, path: read Path) -> fresh Image {
    local image = Image.load(path: read path)
    schedule(
        scheduler: mut scheduler,
        callback: read || {
            Image.inspect(image: read image)
        },
    )
    return image
}
"#;
    let codes = analyze_source("fresh-retained-closure-capture.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0801".to_string()));
    assert!(codes.contains(&"RS0601".to_string()));
}

#[test]
fn checker_rejects_retained_wrapped_closure_capturing_local() {
    let source = r#"
features: local

struct Image
struct CallbackOption
class Scheduler

fn Image.load(path: read Path) -> fresh Image
fn Image.inspect(image: read Image) -> Unit
fn schedule(scheduler: mut Scheduler, callback: read CallbackOption) -> Unit
    effects(retains(callback))

fn bad_schedule(scheduler: mut Scheduler, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    schedule(
        scheduler: mut scheduler,
        callback: read Some(|| {
            Image.inspect(image: read image)
        }),
    )
    return Unit
}
"#;
    let diagnostics = analyze_source("retained-closure-wrapper.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0801"
                && diagnostic.label == "local captured here"),
        "{diagnostics:?}"
    );
}

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

    assert!(codes.contains(&"RS0602".to_string()));
}

#[test]
fn checker_materializes_direct_fresh_read_but_rejects_mut_and_take() {
    let source = r#"
features: local

struct Image {
    width: Int
}

fn Image.load(path: read Path) -> fresh Image
fn inspect(image: read Image) -> Unit
fn resize(image: mut Image) -> Unit
fn consume(image: take Image) -> Unit

fn ok_read(path: read Path) -> Unit {
    inspect(image: read Image.load(path: read path))
}

fn bad_mut(path: read Path) -> Unit {
    resize(image: mut Image.load(path: read path))
}

fn bad_take(path: read Path) -> Unit {
    consume(image: take Image.load(path: read path))
}
"#;
    let diagnostics = analyze_source("fresh-materialization.rss", source);
    let fresh_materialization_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0604")
        .count();

    assert_eq!(fresh_materialization_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_requires_constructor_field_effects_for_handle_and_local_inline_fields() {
    let source = r#"
features: local

struct Buffer
struct Rules

struct Config {
    rules: handle Rules
    workspace: Buffer
}

fn Buffer.new() -> fresh Buffer
fn Rules.new() -> fresh Rules

fn bad_config() -> fresh Config {
    let rules = Rules.new()
    local workspace = Buffer.new()

    return Config(
        rules: rules,
        workspace: workspace,
    )
}
"#;
    let diagnostics = analyze_source("constructor-field-effects.rss", source);
    let constructor_effect_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0202" && diagnostic.label == "missing constructor field effect"
        })
        .count();

    assert_eq!(constructor_effect_count, 2, "{diagnostics:?}");
}

#[test]
fn resource_pool_read_parameter_must_be_local_capability() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        Db.close(fd: fd)
    }
}

fn bad_pool(pool: read ResourcePool<DbConnection>) -> Unit {
    DbConnection.count(pool: read pool)
}
"#;
    let codes = analyze_source("resourcepool-read-param.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0705".to_string()));
}

#[test]
fn diagnostics_json_uses_protocol_shape() {
    let path = Path::new("tests/fixtures/fail/use-after-manage.rss");
    let source = read_fixture(path);
    let diagnostics = analyze_source(path.to_str().unwrap(), &source);
    let json = format_diagnostics_json(&diagnostics);
    let value: Value = serde_json::from_str(&json).expect("diagnostics JSON should parse");
    let first = value
        .as_array()
        .and_then(|items| items.first())
        .expect("expected at least one diagnostic");

    assert_eq!(first["code"], "RS0401");
    assert_eq!(first["severity"], "error");
    assert!(
        first["summary"]
            .as_str()
            .is_some_and(|summary| !summary.is_empty())
    );
    assert!(
        first["spans"]
            .as_array()
            .is_some_and(|spans| !spans.is_empty())
    );
    assert!(first["causes"].is_array());
    assert!(first["fixes"].is_array());
}

#[test]
fn diagnostic_explanations_are_available_by_code() {
    let explanation = explain_diagnostic_code("RS0401").expect("RS0401 should be registered");
    let formatted = format_diagnostic_explanation(explanation);
    let fresh_unknown = explain_diagnostic_code("RS0602").expect("RS0602 should be registered");
    let pool_contract = explain_diagnostic_code("RS0707").expect("RS0707 should be registered");
    let unknown_type = explain_diagnostic_code("RS0024").expect("RS0024 should be registered");
    let unknown_field = explain_diagnostic_code("RS0025").expect("RS0025 should be registered");
    let unknown_binding = explain_diagnostic_code("RS0026").expect("RS0026 should be registered");
    let type_mismatch = explain_diagnostic_code("RS0207").expect("RS0207 should be registered");
    let return_mismatch = explain_diagnostic_code("RS0208").expect("RS0208 should be registered");
    let control_flow_mismatch =
        explain_diagnostic_code("RS0209").expect("RS0209 should be registered");
    let operator_mismatch = explain_diagnostic_code("RS0210").expect("RS0210 should be registered");

    assert_eq!(explanation.title, "use after manage");
    assert!(formatted.contains("RS0401"));
    assert!(formatted.contains("manage"));
    assert!(fresh_unknown.explanation.contains("clean inline fields"));
    assert_eq!(unknown_type.title, "unknown type");
    assert!(unknown_type.explanation.contains("before Rust lowering"));
    assert_eq!(unknown_field.title, "unknown field");
    assert!(unknown_field.explanation.contains("deferred"));
    assert_eq!(unknown_binding.title, "unknown binding");
    assert!(unknown_binding.explanation.contains("visible parameter"));
    assert_eq!(type_mismatch.title, "argument type mismatch");
    assert!(
        type_mismatch
            .explanation
            .contains("resolved parameter type")
    );
    assert_eq!(return_mismatch.title, "return type mismatch");
    assert!(return_mismatch.explanation.contains("declared return type"));
    assert_eq!(control_flow_mismatch.title, "control-flow type mismatch");
    assert!(
        control_flow_mismatch
            .explanation
            .contains("conditions must be `Bool`")
    );
    assert_eq!(operator_mismatch.title, "operator type mismatch");
    assert!(operator_mismatch.explanation.contains("Equality requires"));
    assert_eq!(
        pool_contract.title,
        "ResourcePool factory contract violation"
    );
    assert!(pool_contract.explanation.contains("ResourcePool.try_new"));
    assert!(explain_diagnostic_code("RS9999").is_none());
}

#[test]
fn checker_reports_call_argument_type_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read 42)
    return Unit
}
"#;
    let diagnostics = analyze_source_with_core("arg-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `message` for `Log.write` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_constructor_field_type_mismatch_before_backend_lowering() {
    let source = r#"
struct User {
    name: String
}

fn build() -> User {
    return User(name: read 42)
}
"#;
    let diagnostics = analyze_source("constructor-field-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `name` for `User` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_call_argument_type_mismatch_before_rustc() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read 42)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("arg-type.rss", source)
        .expect_err("argument type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_accepts_none_for_option_arguments() {
    let source = r#"
fn accept(value: read Option<Int>) -> Unit
fn main() -> Unit {
    accept(value: read None)
    return Unit
}
"#;
    let diagnostics = analyze_source("none-arg.rss", source);

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_argument_payload_type_mismatch() {
    let source = r#"
fn accept(value: read Option<String>) -> Unit
fn main() -> Unit {
    accept(value: read Some(42))
    return Unit
}
"#;
    let diagnostics = analyze_source("option-arg-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `value` for `accept` has payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_argument_payload_type_mismatch() {
    let source = r#"
class BuildError {
    code: Int
}

fn accept(value: read Result<String, BuildError>) -> Unit
fn main() -> Unit {
    accept(value: read Err("bad"))
    return Unit
}
"#;
    let diagnostics = analyze_source("result-arg-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `value` for `accept` has payload type `String`, expected `BuildError`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_return_type_mismatch_before_backend_lowering() {
    let source = r#"
fn build() -> String {
    return 42
}
"#;
    let diagnostics = analyze_source("return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary == "return in `build` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_return_type_mismatch_before_rustc() {
    let source = r#"
fn build() -> String {
    return 42
}
"#;
    let diagnostics = lower_source_to_rust("return-type.rss", source)
        .expect_err("return type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_ok_payload_type_mismatch() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<String, BuildError> {
    return Ok(42)
}
"#;
    let diagnostics = analyze_source("result-ok-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary == "Ok payload in `build` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_err_payload_type_mismatch() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<String, BuildError> {
    return Err("bad")
}
"#;
    let diagnostics = analyze_source("result-err-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "Err payload in `build` has type `String`, expected `BuildError`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_some_payload_type_mismatch() {
    let source = r#"
fn maybe_name() -> Option<String> {
    return Some(42)
}
"#;
    let diagnostics = analyze_source("option-some-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "Some payload in `maybe_name` has type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_bare_option_some_return_before_backend_lowering() {
    let source = r#"
fn maybe_name() -> Option<String> {
    return "ok"
}
"#;
    let diagnostics = analyze_source("option-bare-return.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "return in `maybe_name` has type `String`, expected `Option<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_non_bool_if_condition_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if "yes" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("if-condition-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary == "if condition has type `String`, expected `Bool`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_non_bool_if_condition_before_rustc() {
    let source = r#"
fn main() -> Unit {
    if "yes" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("if-condition-type.rss", source)
        .expect_err("if condition type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0209"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_non_option_result_match_scrutinee_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let value = "yes"
    match value {
        Some(result) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-scrutinee-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary
                    == "match scrutinee has type `String`, expected `Option<T>` or `Result<T, E>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_non_option_result_match_scrutinee_before_rustc() {
    let source = r#"
fn main() -> Unit {
    let value = "yes"
    match value {
        Some(result) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("match-scrutinee-type.rss", source)
        .expect_err("match scrutinee type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0209"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_option_match_result_variants_before_backend_lowering() {
    let source = r#"
fn maybe() -> Option<String> {
    return Some("x")
}

fn main() -> Unit {
    let value = maybe()
    match value {
        Ok(result) => return Unit
        Err(error) => return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-variant-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary
                    == "match pattern `Ok` cannot match scrutinee type `Option<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_match_variant_mismatch_before_rustc() {
    let source = r#"
fn maybe() -> Result<String, BuildError> {
    return Ok("x")
}

fn main() -> Unit {
    let value = maybe()
    match value {
        Some(result) => return Unit
        None => return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("match-variant-type.rss", source)
        .expect_err("match variant mismatch should fail before Rust generation");

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0209"
                && diagnostic.summary
                    == "match pattern `Some` cannot match scrutinee type `Result<String, BuildError>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_mixed_equality_operand_types_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if 1 == "1" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("operator-equality-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `==` has operands `Int` and `String`, expected matching operand types."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_mixed_equality_operand_types_before_rustc() {
    let source = r#"
fn main() -> Unit {
    if 1 == "1" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("operator-equality-type.rss", source)
        .expect_err("operator type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0210"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_ordering_operand_types_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if "a" > 1 {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("operator-ordering-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `>` has operands `String` and `Int`, expected numeric operands."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_logical_operand_types_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    if true && "yes" {
        return Unit
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("operator-logical-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0210"
                && diagnostic.summary
                    == "operator `&&` has operands `Bool` and `String`, expected Bool operands."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_bare_option_some_return_before_rustc() {
    let source = r#"
fn maybe_name() -> Option<String> {
    return "ok"
}
"#;
    let diagnostics = lower_source_to_rust("option-bare-return.rss", source)
        .expect_err("bare Option success return should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_allows_bare_result_success_returns_to_match_ok_type() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<String, BuildError> {
    return "ok"
}
"#;
    let diagnostics = analyze_source("result-bare-success.rss", source);

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_unknown_value_bindings_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read missing)
    return Unit
}
"#;
    let diagnostics = analyze_source_with_core("unknown-binding.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0026" && diagnostic.summary == "unknown value binding `missing`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_unknown_bindings_before_rustc() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read missing)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("unknown-binding.rss", source)
        .expect_err("unknown binding should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0026"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_use_before_local_binding_declaration() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read later)
    let later = "ready"
    return Unit
}
"#;
    let diagnostics = analyze_source_with_core("use-before-let.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0026"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_unknown_fields_before_backend_lowering() {
    let source = r#"
struct User {
    id: Int
}

fn main() -> Unit {
    let user = User(id: 1)
    Assert.equal_int(left: user.missing, right: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("unknown-field.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0025"
                && diagnostic.summary == "unknown field `missing` on type `User`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_unknown_fields_before_rustc() {
    let source = r#"
struct User {
    id: Int
}

fn main() -> Unit {
    let user = User(id: 1)
    Assert.equal_int(left: user.missing, right: 1)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("unknown-field.rss", source)
        .expect_err("unknown field should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0025"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_unknown_types_before_backend_lowering() {
    let source = r#"
fn bad(value: read MissingType) -> Result<Unit, MissingError> {
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("unknown-type.rss", source);
    let unknown_types = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0024")
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        unknown_types,
        vec![
            "unknown type `MissingType`.",
            "unknown type `MissingError`."
        ],
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_unknown_types_before_rustc() {
    let source = r#"
fn bad(value: read MissingType) -> Unit {
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("unknown-type.rss", source)
        .expect_err("unknown type should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0024"),
        "{diagnostics:?}"
    );
}

#[test]
fn lint_warns_on_public_signature_complexity() {
    let source = r#"
features: native

pub fn overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Image>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Image>>>, Error>
    effects(no_panic, noalloc, no_block, pure, native)
{
    return Ok(None)
}
"#;
    let diagnostics = lint_source("lint.rss", source);
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RSL001"));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.severity.is_error())
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("7 parameters"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("4 generic parameters"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("5 effects"))
    );
}

#[test]
fn lint_warns_on_duplicate_effects() {
    let source = r#"
fn cache(value: read Image) -> Unit
    effects(no_panic, no_panic, retains(value), retains(value))
{
    return Unit
}
"#;
    let diagnostics = lint_source("lint.rss", source);
    let duplicate_effects = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RSL002")
        .collect::<Vec<_>>();

    assert_eq!(duplicate_effects.len(), 2);
    assert!(
        duplicate_effects
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("no_panic"))
    );
    assert!(
        duplicate_effects
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("retains(value)"))
    );
}

#[test]
fn parser_accepts_qualified_interface_function_signatures() {
    let source = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let program = parse_source("html.rssi", source);

    assert!(
        matches!(&program.items[1], Item::Function(function) if function.name == "Html.escape")
    );
}

#[test]
fn checker_resolves_calls_from_interface_signatures() {
    let interface = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let source = r#"
fn render(body: read String) -> fresh HtmlEscaped {
    return Html.escape(text: read body)
}
"#;

    assert_eq!(
        analyze_source_with_interfaces("page.rss", source, &[("html.rssi", interface)]),
        Vec::new()
    );
}

#[test]
fn calling_unsafe_function_requires_features_unsafe() {
    let interface = r#"
features: unsafe
pub fn Crypto.raw_copy(dst: mut Buffer, src: read Buffer) -> Unit
    effects(unsafe)
"#;
    let source = r#"
fn looks_safe(dst: mut Buffer, src: read Buffer) -> Unit {
    Crypto.raw_copy(dst: mut dst, src: read src)
    return Unit
}
"#;
    let codes = analyze_source_with_interfaces("caller.rss", source, &[("crypto.rssi", interface)])
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0101".to_string()),
        "calling an unsafe function from a file without `features: unsafe` should be rejected, got {codes:?}"
    );
}

#[test]
fn calling_unsafe_function_is_allowed_under_features_unsafe() {
    let interface = r#"
features: unsafe
pub fn Crypto.raw_copy(dst: mut Buffer, src: read Buffer) -> Unit
    effects(unsafe)
"#;
    let source = r#"
features: unsafe

fn wrapper(dst: mut Buffer, src: read Buffer) -> Unit {
    Crypto.raw_copy(dst: mut dst, src: read src)
    return Unit
}
"#;
    assert_eq!(
        analyze_source_with_interfaces("caller.rss", source, &[("crypto.rssi", interface)]),
        Vec::new()
    );
}

#[test]
fn checker_reports_interface_signature_call_violations() {
    let interface = r#"
struct HtmlEscaped

pub fn Html.escape(text: read String) -> fresh HtmlEscaped
"#;
    let source = r#"
fn render(body: read String) -> fresh HtmlEscaped {
    return Html.escape(value: read body)
}
"#;
    let codes = analyze_source_with_interfaces("page.rss", source, &[("html.rssi", interface)])
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0203".to_string()));
    assert!(codes.contains(&"RS0204".to_string()));
}

#[test]
fn rust_lowering_emits_checked_rust_source() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

pub fn make_point(x: Int, y: Int) -> fresh Point {
    return Point(x: x, y: y)
}
"#;
    let rust = lower_source_to_rust("point.rss", source).expect("source should lower");

    assert!(rust.contains("// Generated by RSScript."));
    assert!(rust.contains("// rss:span kind=function file=point.rss"));
    assert!(rust.contains("pub struct Point"));
    assert!(rust.contains("pub x: i64"));
    assert!(rust.contains("pub fn make_point(x: i64, y: i64) -> Point"));
    assert!(rust.contains("return Point { x: x, y: y };"));
}

#[test]
fn rust_lowering_maps_unit_and_result_constructors_to_rust() {
    let source = r#"
struct BuildError {
    code: Int
}

struct Point {
    x: Int
    y: Int
}

pub fn make_result(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    return Ok(Point(x: x, y: y))
}

pub fn fail(code: Int) -> Result<fresh Point, BuildError> {
    return Err(BuildError(code: code))
}

pub fn unit_result() -> Result<Unit, BuildError> {
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("result.rss", source).expect("source should lower");

    assert!(rust.contains("pub fn make_result(x: i64, y: i64) -> Result<Point, BuildError>"));
    assert!(rust.contains("return Ok(Point { x: x, y: y });"));
    assert!(rust.contains("return Err(BuildError { code: code });"));
    assert!(rust.contains("pub fn unit_result() -> Result<(), BuildError>"));
    assert!(rust.contains("return Ok(());"));
}

#[test]
fn rust_lowering_maps_option_type_and_constructors_to_rust() {
    let source = r#"
pub fn maybe_value(flag: Bool) -> Option<Int> {
    if flag {
        return Some(42)
    }
    return None
}
"#;
    let rust = lower_source_to_rust("option.rss", source).expect("source should lower");

    assert!(rust.contains("pub fn maybe_value(flag: bool) -> Option<i64>"));
    assert!(rust.contains("return Some(42);"));
    assert!(rust.contains("return None;"));
}

#[test]
fn rust_lowering_maps_core_surface_types_to_rust_std_types() {
    let source = r#"
struct CoreError {
    code: Int
}

pub fn inspect_core(
    path: read Path,
    bytes: read Bytes,
    buffer: mut Buffer,
    names: read Set<String>,
    counts: read Map<String, Int>,
    items: read List<Int>,
) -> Result<fresh Bytes, CoreError> {
    return Err(CoreError(code: 0))
}
"#;
    let rust = lower_source_to_rust("core-types.rss", source).expect("source should lower");

    assert!(rust.contains("path: &std::path::PathBuf"));
    assert!(rust.contains("bytes: &Vec<u8>"));
    assert!(rust.contains("buffer: &mut Vec<u8>"));
    assert!(rust.contains("names: &std::collections::HashSet<String>"));
    assert!(rust.contains("counts: &std::collections::HashMap<String, i64>"));
    assert!(rust.contains("items: &Vec<i64>"));
    assert!(rust.contains("-> Result<Vec<u8>, CoreError>"));
}

#[test]
fn rust_lowering_maps_take_consume_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn consume_list(list: take List<Int>) -> Unit {
    List.consume(list: take list)
}

fn consume_buffer(buffer: take Buffer) -> Unit {
    Buffer.consume(buffer: take buffer)
}
"#;
    let rust = lower_source_to_rust("consume.rss", source).expect("source should lower");

    assert!(rust.contains("fn consume_list(list: Vec<i64>)"));
    assert!(rust.contains("rsscript_runtime::list_consume(list);"));
    assert!(rust.contains("fn consume_buffer(buffer: Vec<u8>)"));
    assert!(rust.contains("rsscript_runtime::buffer_consume(buffer);"));
}

#[test]
fn rust_lowering_maps_cache_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let cache = Cache.new()
    Cache.insert(cache: mut cache, key: read "/users", value: read "handled /users")
    let body = Cache.lookup(cache: read cache, key: read "/users")
    Log.write(message: read body)
    return Unit
}
"#;
    let rust = lower_source_to_rust("cache.rss", source).expect("source should lower");

    assert!(rust.contains("let mut cache = rsscript_runtime::cache_new();"));
    assert!(rust.contains(
        "rsscript_runtime::cache_insert(&mut cache, &\"/users\".to_string(), &\"handled /users\".to_string());"
    ));
    assert!(
        rust.contains(
            "let body = rsscript_runtime::cache_lookup(&cache, &\"/users\".to_string());"
        )
    );
}

#[test]
fn rust_lowering_maps_list_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let list = List<Int>.new()
    List.push(list: mut list, value: read 10)
    let count = List.len(list: read list)
    let first = List.get(list: read list, index: 0)
    Assert.equal_int(left: count, right: 1)
    Assert.equal_int(left: first, right: 10)
    return Unit
}
"#;
    let rust = lower_source_to_rust("list.rss", source).expect("source should lower");

    assert!(rust.contains("let mut list = rsscript_runtime::list_new();"));
    assert!(rust.contains("rsscript_runtime::list_push(&mut list, &10);"));
    assert!(rust.contains("let count = rsscript_runtime::list_len(&list);"));
    assert!(rust.contains("let first = rsscript_runtime::list_get(&list, 0);"));
}

#[test]
fn rust_lowering_maps_file_core_calls_to_runtime_hooks() {
    let source = r#"
fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    with File.open_read(path: read input) as reader {
        with File.open_write(path: read output) as writer {
            let bytes = File.read_all(file: mut reader)?
            let text = File.read_all_string(file: mut reader)?
            File.write(file: mut writer, data: read bytes)?
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("file.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), std::io::Error>"));
    assert!(rust.contains("let mut reader = rsscript_runtime::file_open_read(&input)?;"));
    assert!(rust.contains("let mut writer = rsscript_runtime::file_open_write(&output)?;"));
    assert!(rust.contains("let bytes = rsscript_runtime::file_read_all(&mut reader)?;"));
    assert!(rust.contains("let text = rsscript_runtime::file_read_all_string(&mut reader)?;"));
    assert!(rust.contains("rsscript_runtime::file_write(&mut writer, &bytes)?;"));
}

#[test]
fn rust_lowering_maps_path_construction_to_runtime_hook() {
    let source = r#"
fn main() -> Result<Unit, FileError> {
    let path = Path.from_string(value: read "rsscript-path.txt")
    let data = Bytes.from_string(value: read "path hook ran")
    with File.open_write(path: read path) as file {
        File.write(file: mut file, data: read data)?
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("path.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let path = rsscript_runtime::path_from_string(&\"rsscript-path.txt\".to_string());"
    ));
    assert!(rust.contains("let mut file = rsscript_runtime::file_open_write(&path)?;"));
}

#[test]
fn rust_lowering_maps_file_read_into_and_buffer_reuse_to_runtime_hooks() {
    let source = r#"
features: local

fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    local buffer = Buffer.new(size: 8192)
    with File.open_read(path: read input) as reader {
        with File.open_write(path: read output) as writer {
            while File.read_into(file: mut reader, buffer: mut buffer)? {
                File.write_buffer(file: mut writer, buffer: read buffer)?
                Buffer.clear(buffer: mut buffer)
            }
        }
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("file-buffer.rss", source).expect("source should lower");

    assert!(rust.contains("let mut buffer = rsscript_runtime::buffer_new(8192);"));
    assert!(rust.contains("while rsscript_runtime::file_read_into(&mut reader, &mut buffer)? {"));
    assert!(rust.contains("rsscript_runtime::file_write_buffer(&mut writer, &buffer)?;"));
    assert!(rust.contains("rsscript_runtime::buffer_clear(&mut buffer);"));
}

#[test]
fn rust_lowering_maps_json_core_calls_to_runtime_hooks() {
    let source = r#"
fn read_name(text: read String) -> Result<String, JsonError> {
    let value = Json.parse(text: read text)?
    let path = Path.from_string(value: read "profile.json")
    let value_from_file = Json.parse_file(path: read path)?
    let count = Json.array_len(value: read value)?
    let first = Json.array_get(value: read value, index: 0)?
    let has_profile = Json.array_contains_string(value: read value, item: read "profile")?
    let has_profile_prefix = Json.array_contains_substring(value: read value, text: read "prof")?
    let has_profile_named_prefix = Json.array_contains_prefix(value: read value, prefix: read "pro")?
    let profile = Json.field(value: read value, name: read "profile")?
    let profile_name_value = Json.field(value: read profile, name: read "name")?
    let profile_name = Json.as_string(value: read profile_name_value)?
    let active = Json.field_bool(value: read profile, name: read "active")?
    let age = Json.field_int(value: read profile, name: read "age")?
    return profile_name
}
"#;
    let rust = lower_source_to_rust("json.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<String, rsscript_runtime::JsonError>"));
    assert!(rust.contains("let value = rsscript_runtime::json_parse(&text)?;"));
    assert!(rust.contains("let value_from_file = rsscript_runtime::json_parse_file(&path)?;"));
    assert!(rust.contains(
        "let profile = rsscript_runtime::json_field(&value, &\"profile\".to_string())?;"
    ));
    assert!(rust.contains("let count = rsscript_runtime::json_array_len(&value)?;"));
    assert!(rust.contains("let first = rsscript_runtime::json_array_get(&value, 0)?;"));
    assert!(rust.contains(
        "let has_profile = rsscript_runtime::json_array_contains_string(&value, &\"profile\".to_string())?;"
    ));
    assert!(rust.contains(
        "let has_profile_prefix = rsscript_runtime::json_array_contains_substring(&value, &\"prof\".to_string())?;"
    ));
    assert!(rust.contains(
        "let has_profile_named_prefix = rsscript_runtime::json_array_contains_prefix(&value, &\"pro\".to_string())?;"
    ));
    assert!(rust.contains(
        "let active = rsscript_runtime::json_field_bool(&profile, &\"active\".to_string())?;"
    ));
    assert!(
        rust.contains("let profile_name = rsscript_runtime::json_as_string(&profile_name_value)?;")
    );
    assert!(
        rust.contains(
            "let age = rsscript_runtime::json_field_int(&profile, &\"age\".to_string())?;"
        )
    );
    assert!(rust.contains("return Ok(profile_name);"));
}

#[test]
fn rust_lowering_maps_csv_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn read_name(path: read Path) -> Result<String, CsvError> {
    local buffer = RowBuffer.new(size: 4096)
    with File.open_read(path: read path) as file {
        Csv.read_into(file: mut file, buffer: mut buffer)?
        let row = Csv.parse_row(buffer: read buffer)?
        return Row.field_string(row: read row, index: 0)
    }
}
"#;
    let rust = lower_source_to_rust("csv.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<String, rsscript_runtime::CsvError>"));
    assert!(rust.contains("let mut buffer = rsscript_runtime::row_buffer_new(4096);"));
    assert!(rust.contains("rsscript_runtime::csv_read_into(&mut file, &mut buffer)?;"));
    assert!(rust.contains("let row = rsscript_runtime::csv_parse_row(&buffer)?;"));
    assert!(rust.contains("return rsscript_runtime::row_field_string(&row, 0);"));
}

#[test]
fn rust_lowering_maps_image_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn process(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?
    Image.resize(image: mut image, width: 320, height: 240)
    Image.normalize(image: mut image)
    Image.sharpen(image: mut image)
    Image.inspect(image: read image)
    Image.save(image: read image, path: read output)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("image.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), rsscript_runtime::ImageError>"));
    assert!(rust.contains("let mut image = rsscript_runtime::image_load(&input)?;"));
    assert!(rust.contains("rsscript_runtime::image_resize(&mut image, 320, 240);"));
    assert!(rust.contains("rsscript_runtime::image_normalize(&mut image);"));
    assert!(rust.contains("rsscript_runtime::image_sharpen(&mut image);"));
    assert!(rust.contains("rsscript_runtime::image_inspect(&image);"));
    assert!(rust.contains("rsscript_runtime::image_save(&image, &output)?;"));
}

#[test]
fn rust_lowering_maps_image_cache_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn cache_image(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local cache = ImageCache.new(capacity: 1)
    local image = Image.load(path: read input)?
    let shared = manage image
    ImageCache.store(cache: mut cache, image: read shared)
    Image.save(image: read shared, path: read output)?
    let count = ImageCache.len(cache: read cache)
    if count == 1 {
        Log.write(message: read "cached")
    }
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("image-cache.rss", source).expect("source should lower");

    assert!(rust.contains("let mut cache = rsscript_runtime::image_cache_new(1);"));
    assert!(rust.contains("let shared = rsscript_runtime::manage_at(image,"));
    assert!(rust.contains("rsscript_runtime::image_cache_store(&mut cache, &shared);"));
    assert!(rust.contains("rsscript_runtime::image_save(&shared, &output)?;"));
    assert!(rust.contains("let count = rsscript_runtime::image_cache_len(&cache);"));
}

#[test]
fn rust_lowering_maps_http_handler_core_calls_to_runtime_hooks() {
    let source = r#"
fn handle_request(request: read Request) -> Result<fresh Response, HttpError> {
    let path = Request.path(request: read request)
    let body = String.concat(left: read "handled ", right: read path)
    return Response.ok(body: read body)
}

fn main() -> Result<Unit, HttpError> {
    let request = Request.new(path: read "/users")
    let response = handle_request(request: read request)?
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("http.rss", source).expect("source should lower");

    assert!(rust.contains("request: &rsscript_runtime::Request"));
    assert!(rust.contains("-> Result<rsscript_runtime::Response, rsscript_runtime::HttpError>"));
    assert!(rust.contains("let path = rsscript_runtime::request_path(&request);"));
    assert!(rust.contains("return rsscript_runtime::response_ok(&body);"));
    assert!(rust.contains("let request = rsscript_runtime::request_new(&\"/users\".to_string());"));
    assert!(rust.contains("let response = handle_request(&request)?;"));
}

#[test]
fn rust_lowering_maps_db_resource_pool_to_runtime_hooks() {
    let source = r#"
features: local

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 2,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("db.rss", source).expect("source should lower");

    assert!(rust.contains("url: &String"));
    assert!(rust.contains("-> Result<(), rsscript_runtime::DbError>"));
    assert!(rust.contains("let mut pool = rsscript_runtime::ResourcePool::from_factory(2, || rsscript_runtime::db_connection_open(&url));"));
    assert!(rust.contains("let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(&mut pool, rsscript_runtime::SourceSpan::new(\"db.rss\""));
    assert!(rust.contains("rsscript_runtime::db_connection_query(&mut conn, &sql)?;"));
}

#[test]
fn rust_lowering_maps_resource_pool_try_new_to_runtime_hooks() {
    let source = r#"
features: local

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.try_new(
        create: || DbConnection.try_open(url: read url),
        max_size: 2,
    )?

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("db-try-pool.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<(), rsscript_runtime::DbError>"));
    assert!(rust.contains("let mut pool = rsscript_runtime::ResourcePool::try_from_factory(2, || rsscript_runtime::db_connection_try_open(&url))?;"));
    assert!(rust.contains("rsscript_runtime::db_connection_query(&mut conn, &sql)?;"));
}

#[test]
fn rust_lowering_maps_config_reload_to_runtime_hooks() {
    let source = r#"
features: local

fn load_config(path: read Path) -> Result<fresh ConfigValue, ConfigError> {
    return Config.load(path: read path)
}

fn reload_config(path: read Path, store: mut ConfigStore) -> Result<Unit, ConfigError> {
    let next = load_config(path: read path)?
    ConfigStore.replace(store: mut store, value: read next)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("config.rss", source).expect("source should lower");

    assert!(
        rust.contains("-> Result<rsscript_runtime::ConfigValue, rsscript_runtime::ConfigError>")
    );
    assert!(rust.contains("return rsscript_runtime::config_load(&path);"));
    assert!(rust.contains("store: &mut rsscript_runtime::ConfigStore"));
    assert!(rust.contains("rsscript_runtime::config_store_replace(store, &next);"));
}

#[test]
fn rust_lowering_maps_rules_config_reload_to_runtime_hooks() {
    let source = r#"
fn load_rules_config(path: read Path) -> Result<fresh Config, ConfigError> {
    let rules = RuleLoader.load_rules(path: read path)?
    return Ok(Config.new(name: read "rules", rules: read rules))
}

fn reload_rules_config(path: read Path, global: mut GlobalConfig) -> Result<Unit, ConfigError> {
    let next = load_rules_config(path: read path)?
    GlobalConfig.replace(global: mut global, value: read next)
    return Ok(Unit)
}
"#;
    let rust = lower_source_to_rust("rules-config.rss", source).expect("source should lower");

    assert!(rust.contains("-> Result<rsscript_runtime::Config, rsscript_runtime::ConfigError>"));
    assert!(rust.contains("let rules = rsscript_runtime::rule_loader_load_rules(&path)?;"));
    assert!(
        rust.contains("return Ok(rsscript_runtime::config_new(&\"rules\".to_string(), &rules));")
    );
    assert!(rust.contains("global: &mut rsscript_runtime::GlobalConfig"));
    assert!(rust.contains("rsscript_runtime::global_config_replace(global, &next);"));
}

#[test]
fn rust_lowering_maps_counter_core_calls_to_runtime_hooks() {
    let source = r#"
fn main() -> Unit {
    let counter = Counter.new(value: 1)
    Counter.add(counter: mut counter, amount: 2)
    let value = Counter.value(counter: read counter)
    if value == 3 {
        Log.write(message: read "counter ran")
    }
    return Unit
}
"#;
    let rust = lower_source_to_rust("counter.rss", source).expect("source should lower");

    assert!(rust.contains("let mut counter = rsscript_runtime::counter_new(1);"));
    assert!(rust.contains("rsscript_runtime::counter_add(&mut counter, 2);"));
    assert!(rust.contains("let value = rsscript_runtime::counter_value(&counter);"));
    assert!(rust.contains("if value == 3 {"));
}

#[test]
fn rust_lowering_maps_interpreter_cycle_core_calls_to_runtime_hooks() {
    let source = r#"
features: local

fn main() -> Unit {
    local root_value = Environment.root()
    let root = manage root_value
    local child_value = Environment.child(parent: read root)
    let child = manage child_value
    local function_value = FunctionObject.new(closure: read child)
    let function = manage function_value
    Environment.bind_function(env: mut child, function: read function)
    if Environment.has_function(env: read child) && FunctionObject.has_closure(function: read function) {
        Log.write(message: read "linked")
    }
    return Unit
}
"#;
    let rust = lower_source_to_rust("interpreter-cycle.rss", source).expect("source should lower");

    assert!(rust.contains("let root_value = rsscript_runtime::environment_root();"));
    assert!(rust.contains("let root = rsscript_runtime::manage_at(root_value,"));
    assert!(rust.contains("let child_value = rsscript_runtime::environment_child(&root);"));
    assert!(rust.contains("let mut child = rsscript_runtime::manage_at(child_value,"));
    assert!(rust.contains("let function_value = rsscript_runtime::function_object_new(&child);"));
    assert!(rust.contains("let function = rsscript_runtime::manage_at(function_value,"));
    assert!(rust.contains("rsscript_runtime::environment_bind_function(&mut child, &function);"));
    assert!(rust.contains("rsscript_runtime::function_object_has_closure(&function)"));
}

#[test]
fn rust_lowering_decodes_string_escape_sequences() {
    let source = r#"
fn json_text() -> String {
    return "{\"profile\":{\"name\":\"RSScript\"}}"
}
"#;
    let rust = lower_source_to_rust("string-escapes.rss", source).expect("source should lower");

    assert!(
        rust.contains("return \"{\\\"profile\\\":{\\\"name\\\":\\\"RSScript\\\"}}\".to_string();")
    );
}

#[test]
fn rust_lowering_maps_log_write_to_runtime_output_hook() {
    let source = r#"
fn main() -> Unit {
    Log.write(message: read "hello RSScript")
    return Unit
}
"#;
    let rust = lower_source_to_rust("log.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::log_write(&\"hello RSScript\".to_string());"));
}

#[test]
fn rust_lowering_maps_assert_equal_to_runtime_hook() {
    let source = r#"
fn main() -> Unit {
    Assert.equal(left: read "rss", right: read "rss")
    Assert.equal_int(left: 42, right: 42)
    Assert.equal_bool(left: true, right: true)
    return Unit
}
"#;
    let rust = lower_source_to_rust("assert.rss", source).expect("source should lower");

    assert!(
        rust.contains(
            "rsscript_runtime::assert_equal(&\"rss\".to_string(), &\"rss\".to_string());"
        )
    );
    assert!(rust.contains("rsscript_runtime::assert_equal_int(42, 42);"));
    assert!(rust.contains("rsscript_runtime::assert_equal_bool(true, true);"));
}

#[test]
fn rust_lowering_maps_string_concat_to_rust_std_expression() {
    let source = r#"
fn main() -> Unit {
    let message = String.concat(left: read "hello ", right: read "world")
    let count = String.from_int(value: 42)
    let ok = String.from_bool(value: true)
    let length = String.len(value: read message)
    let starts = String.starts_with(value: read message, prefix: read "hello")
    let ends = String.ends_with(value: read message, suffix: read "world")
    Log.write(message: read message)
    Log.write(message: read count)
    Log.write(message: read ok)
    return Unit
}
"#;
    let rust = lower_source_to_rust("string.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let message = format!(\"{}{}\", &\"hello \".to_string(), &\"world\".to_string());"
    ));
    assert!(rust.contains("let count = rsscript_runtime::string_from_int(42);"));
    assert!(rust.contains("let ok = rsscript_runtime::string_from_bool(true);"));
    assert!(rust.contains("let length = rsscript_runtime::string_len(&message);"));
    assert!(rust.contains(
        "let starts = rsscript_runtime::string_starts_with(&message, &\"hello\".to_string());"
    ));
    assert!(rust.contains(
        "let ends = rsscript_runtime::string_ends_with(&message, &\"world\".to_string());"
    ));
    assert!(rust.contains("rsscript_runtime::log_write(&message);"));
}

#[test]
fn rust_lowering_maps_int_add_to_rust_std_expression() {
    let source = r#"
fn main() -> Unit {
    let value = 20 + 22
    return Unit
}
"#;
    let rust = lower_source_to_rust("int.rss", source).expect("source should lower");

    assert!(rust.contains("let value = 20 + 22;"));
}

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

#[test]
fn rust_lowering_maps_builtin_operators_to_rust_expressions() {
    let source = r#"
fn main() -> Unit {
    let difference = 44 - 2
    let product = 6 * 7
    let quotient = product / 2
    let equal = product == 42
    let different = quotient != 0
    let less = quotient < product
    let less_equal = quotient <= product
    let greater = product > quotient
    let greater_equal = product >= quotient
    return Unit
}
"#;
    let rust = lower_source_to_rust("operators.rss", source).expect("source should lower");

    assert!(rust.contains("let difference = 44 - 2;"));
    assert!(rust.contains("let product = 6 * 7;"));
    assert!(rust.contains("let quotient = product / 2;"));
    assert!(rust.contains("let equal = product == 42;"));
    assert!(rust.contains("let different = quotient != 0;"));
    assert!(rust.contains("let less = quotient < product;"));
    assert!(rust.contains("let less_equal = quotient <= product;"));
    assert!(rust.contains("let greater = product > quotient;"));
    assert!(rust.contains("let greater_equal = product >= quotient;"));
}

#[test]
fn rust_lowering_maps_bool_literals_to_rust_literals() {
    let source = r#"
fn main() -> Unit {
    let enabled = true
    let disabled = false
    let changed = enabled != disabled
    return Unit
}
"#;
    let rust = lower_source_to_rust("bool.rss", source).expect("source should lower");

    assert!(rust.contains("let enabled = true;"));
    assert!(rust.contains("let disabled = false;"));
    assert!(rust.contains("let changed = enabled != disabled;"));
}

#[test]
fn rust_lowering_compares_read_string_params_to_literals() {
    let source = r#"
fn is_unknown(value: read String) -> Bool {
    return value == "unknown"
}
"#;
    let rust = lower_source_to_rust("string-compare.rss", source).expect("source should lower");

    assert!(rust.contains("return value.as_str() == \"unknown\";"));
}

#[test]
fn rust_lowering_maps_try_operator_to_rust_result_propagation() {
    let source = r#"
features: local

struct BuildError {
    code: Int
}

struct Point {
    x: Int
    y: Int
}

fn maybe_point(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    return Ok(Point(x: x, y: y))
}

fn shift(point: mut Point) -> Unit {
    return Unit
}

pub fn use_try(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    local point = maybe_point(x: x, y: y)?
    shift(point: mut point)
    return Ok(point)
}
"#;
    let lowered = lower_source_to_rust_with_map("try.rss", source).expect("source should lower");

    assert!(
        lowered
            .rust_source
            .contains("let mut point = maybe_point(x, y)?;")
    );
    assert!(lowered.rust_source.contains("shift(&mut point);"));
    assert!(lowered.rust_source.contains("return Ok(point);"));
    assert!(lowered.source_map.iter().any(|entry| entry.kind == "try"));
}

#[test]
fn rust_lowering_wraps_direct_result_success_returns() {
    let source = r#"
struct BuildError {
    code: Int
}

struct Point {
    x: Int
    y: Int
}

fn maybe_point(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    return Point(x: x, y: y)
}

fn already_wrapped(x: Int, y: Int) -> Result<fresh Point, BuildError> {
    return Ok(Point(x: x, y: y))
}
"#;
    let rust = lower_source_to_rust("result-success.rss", source).expect("source should lower");

    assert!(rust.contains("return Ok(Point { x: x, y: y });"));
    assert!(!rust.contains("return Ok(Ok("));
}

#[test]
fn rust_lowering_matches_golden_fixture() {
    let source = read_fixture(Path::new("tests/golden/lowering/simple.rss"));
    let expected_rust = read_fixture(Path::new("tests/golden/lowering/simple.rs"));
    let expected_source_map =
        read_fixture(Path::new("tests/golden/lowering/simple.source-map.txt"));
    let lowered =
        lower_source_to_rust_with_map("simple.rss", &source).expect("source should lower");

    assert_eq!(lowered.rust_source, expected_rust);
    assert_eq!(source_map_summary(&lowered.source_map), expected_source_map);
}

#[test]
fn rust_lowering_emits_machine_readable_source_map() {
    let source = r#"
features: local

struct Session {
    id: Int
}

fn save(session: read Session) -> Unit {
    return Unit
}

pub fn make_session(id: Int) -> Unit {
    local session = Session(id: id)
    save(session: read session)
    let managed = manage session
    return Unit
}
"#;
    let lowered =
        lower_source_to_rust_with_map("session.rss", source).expect("source should lower");
    let kinds: Vec<&str> = lowered
        .source_map
        .iter()
        .map(|entry| entry.kind.as_str())
        .collect();

    assert!(kinds.contains(&"function"));
    assert!(kinds.contains(&"statement"));
    assert!(kinds.contains(&"call"));
    assert!(kinds.contains(&"named_arg"));
    assert!(kinds.contains(&"manage"));
    assert!(kinds.contains(&"field"));
    assert!(lowered.source_map.iter().any(|entry| {
        entry.source.file == "session.rss"
            && entry.generated.file == "src/lib.rs"
            && entry.generated.line > 0
            && entry.generated.column > 0
    }));
}

#[test]
fn rust_lowering_maps_with_resource_drop_points() {
    let source = r#"
fn copy(path: read Path) -> Result<Unit, FileError> {
    with File.open_read(path: read path) as file {
        File.read_all(file: mut file)?
    }

    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust_with_map("resource.rss", source).expect("source should lower");

    assert!(
        lowered
            .rust_source
            .contains("    {\n        let mut file = rsscript_runtime::file_open_read(&path)?;")
    );
    assert!(lowered.rust_source.contains(
        "        rsscript_runtime::file_read_all(&mut file)?;\n        // rss:span kind=resource_drop"
    ));
    assert!(
        lowered
            .rust_source
            .contains("// rss:span kind=resource_drop file=resource.rss")
    );
    assert!(
        lowered
            .source_map
            .iter()
            .any(|entry| entry.kind == "resource_drop" && entry.source.file == "resource.rss")
    );
}

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
    let lowered = lower_source_to_rust_with_map("native.rss", source).expect("source should lower");
    let native_calls = lowered
        .source_map
        .iter()
        .filter(|entry| entry.kind == "native_call")
        .collect::<Vec<_>>();

    assert_eq!(native_calls.len(), 2);
    assert!(native_calls.iter().any(|entry| entry.source.line == 8));
    assert!(native_calls.iter().any(|entry| entry.source.line == 9));
}

#[test]
fn rustc_diagnostics_map_back_to_rsscript_source_spans() {
    let source = r#"
features: local

struct Session {
    id: Int
}

pub fn make_session(id: Int) -> Session {
    local session = Session(id: id)
    return manage session
}
"#;
    let lowered =
        lower_source_to_rust_with_map("session.rss", source).expect("source should lower");
    let rust_line = lowered
        .rust_source
        .lines()
        .position(|line| line.contains("return rsscript_runtime::manage_at(session,"))
        .map(|index| index + 1)
        .expect("generated Rust should contain manage return");
    let rustc_json = format!(
        r#"{{"message":"mismatched types","code":{{"code":"E0308","explanation":null}},"level":"error","spans":[{{"file_name":"src/lib.rs","line_start":{rust_line},"line_end":{rust_line},"column_start":12,"column_end":40,"is_primary":true}}]}}"#
    );

    let remapped = remap_rustc_diagnostic_json(&lowered.source_map, &rustc_json)
        .expect("rustc JSON should parse")
        .expect("error should produce a diagnostic");

    assert!(remapped.mapped);
    assert_eq!(remapped.diagnostic.code, "RS1101");
    assert_eq!(remapped.diagnostic.span.file, "session.rss");
    assert!(
        remapped
            .diagnostic
            .causes
            .iter()
            .any(|cause| cause.contains("rustc code: E0308"))
    );
}

#[test]
fn rustc_diagnostics_report_unmappable_generated_spans() {
    let rustc_json = r#"{"message":"cannot find value","code":{"code":"E0425","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":99,"line_end":99,"column_start":5,"column_end":10,"is_primary":true}]}"#;

    let remapped = remap_rustc_diagnostic_json(&[], rustc_json)
        .expect("rustc JSON should parse")
        .expect("error should produce a diagnostic");

    assert!(!remapped.mapped);
    assert_eq!(remapped.diagnostic.code, "RS1102");
    assert_eq!(remapped.diagnostic.span.file, "src/lib.rs");
}

#[test]
fn rustc_diagnostic_line_remap_ignores_non_diagnostic_messages() {
    let lines = r#"{"reason":"compiler-artifact","target":{"name":"generated"}}
{"reason":"compiler-message","message":{"message":"cannot find value","code":{"code":"E0425","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":99,"line_end":99,"column_start":5,"column_end":10,"is_primary":true}]}}"#;

    let remapped =
        remap_rustc_diagnostic_json_lines(&[], lines).expect("rustc JSON lines should parse");

    assert_eq!(remapped.len(), 1);
    assert_eq!(remapped[0].diagnostic.code, "RS1102");
}

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
fn rss_run_rejects_empty_resource_pool_before_runtime() {
    let temp_dir = unique_temp_dir("rsscript-runtime-diagnostic");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("empty_pool.rss");
    write_empty_resource_pool_fixture(&source_path);

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(
        stdout
            .contains("error[RS0708]: `ResourcePool.new` requires a positive literal `max_size`."),
        "{stdout}"
    );
    assert!(stdout.contains("empty_pool.rss"), "{stdout}");
    assert!(
        !stdout.contains("runtime error kind: resource_pool_empty"),
        "{stdout}"
    );
}

#[test]
fn rss_run_json_rejects_empty_resource_pool_before_runtime() {
    let temp_dir = unique_temp_dir("rsscript-runtime-diagnostic-json");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("empty_pool.rss");
    write_empty_resource_pool_fixture(&source_path);

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("--json")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(!output.status.success());
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json[0]["code"], "RS0708");
    assert_eq!(json[0]["severity"], "error");
    assert_eq!(
        json[0]["spans"][0]["file"],
        source_path.display().to_string()
    );
    assert!(
        !json[0]["causes"]
            .as_array()
            .expect("causes should be an array")
            .iter()
            .any(|cause| cause == "runtime error kind: resource_pool_empty")
    );
}

#[test]
fn rss_fmt_outputs_canonical_source() {
    let temp_dir = unique_temp_dir("rsscript-fmt-cli");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("messy.rss");
    fs::write(
        &source_path,
        r#"features:   local
fn   main( )->Unit{
local value=String.concat(left:read "hello",right:read " fmt")
Log.write(message:read value)
return Unit
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("fmt")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss fmt should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(
        stdout,
        r#"features: local

fn main() -> Unit {
    local value = String.concat(left: read "hello", right: read " fmt")
    Log.write(message: read value)
    return Unit
}
"#
    );
}

#[test]
fn rss_run_accepts_resource_pool_try_new() {
    let temp_dir = unique_temp_dir("rsscript-run-resource-pool-try-new");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("try_pool.rss");
    fs::write(
        &source_path,
        r#"features: local

fn run_query(url: read Url, sql: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.try_new(
        create: || DbConnection.try_open(url: read url),
        max_size: 2,
    )?

    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn, sql: read sql)?
    }

    return Ok(Unit)
}

fn main() -> Result<Unit, DbError> {
    let url = Url.from_string(value: read "db://local")
    run_query(url: read url, sql: read "select 1")?
    return Ok(Unit)
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(
        stdout.contains("db query on db://local: select 1"),
        "{stdout}"
    );
}

#[test]
fn rss_run_accepts_non_consuming_noescape_callback() {
    let temp_dir = unique_temp_dir("rsscript-run-noescape-fnmut");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("noescape_fnmut.rss");
    fs::write(
        &source_path,
        r#"features: local

fn apply_twice(callback: noescape Fn()) -> Unit {
    callback()
    callback()
    return Unit
}

fn main() -> Unit {
    local buffer = Buffer.new(size: 16)
    apply_twice(callback: || {
        Buffer.clear(buffer: mut buffer)
    })
    Log.write(message: read "noescape fnmut ran")
    return Unit
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(stdout, "noescape fnmut ran\n");
}

#[test]
fn rss_run_accepts_local_closure_that_mutates_captured_local() {
    let temp_dir = unique_temp_dir("rsscript-run-local-closure-fnmut");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_path = temp_dir.join("local_closure_fnmut.rss");
    fs::write(
        &source_path,
        r#"features: local

fn main() -> Unit {
    local buffer = Buffer.new(size: 16)
    local callback = || {
        Buffer.clear(buffer: mut buffer)
    }
    callback()
    Log.write(message: read "local closure fnmut ran")
    return Unit
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(stdout, "local closure fnmut ran\n");
}

#[test]
fn rss_run_accepts_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-run-package-cli");
    write_named_package_fixture(&temp_dir, "rss-run-package", "0.1.0", "", "");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    Log.write(message: read "hello package")
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(stdout.contains("hello package"), "{stdout}");
}

#[test]
fn rss_run_accepts_multi_file_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-run-multi-package-cli");
    write_named_package_fixture(
        &temp_dir,
        "rss-run-multi-package",
        "0.1.0",
        "",
        "pub fn package_message() -> fresh String\n",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/helper.rss"),
        r#"pub fn package_message() -> fresh String {
    return String.concat(left: read "hello", right: read " multi package")
}
"#,
    )
    .expect("helper source should be written");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    let message = package_message()
    Log.write(message: read message)
}
"#,
    )
    .expect("main source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(stdout.contains("hello multi package"), "{stdout}");
}

#[test]
fn rss_run_json_remaps_rustc_compile_errors() {
    let temp_dir = unique_temp_dir("rsscript-run-rustc-remap");
    write_named_package_fixture(
        &temp_dir,
        "rss-run-rustc-remap",
        "0.1.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello")
    Log.write(message: read message)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: String) -> String { message }\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(!output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json[0]["code"], "RS1101");
    assert!(
        json[0]["spans"][0]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("src/main.rss"))
    );
    assert!(
        json[0]["causes"]
            .as_array()
            .expect("causes should be an array")
            .iter()
            .any(|cause| cause
                .as_str()
                .is_some_and(|cause| cause.contains("rustc code: E0308")))
    );
}

#[test]
fn rss_verify_rust_json_accepts_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-verify-package-cli");
    write_named_package_fixture(&temp_dir, "rss-verify-package", "0.1.0", "", "");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    Log.write(message: read "verify package")
}
"#,
    )
    .expect("source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json, serde_json::json!([]));
}

#[test]
fn rss_verify_rust_accepts_package_native_wrapper_dependency() {
    let temp_dir = unique_temp_dir("rsscript-verify-native-package-cli");
    let out_dir = temp_dir.join("generated-rust");
    write_named_package_fixture(
        &temp_dir,
        "rss-verify-native-package",
        "0.1.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    Log.write(message: read "verify native package")
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg(&temp_dir)
        .arg("--out-dir")
        .arg(&out_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust package directory should execute");
    let generated_cargo_toml =
        fs::read_to_string(out_dir.join("Cargo.toml")).expect("generated Cargo.toml should exist");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json, serde_json::json!([]));
    assert!(generated_cargo_toml.contains("\"rss_json_native\" = { path = "));
}

#[test]
fn rss_verify_rust_lowers_native_binding_manifest_calls() {
    let temp_dir = unique_temp_dir("rsscript-verify-native-binding-package-cli");
    let out_dir = temp_dir.join("generated-rust");
    write_named_package_fixture(
        &temp_dir,
        "rss-verify-native-binding-package",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg(&temp_dir)
        .arg("--out-dir")
        .arg(&out_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust package directory should execute");
    let generated_lib_rs =
        fs::read_to_string(out_dir.join("src/lib.rs")).expect("generated lib.rs should exist");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json, serde_json::json!([]));
    assert!(generated_lib_rs.contains("rss_json_native::echo"));
}

#[test]
fn rust_lowering_targets_runtime_crate_hooks() {
    let source = r#"
features: local

resource TestConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn pooled(pool: mut ResourcePool<TestConnection>) -> Unit
"#;
    let rust = lower_source_to_rust("pool.rssi", source).expect("source should lower");

    assert!(rust.contains("impl rsscript_runtime::Resource for TestConnection"));
    assert!(rust.contains("impl Drop for TestConnection"));
    assert!(rust.contains("rsscript_runtime::os_close(self.fd);"));
    assert!(!rust.contains("pub fn pooled"));
    assert!(!rust.contains("RSScript declaration has no generated implementation"));
    assert!(!rust.contains("todo!"));
}

#[test]
fn rust_lowering_emits_source_spans_for_resource_pool_borrow() {
    let source = r#"
features: local

resource TestConnection {
    fd: Int
}

fn TestConnection.query(conn: mut TestConnection, sql: read String) -> Unit {
    return Unit
}

fn pooled(pool: mut ResourcePool<TestConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        TestConnection.query(conn: mut conn, sql: read "select 1")
    }
}
"#;
    let rust = lower_source_to_rust("pool.rss", source).expect("source should lower");

    assert!(rust.contains(
        "let mut conn = rsscript_runtime::unwrap_runtime(rsscript_runtime::ResourcePool::borrow_at(pool, rsscript_runtime::SourceSpan::new(\"pool.rss\""
    ));
}

#[test]
fn rust_lowering_mangles_qualified_user_functions() {
    let source = r#"
struct User {
    name: String
}

fn User.lookup(name: read String) -> fresh User {
    return User(name: read name)
}

fn render(name: read String) -> fresh User {
    return User.lookup(name: read name)
}
"#;
    let rust = lower_source_to_rust("qualified.rss", source).expect("source should lower");

    assert!(rust.contains("fn User_lookup(name: &String) -> User"));
    assert!(rust.contains("fn render(name: &String) -> User"));
    assert!(rust.contains("return User_lookup(&name);"));
    assert!(!rust.contains("fn User.lookup"));
    assert!(!rust.contains("User::lookup"));
}

#[test]
fn rust_lowering_wraps_managed_class_returns_in_managed_handle() {
    let source = r#"
class Session {
    id: Int
}

pub fn make_session(id: Int) -> Session {
    return Session(id: id)
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub struct Session"));
    assert!(rust.contains("pub fn make_session(id: i64) -> rsscript_runtime::Managed<Session>"));
    assert!(rust.contains("return rsscript_runtime::manage_at(Session { id: id }, rsscript_runtime::SourceSpan::new(\"session.rss\", 7, 12, 7));"));
}

#[test]
fn rust_lowering_wraps_handle_fields_once() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: User
    explicit_owner: handle User
}
"#;
    let rust = lower_source_to_rust("session.rss", source).expect("source should lower");

    assert!(rust.contains("pub owner: rsscript_runtime::Managed<User>"));
    assert!(rust.contains("pub explicit_owner: rsscript_runtime::Managed<User>"));
    assert!(!rust.contains("rsscript_runtime::Managed<rsscript_runtime::Managed<User>>"));
}

#[test]
fn rust_lowering_maps_weak_class_fields_to_runtime_weak_handles() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn make_session() -> Session {
    let user = User(id: 1)
    return Session(owner: Weak.from(value: read user))
}

fn make_session_from_param(user: read User) -> Session {
    return Session(owner: Weak.from(value: read user))
}
"#;
    let rust = lower_source_to_rust("weak-session.rss", source).expect("source should lower");

    assert!(rust.contains("pub owner: rsscript_runtime::WeakManaged<User>"));
    assert!(rust.contains("owner: rsscript_runtime::weak(&user)"));
    assert!(rust.contains("owner: rsscript_runtime::weak(user)"));

    let package = lower_source_to_rust_package(
        "weak-session.rss",
        source,
        "weak-session",
        &format!("{}/runtime", env!("CARGO_MANIFEST_DIR")),
    )
    .expect("source should lower into package");
    let temp_dir = unique_temp_dir("rsscript-weak-session");
    write_generated_rust_package(&temp_dir, &package).expect("generated package should be written");
    let check =
        check_generated_rust_package(&temp_dir).expect("generated package should be checked");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        check.success,
        "diagnostics={:?}\nstderr={}",
        check.diagnostics, check.stderr
    );
}

#[test]
fn rust_lowering_materializes_constructor_read_fields_as_owned_values() {
    let source = r#"
struct Inner {
    name: String
}

struct Outer {
    inner: Inner
    label: String
}

fn make_outer(name: read String, label: read String) -> fresh Outer {
    let inner = Inner(name: read name)
    return Outer(inner: read inner, label: read label)
}
"#;
    let rust = lower_source_to_rust("owned-constructor.rss", source).expect("source should lower");

    assert!(rust.contains("#[derive(Debug, Clone)]"));
    assert!(rust.contains("Inner { name: name.clone() }"));
    assert!(rust.contains("Outer { inner: inner.clone(), label: label.clone() }"));
    assert!(!rust.contains("name: &name"));
    assert!(!rust.contains("inner: &inner"));
}

#[test]
fn rust_lowering_maps_weak_upgrade_to_runtime_handle_upgrade() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn read_owner(session: read Session) -> Option<User> {
    return Weak.upgrade(value: read session.owner)
}
"#;
    let rust = lower_source_to_rust("weak-upgrade.rss", source).expect("source should lower");

    assert!(rust.contains("return session.owner.upgrade();"));
    assert!(!rust.contains("Weak_upgrade"));
}

#[test]
fn checker_requires_explicit_weak_handle_for_weak_field_initialization() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn make_session() -> Session {
    let user = User(id: 1)
    return Session(owner: read user)
}
"#;
    let diagnostics = analyze_source("weak-field-initializer.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0904"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_requires_weak_field_upgrade_before_read_or_mut_use() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn User.log(user: read User) -> Unit
fn User.rename(user: mut User) -> Unit

fn bad_read(session: read Session) -> Unit {
    User.log(user: read session.owner)
}

fn bad_mut(session: read Session) -> Unit {
    User.rename(user: mut session.owner)
}
"#;
    let diagnostics = analyze_source("weak-field-direct-use.rss", source);
    let weak_use_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0903")
        .count();

    assert_eq!(weak_use_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_accepts_explicit_weak_upgrade() {
    let source = r#"
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn Weak.upgrade(value: read User) -> Option<User>

fn read_owner(session: read Session) -> Unit {
    let owner = Weak.upgrade(value: read session.owner)
}
"#;
    let diagnostics = analyze_source("weak-field-upgrade.rss", source);

    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
}

#[test]
fn rust_lowering_uses_shared_handles_for_managed_class_mut_parameters() {
    let source = r#"
features: local

class User {
    id: Int
}

fn touch(user: mut User) -> Unit {
    return Unit
}

fn call(user: mut User) -> Unit {
    touch(user: mut user)
}

fn promote(id: Int) -> Unit {
    let shared = User(id: id)
    touch(user: mut shared)
}
"#;
    let rust = lower_source_to_rust("user.rss", source).expect("source should lower");

    assert!(rust.contains("fn touch(user: &rsscript_runtime::Managed<User>)"));
    assert!(rust.contains("fn call(user: &rsscript_runtime::Managed<User>)"));
    assert!(rust.contains("touch(user);"));
    assert!(rust.contains("touch(&shared);"));
    assert!(!rust.contains("&mut rsscript_runtime::Managed<User>"));
    assert!(!rust.contains("touch(&mut shared);"));
}

#[test]
fn rust_lowering_reads_managed_class_fields_through_runtime_handle() {
    let source = r#"
features: local

class User {
    id: Int
}

fn user_id(user: read User) -> Int {
    return user.id
}

fn main() -> Unit {
    let shared = User(id: 42)
    let id = user_id(user: read shared)
    Assert.equal(left: read "42", right: read "42")
    return Unit
}
"#;
    let package = lower_source_to_rust_package(
        "user.rss",
        source,
        "managed-class-field",
        &format!("{}/runtime", env!("CARGO_MANIFEST_DIR")),
    )
    .expect("source should lower into package");
    assert!(package.lib_rs.contains(
        "return rsscript_runtime::unwrap_runtime(user.try_read_at(rsscript_runtime::SourceSpan::new(\"user.rss\", 9, 12, 4))).id.clone();"
    ));

    let temp_dir = unique_temp_dir("rsscript-managed-class-field");
    write_generated_rust_package(&temp_dir, &package).expect("generated package should be written");
    let check =
        check_generated_rust_package(&temp_dir).expect("generated package should be checked");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        check.success,
        "diagnostics={:?}\nstderr={}",
        check.diagnostics, check.stderr
    );
}

#[test]
fn rust_lowering_maps_read_and_mut_effects_to_rust_borrows() {
    let source = r#"
features: local

struct Meter {
    value: Int
}

fn read_value(counter: read Meter) -> Int {
    return counter.value
}

fn touch(counter: mut Meter) -> Unit {
    return Unit
}

pub fn run() -> Int {
    local counter = Meter(value: 1)
    touch(counter: mut counter)
    return read_value(counter: read counter)
}
"#;
    let rust = lower_source_to_rust("effects.rss", source).expect("source should lower");

    assert!(rust.contains("fn read_value(counter: &Meter) -> i64"));
    assert!(rust.contains("fn touch(counter: &mut Meter)"));
    assert!(rust.contains("touch(&mut counter);"));
    assert!(rust.contains("return read_value(&counter);"));
}

#[test]
fn rust_lowering_can_emit_cargo_package_artifacts() {
    let source = r#"
struct Point {
    x: Int
    y: Int
}

pub fn make_point(x: Int, y: Int) -> fresh Point {
    return Point(x: x, y: y)
}
"#;
    let package = lower_source_to_rust_package(
        "point.rss",
        source,
        "Point Example.rss",
        "/workspace/rsscript/runtime",
    )
    .expect("source should lower into package");

    assert_eq!(package.package_name, "point-example-rss");
    assert!(package.cargo_toml.contains("[workspace]"));
    assert!(
        package
            .cargo_toml
            .contains("rsscript-runtime = { path = \"/workspace/rsscript/runtime\" }")
    );
    assert!(package.lib_rs.contains("pub struct Point"));
    assert!(package.main_rs.is_none());
    let source_map: Value =
        serde_json::from_str(&package.source_map_json).expect("source map JSON should parse");
    assert!(source_map.as_array().is_some_and(|items| !items.is_empty()));
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
            bindings: BTreeMap::new(),
        }],
    )
    .expect("source should lower into package with native dependency");

    assert!(
        package
            .cargo_toml
            .contains("\"rss_json_native\" = { path = \"/workspace/rss-json/native/rust\" }")
    );
}

#[test]
fn rust_lowering_can_emit_runnable_main_harness() {
    let source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let package = lower_source_to_rust_package(
        "main.rss",
        source,
        "Runnable Example.rss",
        "/workspace/rsscript/runtime",
    )
    .expect("source should lower into package");
    let main_rs = package
        .main_rs
        .as_ref()
        .expect("zero-argument Unit main should emit a Rust binary harness");

    assert_eq!(package.package_name, "runnable-example-rss");
    assert!(package.lib_rs.contains("pub fn main()"));
    assert!(main_rs.contains("rsscript_runtime::install_runtime_diagnostic_panic_hook();"));
    assert!(main_rs.contains("runnable_example_rss::main();"));
}

#[test]
fn rust_lowering_can_emit_result_main_harness() {
    let source = r#"
struct MainError

fn main() -> Result<Unit, MainError> {
    return Ok(Unit)
}
"#;
    let package = lower_source_to_rust_package(
        "main.rss",
        source,
        "Result Runnable Example.rss",
        "/workspace/rsscript/runtime",
    )
    .expect("source should lower into package");
    let main_rs = package
        .main_rs
        .as_ref()
        .expect("zero-argument Result<Unit, E> main should emit a Rust binary harness");

    assert!(
        package
            .lib_rs
            .contains("pub fn main() -> Result<(), MainError>")
    );
    assert!(main_rs.contains("rsscript_runtime::install_runtime_diagnostic_panic_hook();"));
    assert!(main_rs.contains(
        "result_runnable_example_rss::main().expect(\"RSScript main returned an error\");"
    ));
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

#[test]
fn rust_lowering_rejects_unsupported_syntax_before_generation() {
    let source = r#"
fn bad(path: read Path) -> Unit {
    with File.open(path: read path) {
        return Unit
    }
}
"#;
    let diagnostics = lower_source_to_rust("unsupported.rss", source)
        .expect_err("unsupported source should fail before Rust generation");
    let codes = diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RS0015".to_string()));
}

#[test]
fn rust_lowering_rejects_unimplemented_declaration_calls_before_generation() {
    let source = r#"
features: local

struct Meter {
    value: Int
}

fn touch(counter: mut Meter) -> Unit

pub fn run() -> Unit {
    local counter = Meter(value: 1)
    touch(counter: mut counter)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("unimplemented.rss", source)
        .expect_err("unimplemented declaration call should fail before Rust generation");

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "unimplemented declaration call"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_managed_type_drop_blocks() {
    let source = r#"
class Session {
    id: Int

    drop {
        Log.write(message: read "closing")
    }
}

struct BufferOwner {
    bytes: Bytes

    drop {
        Log.write(message: read "closing")
    }
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}
"#;
    let diagnostics = analyze_source("managed-drop.rss", source);
    let managed_drop_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "unsupported managed drop"
        })
        .count();

    assert_eq!(managed_drop_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_unknown_top_level_items_as_unsupported() {
    let source = r#"
enum Color {
    Red
}

fn main() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("unknown-top-level.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "unsupported top-level item"
    }));
}

#[test]
fn checker_reports_malformed_declarations_as_unsupported() {
    let source = r#"
fn (value: read String) -> Unit {
    return Unit
}

struct {
    value: String
}

fn main() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-declarations.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed declaration"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_parameters_as_unsupported() {
    let source = r#"
fn bad(read Path) -> Unit {
    return Unit
}

fn leading_comma(, path: read Path) -> Unit {
    return Unit
}

fn double_comma(path: read Path,, output: read Path) -> Unit {
    return Unit
}

fn trailing_comma(path: read Path,) -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-parameters.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed parameter declaration"
        })
        .count();

    assert_eq!(malformed_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_generic_parameters_as_unsupported() {
    let source = r#"
struct Box<T: Unknown> {
    value: handle T
}

fn bad<read T>(value: read String) -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-generics.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015"
                && diagnostic.label == "malformed generic parameter declaration"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_type_arguments_as_unsupported() {
    let source = r#"
fn bad(
    values: read Map<, String>,
    backup: read Result<String,, IOError>,
) -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-type-args.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed type argument"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_empty_call_arguments_as_unsupported() {
    let source = r#"
fn main() -> Unit {
    Log.write(, message: read "hello")
    Log.write(message: read "hello",, tag: read "demo")
    Log.write(message: read "trailing comma is ok",)
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-call-args.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed call argument"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_effect_declarations_as_unsupported() {
    let source = r#"
fn empty_effect_slot(value: read String) -> Unit
    effects(no_panic,, native)
{
    return Unit
}

fn empty_retains(value: read String) -> Unit
    effects(retains())
{
    return Unit
}

fn unknown_effect_call(value: read String) -> Unit
    effects(custom(value))
{
    return Unit
}
"#;
    let diagnostics = analyze_source("malformed-effects.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed effect declaration"
        })
        .count();

    assert_eq!(malformed_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_match_arms_as_unsupported() {
    let source = r#"
fn bad(value: read Option<Int>) -> Int {
    match value {
        Some(result) return result
        None => return 0
    }
}
"#;
    let diagnostics = analyze_source("malformed-match-arm.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "malformed match arm"
    }));
}

#[test]
fn checker_reports_malformed_with_statements_as_unsupported() {
    let source = r#"
fn missing_as(path: read Path) -> Unit {
    with File.open(path: read path) {
        return Unit
    }
}

fn missing_binding(path: read Path) -> Unit {
    with File.open(path: read path) as {
        return Unit
    }
}
"#;
    let diagnostics = analyze_source("malformed-with.rss", source);
    let malformed_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed with statement"
        })
        .count();

    assert_eq!(malformed_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_reports_malformed_control_statements_as_unsupported() {
    let source = r#"
fn bad_if(value: read Bool) -> Unit {
    if value {
        return Unit
    } else return Unit
}

fn bad_loop() -> Unit {
    loop true {
        return Unit
    }
}

fn bad_while() -> Unit {
    while {
        return Unit
    }
}

fn bad_match() -> Unit {
    match {
        _ => return Unit
    }
}
"#;
    let diagnostics = analyze_source("malformed-control.rss", source);

    for label in [
        "malformed if statement",
        "malformed loop statement",
        "malformed match statement",
    ] {
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "RS0015" && diagnostic.label == label),
            "{label}: {diagnostics:?}",
        );
    }

    let loop_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "malformed loop statement"
        })
        .count();
    assert_eq!(loop_count, 2, "{diagnostics:?}");
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
fn checker_reports_trailing_expression_tokens_as_unsupported() {
    let source = r#"
fn main() -> Unit {
    let call = Log.write(message: read "hello") extra
    let name = call extra
    return Unit
}
"#;
    let diagnostics = analyze_source("trailing-expression-token.rss", source);
    let unsupported_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "unsupported expression"
        })
        .count();

    assert_eq!(unsupported_count, 2, "{diagnostics:?}");
}

#[test]
fn parser_keeps_multiline_return_call_with_try_as_one_statement() {
    let source = r#"
fn make_response(
    status: read String,
    trace_id: read String,
) -> Result<fresh Response, HttpError> {
    return Response.ok(
        body: read String.concat(left: read status, right: read trace_id),
    )?
}
"#;
    let diagnostics = analyze_source("multiline-return-call.rss", source);
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
}

#[test]
fn checker_reports_unclosed_delimiters_on_own_construct() {
    let unclosed_body = r#"
fn main() -> Unit {
    let x = 1
"#;
    let diagnostics = analyze_source("unclosed-function-body.rss", unclosed_body);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015"
            && diagnostic.label == "malformed declaration"
            && diagnostic.span.line == 2
    }));

    let unclosed_call = r#"
fn main() -> Unit {
    let x = Log.write(message: read "hello"
    return Unit
}
"#;
    let diagnostics = analyze_source("unclosed-call-expression.rss", unclosed_call);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015"
            && diagnostic.label == "unsupported expression"
            && diagnostic.span.line == 3
    }));
}

#[test]
fn checker_rejects_effect_wrapped_managed_to_local() {
    let source = r#"
features: local

struct Widget

fn make_widget() -> fresh Widget

fn bad() -> Unit {
    let shared = make_widget()
    local read_copy = read shared
    local mut_copy = mut shared
    return Unit
}
"#;
    let diagnostics = analyze_source("managed-to-local-effect.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_wrapped_managed_bound_as_local() {
    let source = r#"
features: local

struct Widget

fn make_widget() -> fresh Widget

fn bad() -> Unit {
    let shared = make_widget()
    local maybe_shared = Some(shared)
    local read_maybe_shared = read Some(shared)
    return Unit
}
"#;
    let diagnostics = analyze_source("managed-wrapper-to-local.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_handle_field_bound_as_local() {
    let source = r#"
features: local

struct Rules
struct Holder {
    rules: handle Rules
}

fn make_holder() -> fresh Holder

fn bad() -> Unit {
    local holder = make_holder()
    local rules = holder.rules
    local read_rules = read holder.rules
    return Unit
}
"#;
    let diagnostics = analyze_source("handle-field-to-local.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_wrapped_handle_field_bound_as_local() {
    let source = r#"
features: local

struct Rules
struct Holder {
    rules: handle Rules
}

fn make_holder() -> fresh Holder

fn bad() -> Unit {
    local holder = make_holder()
    local maybe_rules = Some(holder.rules)
    local read_maybe_rules = read Some(holder.rules)
    return Unit
}
"#;
    let diagnostics = analyze_source("handle-field-wrapper-to-local.rss", source);
    let managed_to_local_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0301" && diagnostic.label == "managed value used as local"
        })
        .count();

    assert_eq!(managed_to_local_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_retaining_local_inline_field() {
    let source = r#"
features: local

struct Image
struct Holder {
    image: Image
}
struct Cache

fn Cache.store(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))

fn make_holder(path: read Path) -> fresh Holder

fn bad_store(cache: mut Cache, path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    Cache.store(cache: mut cache, value: read holder.image)
}
"#;
    let diagnostics = analyze_source("retaining-local-field.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0501" && diagnostic.label == "local value retained"
    ));
}

#[test]
fn checker_rejects_retaining_local_through_enum_wrapper() {
    let source = r#"
features: local

struct Image
struct Holder {
    image: Image
}
struct Cache

fn Cache.store_option(cache: mut Cache, value: read Option<Image>) -> Unit
    effects(retains(value))

fn Cache.store_result(cache: mut Cache, value: read Result<Image, StoreError>) -> Unit
    effects(retains(value))

fn make_holder() -> fresh Holder

fn bad_store(cache: mut Cache) -> Unit {
    local holder = make_holder()
    Cache.store_option(cache: mut cache, value: read Some(holder.image))
    Cache.store_result(cache: mut cache, value: read Ok(holder.image))
    Cache.store_option(cache: mut cache, value: read Some(read holder.image))
}
"#;
    let diagnostics = analyze_source("retaining-local-wrapper.rss", source);
    let retained_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0501" && diagnostic.label == "local value retained"
        })
        .count();

    assert_eq!(retained_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_accepts_managed_closure_capturing_handle_field() {
    let source = r#"
features: local

class Image

struct Holder {
    image: handle Image
}

fn make_holder(path: read Path) -> fresh Holder

fn use_image(image: read Image) -> Unit

fn ok_capture(path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    let callback = || {
        use_image(image: read holder.image)
    }
}
"#;
    let diagnostics = analyze_source("managed-closure-handle-field.rss", source);

    assert_eq!(diagnostics, Vec::new());
}

#[test]
fn checker_rejects_resource_escape_through_effect_wrapper() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn bad_return(path: read Path) -> File {
    with File.open(path: read path) as file {
        return read file
    }
}

fn bad_binding(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        let saved = read file
    }
}
"#;
    let diagnostics = analyze_source("resource-effect-escape.rss", source);
    let escape_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0702" && diagnostic.label == "resource escapes")
        .count();

    assert_eq!(escape_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_escape_through_result_wrapper() {
    let source = r#"
features: local

resource LogFile {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn LogFile.open(path: read Path) -> LogFile

fn bad_return(path: read Path) -> Unit {
    with LogFile.open(path: read path) as file {
        return Ok(file)
    }
}

fn bad_binding(path: read Path) -> Unit {
    with LogFile.open(path: read path) as file {
        let saved = Some(file)
    }
}
"#;
    let diagnostics = analyze_source("resource-wrapper-escape.rss", source);
    let escape_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0702" && diagnostic.label == "resource escapes")
        .count();

    assert_eq!(escape_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_requires_resource_producers_to_enter_with_context() {
    let source = r#"
features: local

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn File.open(path: read Path) -> File
fn File.open_result(path: read Path) -> Result<File, IOError>
fn File.stat(file: read File) -> Unit

fn ok_with(path: read Path) -> Result<Unit, IOError> {
    with File.open_result(path: read path)? as file {
        File.stat(file: read file)
    }
    return Ok(Unit)
}

fn bad_with_missing_try(path: read Path) -> Unit {
    with File.open_result(path: read path) as file {
        File.stat(file: read file)
    }
}

fn bad_let(path: read Path) -> Unit {
    let file = File.open(path: read path)
}

fn bad_result_let(path: read Path) -> Result<Unit, IOError> {
    let file = File.open_result(path: read path)?
    return Ok(Unit)
}

fn bad_return(path: read Path) -> File {
    return File.open(path: read path)
}

fn bad_result_return(path: read Path) -> Result<File, IOError> {
    return File.open_result(path: read path)?
}

fn bad_arg(path: read Path) -> Unit {
    File.stat(file: read File.open(path: read path))
}
"#;
    let diagnostics = analyze_source("resource-producer-context.rss", source);
    let producer_escape_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0702" && diagnostic.label == "resource producer escapes"
        })
        .count();

    assert_eq!(producer_escape_count, 5, "{diagnostics:?}");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0706" && diagnostic.label == "missing resource producer `?`"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_same_call_managed_place_conflict() {
    let source = r#"
class Cache {
    count: Int
}

fn Cache.new() -> Cache
fn touch(first: mut Cache, second: read Cache) -> Unit

fn bad() -> Unit {
    let cache = Cache.new()
    touch(first: mut cache, second: read cache)
}
"#;
    let diagnostics = analyze_source("same-call-managed-place.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0303" && diagnostic.label == "field path conflict"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_same_call_manage_conflict_independent_of_argument_order() {
    let source = r#"
features: local

struct Image {
    width: Int
}

fn Image.new() -> fresh Image
fn sink(left: read Image, right: read Image) -> Unit

fn bad_read_then_manage() -> Unit {
    local image = Image.new()
    sink(left: read image, right: read (manage image))
}

fn bad_manage_then_read() -> Unit {
    local image = Image.new()
    sink(left: read (manage image), right: read image)
}
"#;
    let diagnostics = analyze_source("same-call-manage-order.rss", source);
    let move_conflicts = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0305")
        .count();

    assert_eq!(move_conflicts, 2, "{diagnostics:?}");
}

#[test]
fn checker_allows_result_resource_only_as_return_producer_contract() {
    let source = r#"
resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Holder {
    file: Result<File, IOError>
}

fn accept(result: read Result<File, IOError>) -> Unit
fn File.open(path: read Path) -> Result<File, IOError>
"#;
    let diagnostics = analyze_source("result-resource-contract.rss", source);
    let generic_resource_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0704")
        .count();

    assert_eq!(generic_resource_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_pool_non_positive_or_dynamic_max_size() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn DbConnection.open(url: read String) -> DbConnection

fn zero_pool(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 0,
    )
}

fn negative_pool(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: -1,
    )
}

fn dynamic_pool(url: read String, count: Int) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: count,
    )
}
"#;
    let diagnostics = analyze_source("resourcepool-max-size.rss", source);
    let invalid_max_size_count = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0708" && diagnostic.label == "invalid ResourcePool max_size"
        })
        .count();

    assert_eq!(invalid_max_size_count, 3, "{diagnostics:?}");
}

#[test]
fn checker_matches_resource_pool_constructor_to_factory_result_shape() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn DbConnection.open(url: read String) -> DbConnection
fn DbConnection.try_open(url: read String) -> Result<DbConnection, DbError>

fn bad_new(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.try_open(url: read url),
        max_size: 1,
    )
}

fn bad_try_new(url: read String) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.try_new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )?

    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("resourcepool-factory-shape.rss", source);
    let factory_contract_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0707")
        .count();

    assert_eq!(factory_contract_count, 2, "{diagnostics:?}");
}

#[test]
fn checker_rejects_resource_pool_use_while_lease_is_active() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

fn DbConnection.open(url: read String) -> DbConnection
fn Pool.consume(pool: take ResourcePool<DbConnection>) -> Unit
fn Pool.stats(pool: read ResourcePool<DbConnection>) -> Int

fn bad_nested_borrow(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 2,
    )

    with ResourcePool.borrow(pool: mut pool) as first {
        with ResourcePool.borrow(pool: mut pool) as second {
            Log.write(message: read "unreachable")
        }
    }
}

fn bad_take_during_lease(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        Pool.consume(pool: take pool)
    }
}

fn bad_read_during_lease(url: read String) -> Unit {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        let count = Pool.stats(pool: read pool)
        Log.write(message: read "unreachable")
    }
}

fn ok_different_pool(url: read String) -> Unit {
    local first_pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )
    local second_pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 1,
    )

    with ResourcePool.borrow(pool: mut first_pool) as conn {
        with ResourcePool.borrow(pool: mut second_pool) as other {
            Log.write(message: read "ok")
        }
    }
}
"#;
    let diagnostics = analyze_source("resourcepool-active-lease.rss", source);
    let active_lease_conflicts = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0709" && diagnostic.label == "ResourcePool active lease conflict"
        })
        .count();

    assert_eq!(active_lease_conflicts, 3, "{diagnostics:?}");
}

#[test]
fn checker_tracks_use_after_take_for_local_inline_field() {
    let bad = r#"
features: local

struct Image
struct Holder {
    image: Image
}

fn make_holder(path: read Path) -> fresh Holder
fn consume(image: take Image) -> Unit
fn inspect(image: read Image) -> Unit

fn bad_take_field(path: read Path) -> Unit {
    local holder = make_holder(path: read path)
    consume(image: take holder.image)
    inspect(image: read holder.image)
}
"#;
    let diagnostics = analyze_source("take-inline-field-use-after.rss", bad);
    assert!(
        diagnostics.iter().any(
            |diagnostic| diagnostic.code == "RS0401" && diagnostic.label == "used after manage"
        )
    );

    let ok = r#"
features: local

struct Image
struct Pair {
    left: Image
    right: Image
}

fn make_pair(path: read Path) -> fresh Pair
fn consume(image: take Image) -> Unit
fn inspect(image: read Image) -> Unit

fn ok_take_disjoint(path: read Path) -> Unit {
    local pair = make_pair(path: read path)
    consume(image: take pair.left)
    inspect(image: read pair.right)
}
"#;
    assert_eq!(
        analyze_source("take-inline-field-disjoint.rss", ok),
        Vec::new()
    );
}

#[test]
fn review_json_uses_protocol_shape() {
    let old_source = r#"

fn render(path: read Path) -> Image
    effects(no_panic)
{
    Image.load(path: read path)
}
"#;
    let new_source = r#"
features: local

fn render(path: take Path) -> fresh Image
    effects(retains(path))
{
    Image.load(path: read path)
}
"#;
    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    let items = value.as_array().expect("review JSON should be an array");

    assert!(
        items
            .iter()
            .any(|item| item["code"] == "RSR001" && item["risk"] == "feature")
    );
    assert!(
        items
            .iter()
            .any(|item| item["code"] == "RSR006" && item["risk"] == "effect")
    );
    let effect = items
        .iter()
        .find(|item| item["code"] == "RSR006")
        .expect("expected effect review finding");
    assert_eq!(effect["before"], "no_panic");
    assert_eq!(effect["after"], "retains(path)");
    assert!(
        effect["spans"]
            .as_array()
            .is_some_and(|spans| spans.len() == 2)
    );
    let effect_fixes = effect["fixes"]
        .as_array()
        .expect("review finding should include fixes");
    assert!(effect_fixes.iter().any(|fix| {
        fix["kind"] == "review_effect_contract" && fix["applicability"] == "manual"
    }));
    assert!(items.iter().all(|item| {
        item["fixes"]
            .as_array()
            .is_some_and(|fixes| !fixes.is_empty())
    }));
    assert!(items.iter().all(|item| {
        item["summary"]
            .as_str()
            .is_some_and(|summary| !summary.is_empty())
    }));
}

#[test]
fn syntax_parser_accepts_all_fixtures() {
    let mut paths = fixture_paths("tests/fixtures/pass");
    paths.extend(fixture_paths("tests/fixtures/fail"));
    paths.extend(fixture_paths("examples"));

    for path in paths {
        let source = read_fixture(&path);
        let program = parse_source(path.to_str().unwrap(), &source);
        if program.items.is_empty() {
            assert!(
                !program.malformed_declaration_spans.is_empty()
                    || !program.unknown_top_level_spans.is_empty(),
                "{} missing items without parser diagnostics",
                path.display()
            );
            continue;
        }
        assert!(
            program
                .items
                .iter()
                .any(|item| matches!(item, Item::Function(_))),
            "{} missing function item",
            path.display()
        );
        if path.extension().is_some_and(|extension| extension == "rss") {
            assert!(
                program.items.iter().any(|item| match item {
                    Item::Function(function) => !function.body.statements.is_empty(),
                    Item::Type(_) => false,
                }),
                "{} missing function body statements",
                path.display()
            );
        }
    }
}

#[test]
fn review_reports_api_contract_changes() {
    let old_source = r#"

fn render(path: read Path) -> Image
    effects(no_panic)
{
    Image.load(path: read path)
}
"#;
    let new_source = r#"
features: local

fn render(path: take Path, width: Int) -> fresh Image
    effects(retains(path))
{
    Image.load(path: read path)
}

fn inspect(image: read Image) -> Unit {
    Image.inspect(image: read image)
}
"#;

    let codes: Vec<String> = review_sources("old.rss", old_source, "new.rss", new_source)
        .into_iter()
        .map(|finding| finding.code)
        .collect();

    assert!(codes.contains(&"RSR001".to_string()));
    assert!(codes.contains(&"RSR003".to_string()));
    assert!(codes.contains(&"RSR004".to_string()));
    assert!(codes.contains(&"RSR005".to_string()));
    assert!(codes.contains(&"RSR006".to_string()));
    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR001" && finding.risk == ReviewRisk::Feature)
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR006" && finding.risk == ReviewRisk::Effect)
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR004" && finding.risk == ReviewRisk::Api)
    );
    assert!(format_review_human(&findings).contains("RSR006[effect]:"));
}

#[test]
fn review_reports_function_kind_changes() {
    let old_source = r#"
fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError> {
    Net.fetch(url: read url)
}
"#;
    let new_source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError> {
    Net.fetch(url: read url)
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let kind = findings
        .iter()
        .find(|finding| finding.code == "RSR014")
        .expect("expected function kind review finding");

    assert_eq!(kind.risk, ReviewRisk::Api);
    assert_eq!(kind.before.as_deref(), Some("fn"));
    assert_eq!(kind.after.as_deref(), Some("async fn"));
    assert!(format_review_human(&findings).contains("RSR014[api]: function `fetch` kind changed."));
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
fn review_reports_removed_guarantees() {
    let old_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(noalloc, no_panic, pure)
{
    Bytes.checksum(data: read data)
}
"#;
    let new_source = r#"

fn checksum(data: read Bytes) -> UInt64
    effects(no_panic)
{
    Bytes.checksum(data: read data)
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let guarantee = findings
        .iter()
        .find(|finding| finding.code == "RSR013")
        .expect("expected removed guarantee finding");

    assert_eq!(guarantee.risk, ReviewRisk::Guarantee);
    assert_eq!(guarantee.before.as_deref(), Some("no_panic, noalloc, pure"));
    assert_eq!(guarantee.after.as_deref(), Some("no_panic"));
    assert!(
        guarantee
            .summary
            .contains("removed guarantee(s): noalloc, pure")
    );
    assert!(format_review_human(&findings).contains("RSR013[guarantee]:"));

    let json = format_review_json(&findings);
    let value: Value = serde_json::from_str(&json).expect("review JSON should parse");
    assert!(value.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "RSR013" && item["risk"] == "guarantee")
    }));
}

#[test]
fn review_reports_type_layout_changes() {
    let old_source = r#"

struct Config {
    rules: List<Rule>
    version: Int
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}
"#;
    let new_source = r#"

class Config {
    rules: handle List<Rule>
    version: Int
}

struct Session {
    config: Config
}
"#;

    let codes: Vec<String> = review_sources("old.rss", old_source, "new.rss", new_source)
        .into_iter()
        .map(|finding| finding.code)
        .collect();

    assert!(codes.contains(&"RSR007".to_string()));
    assert!(codes.contains(&"RSR008".to_string()));
    assert!(codes.contains(&"RSR009".to_string()));
    assert!(codes.contains(&"RSR010".to_string()));
    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.code == "RSR010" && finding.risk == ReviewRisk::TypeLayout)
    );
}

#[test]
fn review_reports_local_manage_boundary_changes() {
    let old_source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    Image.inspect(image: read Image.load(path: read path))
}
"#;
    let new_source = r#"
features: local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
}
"#;

    let findings = review_sources("old.rss", old_source, "new.rss", new_source);
    let codes: Vec<String> = findings
        .iter()
        .map(|finding| finding.code.clone())
        .collect();

    assert!(codes.contains(&"RSR011".to_string()));
    let boundary = findings
        .iter()
        .find(|finding| finding.code == "RSR011")
        .expect("expected boundary review finding");
    assert!(boundary.summary.contains("added local binding `image`"));
    assert!(boundary.summary.contains("added manage `image`"));
    assert!(boundary.summary.contains("body[1]"));
    assert!(boundary.summary.contains("body[2].value"));
    assert_eq!(boundary.risk, ReviewRisk::Boundary);
    assert!(
        boundary
            .fixes
            .iter()
            .any(|fix| fix.kind == "review_local_manage_boundary")
    );
}

#[test]
fn review_map_partitions_functions_for_review() {
    let source = r#"
fn helper(value: read Int) -> Int {
    return value
}

pub fn publish(path: read Path) -> Image {
    local image = Image.load(path: read path)
    return manage image
}

fn delegated(value: read Int) -> Int {
    return unknown(value: read value)
}
"#;

    let map = review_map_sources(vec![("map.rss", source)]);
    let regions = &map.files[0].regions;

    assert_eq!(map.summary.total_functions, 3);
    assert_eq!(map.summary.review_required.functions, 1);
    assert_eq!(map.summary.foldable.functions, 1);
    assert_eq!(map.summary.unknown.functions, 1);
    assert!(map.summary.total_lines >= 3);
    let human = format_review_map_human(&map);
    assert!(human.starts_with("summary: must-review 1 functions/"));
    assert!(human.contains("low-semantic-risk 1 functions/"));
    assert!(human.contains("helper [low-semantic-risk]"));
    let json = format_review_map_json(&map);
    let value: Value = serde_json::from_str(&json).expect("review map JSON should parse");
    assert_eq!(value["summary"]["total_functions"], 3);
    assert_eq!(value["summary"]["must_review"]["functions"], 1);
    assert_eq!(value["summary"]["low_semantic_risk"]["functions"], 1);
    assert_eq!(value["summary"]["unknown"]["functions"], 1);
    assert!(value["summary"]["must_review_lines"].is_number());
    assert!(value["summary"]["low_semantic_risk_lines"].is_number());
    assert!(value["summary"]["suggested_review_lines"].is_number());
    assert!(value["summary"]["review_ratio"].is_number());
    assert!(value["summary"]["unknown_ratio"].is_number());
    assert!(value["summary"]["unknown_function_ratio"].is_number());
    assert!(
        value["files"][0]["regions"]
            .as_array()
            .is_some_and(|regions| regions
                .iter()
                .any(|region| region["classification"] == "must_review"))
    );
    assert!(
        value["files"][0]["regions"]
            .as_array()
            .is_some_and(|regions| regions
                .iter()
                .any(|region| region["classification"] == "low_semantic_risk"))
    );
    assert!(
        value["files"]
            .as_array()
            .is_some_and(|files| files.len() == 1)
    );
    assert!(
        value["files"][0]["features"]
            .as_array()
            .is_some_and(|features| features.is_empty())
    );

    assert!(regions.iter().any(|region| {
        region.function == "helper" && region.classification == ReviewMapClassification::Foldable
    }));
    assert!(regions.iter().any(|region| {
        region.function == "publish"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "public entry point")
    }));
    assert!(regions.iter().any(|region| {
        region.function == "delegated" && region.classification == ReviewMapClassification::Unknown
    }));
}

#[test]
fn review_map_marks_async_signatures_review_required() {
    let source = r#"
features: async

async fn ping(url: read Url) -> Result<Unit, NetworkError>
"#;

    let map = review_map_sources(vec![("async-map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "ping")
        .expect("expected ping region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "async function boundary"),
        "{region:?}"
    );
    assert_eq!(map.summary.foldable.functions, 0);
    assert_eq!(map.summary.review_required.functions, 1);
}

#[test]
fn review_map_marks_spawn_retention_boundary() {
    let source = r#"
features: async

async fn fetch_user(client: read HttpClient, id: read UserId) -> Result<fresh User, HttpError>

fn schedule(client: read HttpClient, id: read UserId) -> Unit {
    let task = spawn fetch_user(client: read client, id: read id)
    return Unit
}
"#;

    let map = review_map_sources(vec![("spawn-map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "schedule")
        .expect("expected schedule region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "spawn task boundary"),
        "{region:?}"
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "spawn retains-until-task-complete `client`, `id`"),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_marks_await_suspension_boundary() {
    let source = r#"
features: async

async fn fetch_user(client: read HttpClient, id: read UserId) -> Result<fresh User, HttpError>

fn receive(client: read HttpClient, id: read UserId) -> Unit {
    let user = await fetch_user(client: read client, id: read id)
    return Unit
}
"#;

    let map = review_map_sources(vec![("await-map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "receive")
        .expect("expected receive region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "await suspension boundary"),
        "{region:?}"
    );
}

#[test]
fn review_map_marks_noescape_callback_calls_review_required_not_unknown() {
    let source = r#"
features: local

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}
"#;
    let map = review_map_sources(vec![("callback.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "apply")
        .expect("expected apply region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "noescape callback call `callback`"),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
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
fn review_map_marks_local_closure_direct_calls_review_required_not_unknown() {
    let source = r#"
features: local

fn run() -> Unit {
    local callback = || {
        return Unit
    }

    callback()
    return Unit
}
"#;
    let map = review_map_sources(vec![("local-callback.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "run")
        .expect("expected run region");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        !region
            .reasons
            .iter()
            .any(|reason| reason.contains("unresolved call")),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_marks_managed_closure_capture_retention() {
    let source = r#"
struct Image

fn retain_callback(image: read Image) -> Unit {
    let callback = || {
        let seen = image
    }
    return Unit
}
"#;
    let map = review_map_sources(vec![("managed-closure-retention.rss", source)]);
    let region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "retain_callback")
        .expect("retain_callback region should exist");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "managed closure retains `image`"),
        "{region:?}"
    );
    assert_eq!(map.summary.unknown.functions, 0);
}

#[test]
fn review_map_marks_inline_managed_closure_capture_retention_unless_noescape() {
    let source = r#"
struct Image
struct Callback

fn store(callback: read Callback) -> Unit
    effects(retains(callback))

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn retain_inline(image: read Image) -> Unit {
    store(callback: read || {
        Image.inspect(image: read image)
    })
    return Unit
}

fn noescape_inline(image: read Image) -> Unit {
    apply(callback: || {
        Image.inspect(image: read image)
    })
    return Unit
}
"#;
    let map = review_map_sources(vec![("inline-closure-retention.rss", source)]);
    let retain_region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "retain_inline")
        .expect("retain_inline region should exist");
    let noescape_region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "noescape_inline")
        .expect("noescape_inline region should exist");
    let apply_region = map
        .files
        .iter()
        .flat_map(|file| &file.regions)
        .find(|region| region.function == "apply")
        .expect("apply region should exist");

    assert!(
        retain_region
            .reasons
            .iter()
            .any(|reason| reason == "managed closure retains `image`"),
        "{retain_region:?}"
    );
    assert!(
        !noescape_region
            .reasons
            .iter()
            .any(|reason| reason == "managed closure retains `image`"),
        "{noescape_region:?}"
    );
    assert!(
        apply_region
            .reasons
            .iter()
            .any(|reason| reason == "noescape callback call `callback`"),
        "{apply_region:?}"
    );
}

#[test]
fn review_map_pass_fixture_unknown_rate_stays_low() {
    let owned_sources = fixture_paths("tests/fixtures/pass")
        .into_iter()
        .map(|path| {
            let file = path.display().to_string();
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            (file, source)
        })
        .collect::<Vec<_>>();
    let sources = owned_sources
        .iter()
        .map(|(file, source)| (file.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let map = review_map_sources(sources);

    assert!(map.summary.total_functions >= 60);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn readme_review_map_confidence_corpus_count_matches_fixtures() {
    let owned_sources = fixture_paths("tests/fixtures/pass")
        .into_iter()
        .map(|path| {
            let file = path.display().to_string();
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            (file, source)
        })
        .collect::<Vec<_>>();
    let sources = owned_sources
        .iter()
        .map(|(file, source)| (file.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let map = review_map_sources(sources);
    let readme = fs::read_to_string("README.md").expect("README should be readable");
    let expected = format!(
        "reports {} functions / {} lines with 0 unknown functions and 0 unknown lines",
        map.summary.total_functions, map.summary.total_lines
    );

    assert!(
        readme.contains(&expected),
        "README confidence corpus count is stale; expected phrase `{expected}`"
    );
}

#[test]
fn review_map_complex_supported_script_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/complex-supported-review-map.rss");
    let source = read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert_eq!(map.summary.total_functions, 14);
    assert!(map.summary.total_lines >= 70);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert_eq!(map.summary.review_required.functions, 10);
    assert_eq!(map.summary.foldable.functions, 4);
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn review_map_realistic_supported_corpus_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/realistic-supported-review-corpus.rss");
    let source = read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert_eq!(map.summary.total_functions, 21);
    assert!(map.summary.total_lines >= 120);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert!(map.summary.review_required.functions >= 10, "{map:?}");
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn review_map_app_benchmark_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/app-review-benchmark.rss");
    let source = read_fixture(path);
    let map = review_map_sources(vec![(path.to_str().unwrap(), source.as_str())]);

    assert_eq!(map.summary.total_functions, 42);
    assert!(map.summary.total_lines >= 300, "{map:?}");
    assert_eq!(map.files[0].risk, ReviewMapFileRisk::High);
    assert_eq!(map.summary.unknown.functions, 0, "{map:?}");
    assert_eq!(map.summary.unknown.lines, 0, "{map:?}");
    assert!(map.summary.review_required.functions >= 32, "{map:?}");
    assert!(map.summary.foldable.functions <= 10, "{map:?}");

    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
    assert_eq!(json["summary"]["must_review"]["functions"], 33);
    assert_eq!(json["summary"]["low_semantic_risk"]["functions"], 9);
}

#[test]
fn review_map_dogfood_classifier_has_no_unknown_regions() {
    let path = Path::new("tests/fixtures/pass/dogfood-review-classifier.rss");
    let source = read_fixture(path);
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
fn rss_review_map_json_reports_app_benchmark_unknown_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("review")
        .arg("--map")
        .arg("--json")
        .arg("tests/fixtures/pass/app-review-benchmark.rss")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss review --map --json should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be review map JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["summary"]["total_functions"], 42);
    assert_eq!(json["summary"]["unknown"]["functions"], 0);
    assert_eq!(json["summary"]["unknown"]["lines"], 0);
    assert_eq!(json["summary"]["unknown_ratio"], 0.0);
    assert_eq!(json["summary"]["unknown_function_ratio"], 0.0);
}

#[test]
fn dogfood_review_facts_match_generated_review_map_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("review")
        .arg("--map")
        .arg("--json")
        .arg("tests/fixtures/pass/complex-supported-review-map.rss")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss review --map --json should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let generated: Value = serde_json::from_str(&stdout).expect("stdout should be review map JSON");
    let checked_in: Value = serde_json::from_str(&read_fixture(Path::new(
        "tests/fixtures/pass/dogfood-review-facts.json",
    )))
    .expect("dogfood review facts should be JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(checked_in, generated);
}

#[test]
fn rss_verify_rust_json_accepts_dogfood_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-review-classifier");
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("verify-rust")
        .arg("--json")
        .arg("tests/fixtures/pass/dogfood-review-classifier.rss")
        .arg("--out-dir")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss verify-rust --json should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be diagnostics JSON");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert!(json.as_array().is_some_and(|diagnostics| {
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["severity"] != "error")
    }));
}

#[test]
fn rss_run_accepts_dogfood_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-run-generated-review-map");
    let Some(fixture_dir) = prepare_dogfood_run_dir(&temp_dir) else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let review_json = generated_dogfood_review_map_json();
    fs::write(
        fixture_dir.join("dogfood-review-facts.json"),
        serde_json::to_vec(&review_json).expect("review JSON should serialize"),
    )
    .expect("generated review map should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-review-classifier.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    let expected_stdout = format!(
        "dogfood review summary total={} must={} low={} unknown={} lines={} mismatches=0 unmodeled_reasons=0 first_mismatch=none",
        review_json["summary"]["total_functions"],
        review_json["summary"]["must_review"]["functions"],
        review_json["summary"]["low_semantic_risk"]["functions"],
        review_json["summary"]["unknown"]["functions"],
        review_json["summary"]["suggested_review_lines"],
    );
    assert_eq!(stdout.trim(), expected_stdout);
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_reports_dogfood_classification_mismatch_detail() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-run-mismatch");
    let Some(fixture_dir) = prepare_dogfood_run_dir(&temp_dir) else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut review_json = generated_dogfood_review_map_json();
    review_json["files"][0]["regions"][0]["classification"] =
        Value::String("must_review".to_string());
    fs::write(
        fixture_dir.join("dogfood-review-facts.json"),
        serde_json::to_vec(&review_json).expect("review JSON should serialize"),
    )
    .expect("mutated review map should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-review-classifier.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "stdout={stdout}");
    assert!(
        stdout.contains("mismatches=1 unmodeled_reasons=0 first_mismatch=ReviewClass.unknown expected=must_review actual=low_semantic_risk"),
        "{stdout}"
    );
}

#[test]
fn rss_run_reports_dogfood_unmodeled_reason_count() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-run-unmodeled-reason");
    let Some(fixture_dir) = prepare_dogfood_run_dir(&temp_dir) else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut review_json = generated_dogfood_review_map_json();
    review_json["files"][0]["regions"][0]["reasons"]
        .as_array_mut()
        .expect("first region reasons should be an array")
        .push(Value::String("new unmodeled review reason".to_string()));
    fs::write(
        fixture_dir.join("dogfood-review-facts.json"),
        serde_json::to_vec(&review_json).expect("review JSON should serialize"),
    )
    .expect("mutated review map should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-review-classifier.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "stdout={stdout}");
    assert!(
        stdout.contains("mismatches=0 unmodeled_reasons=1 first_mismatch=none"),
        "{stdout}"
    );
}

#[test]
fn rss_run_accepts_current_review_reason_families_in_dogfood_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-run-current-reasons");
    let Some(fixture_dir) = prepare_dogfood_run_dir(&temp_dir) else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut review_json = generated_dogfood_review_map_json();
    let regions = review_json["files"][0]["regions"]
        .as_array_mut()
        .expect("first file regions should be an array");
    let region = regions
        .iter_mut()
        .find(|region| region["classification"] == "low_semantic_risk")
        .expect("dogfood review map should contain a low-risk region");
    let line_count = region["line_count"]
        .as_i64()
        .expect("region line_count should be an integer");
    region["classification"] = Value::String("must_review".to_string());
    let reasons = region["reasons"]
        .as_array_mut()
        .expect("region reasons should be an array");
    reasons.extend([
        Value::String("manage boundary".to_string()),
        Value::String("take effect".to_string()),
        Value::String("async function boundary".to_string()),
        Value::String("await suspension boundary".to_string()),
        Value::String("spawn task boundary".to_string()),
        Value::String("spawn retains-until-task-complete `task`".to_string()),
        Value::String("writes through handle field".to_string()),
        Value::String("guarantee `no_panic`".to_string()),
        Value::String("take parameter `buffer`".to_string()),
    ]);
    review_json["summary"]["must_review"]["functions"] = Value::from(
        review_json["summary"]["must_review"]["functions"]
            .as_i64()
            .unwrap()
            + 1,
    );
    review_json["summary"]["low_semantic_risk"]["functions"] = Value::from(
        review_json["summary"]["low_semantic_risk"]["functions"]
            .as_i64()
            .unwrap()
            - 1,
    );
    review_json["summary"]["suggested_review_lines"] = Value::from(
        review_json["summary"]["suggested_review_lines"]
            .as_i64()
            .unwrap()
            + line_count,
    );
    fs::write(
        fixture_dir.join("dogfood-review-facts.json"),
        serde_json::to_vec(&review_json).expect("review JSON should serialize"),
    )
    .expect("mutated review map should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-review-classifier.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains("mismatches=0 unmodeled_reasons=0 first_mismatch=none"),
        "{stdout}"
    );
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_accepts_dogfood_package_risk_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-risk");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-risk.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let package_review_json = generated_dogfood_package_review_json(&temp_dir);
    fs::write(
        fixture_dir.join("dogfood-package-review.json"),
        serde_json::to_vec(&package_review_json).expect("package review JSON should serialize"),
    )
    .expect("generated package review should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-risk.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package risk classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.trim(),
        "dogfood package risk cases=5 mismatches=0 unmodeled_reasons=0 low=1 elevated=1 high=2 unknown=1"
    );
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_reports_dogfood_package_risk_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-risk-mismatch");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-risk.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut package_review_json = generated_dogfood_package_review_json(&temp_dir);
    package_review_json[1]["risk"] = Value::String("low".to_string());
    fs::write(
        fixture_dir.join("dogfood-package-review.json"),
        serde_json::to_vec(&package_review_json).expect("package review JSON should serialize"),
    )
    .expect("mutated package review should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-risk.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package risk classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("dogfood package risk cases=5 mismatches=1 unmodeled_reasons=0 low=1 elevated=1 high=2 unknown=1"),
        "{stdout}"
    );
}

#[test]
fn rss_run_accepts_dogfood_package_export_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-exports");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-exports.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let package_review_json = generated_dogfood_package_exports_json(&temp_dir);
    fs::write(
        fixture_dir.join("dogfood-package-exports.json"),
        serde_json::to_vec(&package_review_json).expect("package review JSON should serialize"),
    )
    .expect("generated package review should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-exports.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package export classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.trim(),
        "dogfood package exports types=2 functions=3 apis=5 mutating=1 retaining=1 resource=1 fresh=1 unknown=1 mismatches=0 unmodeled_reasons=0"
    );
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_reports_dogfood_package_export_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-exports-mismatch");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-exports.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut package_review_json = generated_dogfood_package_exports_json(&temp_dir);
    package_review_json["summary"]["mutating_apis"] = Value::from(0);
    fs::write(
        fixture_dir.join("dogfood-package-exports.json"),
        serde_json::to_vec(&package_review_json).expect("package review JSON should serialize"),
    )
    .expect("mutated package review should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-exports.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package export classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("dogfood package exports types=2 functions=3 apis=5 mutating=1 retaining=1 resource=1 fresh=1 unknown=1 mismatches=1 unmodeled_reasons=0"),
        "{stdout}"
    );
}

#[test]
fn rss_run_accepts_dogfood_package_diff_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-diff");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-diff.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let package_diff_json = generated_dogfood_package_diff_json(&temp_dir);
    fs::write(
        fixture_dir.join("dogfood-package-diff.json"),
        serde_json::to_vec(&package_diff_json).expect("package diff JSON should serialize"),
    )
    .expect("generated package diff should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-diff.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package diff classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.trim(),
        "dogfood package diff manifest=8 interface=2 high_manifest=5 high_interface=2 mismatches=0 unmodeled_reasons=0"
    );
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_reports_dogfood_package_diff_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-diff-mismatch");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-diff.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut package_diff_json = generated_dogfood_package_diff_json(&temp_dir);
    package_diff_json["risk"] = Value::String("elevated".to_string());
    fs::write(
        fixture_dir.join("dogfood-package-diff.json"),
        serde_json::to_vec(&package_diff_json).expect("package diff JSON should serialize"),
    )
    .expect("mutated package diff should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-diff.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package diff classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("dogfood package diff manifest=8 interface=2 high_manifest=5 high_interface=2 mismatches=1 unmodeled_reasons=0"),
        "{stdout}"
    );
}

#[test]
fn rss_run_accepts_dogfood_package_lock_diff_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-lock-diff");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-lock-diff.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let package_lock_diff_json = generated_dogfood_package_lock_diff_json(&temp_dir);
    fs::write(
        fixture_dir.join("dogfood-package-lock-diff.json"),
        serde_json::to_vec(&package_lock_diff_json)
            .expect("package lock diff JSON should serialize"),
    )
    .expect("generated package lock diff should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-lock-diff.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package lock diff classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.trim(),
        "dogfood package lock diff packages=1 high_packages=1 high_features=1 mismatches=0 unmodeled_reasons=0"
    );
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_reports_dogfood_package_lock_diff_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-package-lock-diff-mismatch");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-package-lock-diff.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut package_lock_diff_json = generated_dogfood_package_lock_diff_json(&temp_dir);
    package_lock_diff_json["risk"] = Value::String("elevated".to_string());
    fs::write(
        fixture_dir.join("dogfood-package-lock-diff.json"),
        serde_json::to_vec(&package_lock_diff_json)
            .expect("package lock diff JSON should serialize"),
    )
    .expect("mutated package lock diff should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-package-lock-diff.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood package lock diff classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("dogfood package lock diff packages=1 high_packages=1 high_features=1 mismatches=1 unmodeled_reasons=0"),
        "{stdout}"
    );
}

#[test]
fn rss_run_accepts_dogfood_rustc_remap_classifier() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-rustc-remap");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-rustc-remap.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let diagnostics_json = generated_dogfood_rustc_remap_json(&temp_dir);
    fs::write(
        fixture_dir.join("dogfood-rustc-remap.json"),
        serde_json::to_vec(&diagnostics_json).expect("remap diagnostics JSON should serialize"),
    )
    .expect("generated remap diagnostics should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-rustc-remap.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood rustc remap classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert_eq!(
        stdout.trim(),
        "dogfood rustc remap diagnostics=1 mapped=1 rustc_code=1 source_file=1 source_line=1 source_column=1"
    );
    assert!(stderr.trim().is_empty(), "{stderr}");
}

#[test]
fn rss_run_reports_dogfood_rustc_remap_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-dogfood-rustc-remap-mismatch");
    let Some(fixture_dir) = prepare_dogfood_run_dir_for(&temp_dir, "dogfood-rustc-remap.rss")
    else {
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };
    let mut diagnostics_json = generated_dogfood_rustc_remap_json(&temp_dir);
    diagnostics_json[0]["code"] = Value::String("RS1102".to_string());
    fs::write(
        fixture_dir.join("dogfood-rustc-remap.json"),
        serde_json::to_vec(&diagnostics_json).expect("remap diagnostics JSON should serialize"),
    )
    .expect("mutated remap diagnostics should write");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("tests/fixtures/pass/dogfood-rustc-remap.rss")
        .current_dir(&temp_dir)
        .output()
        .expect("rss run should execute dogfood rustc remap classifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("dogfood rustc remap diagnostics=1 mapped=0 rustc_code=1 source_file=1 source_line=1 source_column=1"),
        "{stdout}"
    );
}

fn prepare_dogfood_run_dir(temp_dir: &Path) -> Option<PathBuf> {
    prepare_dogfood_run_dir_for(temp_dir, "dogfood-review-classifier.rss")
}

fn prepare_dogfood_run_dir_for(temp_dir: &Path, script_name: &str) -> Option<PathBuf> {
    fs::create_dir_all(temp_dir).expect("temporary root should be created");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime"),
            temp_dir.join("runtime"),
        )
        .expect("runtime crate symlink should be created");
    }
    #[cfg(not(unix))]
    {
        return None;
    }
    let fixture_dir = temp_dir.join("tests/fixtures/pass");
    fs::create_dir_all(&fixture_dir).expect("fixture directory should be created");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pass")
            .join(script_name),
        fixture_dir.join(script_name),
    )
    .expect("dogfood script should copy");
    Some(fixture_dir)
}

fn generated_dogfood_review_map_json() -> Value {
    let review_output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("review")
        .arg("--map")
        .arg("--json")
        .arg("tests/fixtures/pass/dogfood-review-classifier.rss")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss review --map --json should execute");
    assert!(
        review_output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&review_output.stdout),
        String::from_utf8_lossy(&review_output.stderr)
    );
    serde_json::from_slice(&review_output.stdout).expect("review map stdout should be JSON")
}

fn generated_dogfood_package_review_json(temp_dir: &Path) -> Value {
    let low_dir = temp_dir.join("package-low");
    write_empty_named_package_fixture(&low_dir, "rss-dogfood-low", "0.1.0", "");

    let elevated_dir = temp_dir.join("package-elevated");
    write_named_package_fixture(
        &elevated_dir,
        "rss-dogfood-elevated",
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );

    let high_dir = temp_dir.join("package-high");
    write_named_package_fixture(
        &high_dir,
        "rss-dogfood-high",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_dogfood_native"
build_scripts = "review"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );

    let unknown_dir = temp_dir.join("package-unknown");
    write_empty_named_package_fixture(
        &unknown_dir,
        "rss-dogfood-unknown",
        "0.1.0",
        r#"[review.expect]
risk = "unknown"
"#,
    );

    let feature_high_dir = temp_dir.join("package-feature-high");
    write_named_package_fixture(
        &feature_high_dir,
        "rss-dogfood-feature-high",
        "0.1.0",
        r#"[features]
native-tls = ["native"]
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    Value::Array(vec![
        package_review_json_for_dir(&low_dir),
        package_review_json_for_dir(&elevated_dir),
        package_review_json_for_dir(&high_dir),
        package_review_json_for_dir(&feature_high_dir),
        package_review_json_for_dir(&unknown_dir),
    ])
}

fn package_review_json_for_dir(package_dir: &Path) -> Value {
    let review = review_package_dir(package_dir).expect("package review should succeed");
    serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse")
}

fn generated_dogfood_package_exports_json(temp_dir: &Path) -> Value {
    let package_dir = temp_dir.join("package-exports");
    write_named_package_fixture(
        &package_dir,
        "rss-dogfood-exports",
        "0.1.0",
        "",
        r#"struct Image
resource DbConnection

pub fn Image.load(path: read String) -> fresh Image
pub fn Cache.store(conn: mut DbConnection, image: read Image) -> Unit
    effects(retains(image))
pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(package_dir.join("src")).expect("source dir should be created");
    fs::write(
        package_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    package_review_json_for_dir(&package_dir)
}

fn generated_dogfood_package_diff_json(temp_dir: &Path) -> Value {
    let old_dir = temp_dir.join("package-diff-old");
    let new_dir = temp_dir.join("package-diff-new");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[dependencies]
rss-core = "0.5"
rss-cache = "0.1"

[features]
native-tls = ["native"]

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
"#,
        r#"features: native

struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    );
    fs::write(
        new_dir.join("interface/cache.rssi"),
        r#"pub fn JsonCache.store(cache: mut JsonCache, value: read JsonValue) -> Unit
    effects(retains(value))
"#,
    )
    .expect("added high-risk interface should be written");
    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse")
}

fn generated_dogfood_package_lock_diff_json(temp_dir: &Path) -> Value {
    let lock_dir = temp_dir.join("package-lock-diff");
    fs::create_dir_all(&lock_dir).expect("lock diff dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    let old_lock = rsscript::PackageLock {
        version: 1,
        packages: vec![rsscript::PackageLockPackage {
            name: "rss-net".to_string(),
            version: "0.1.0".to_string(),
            source: "path:.".to_string(),
            checksum: "sha256:old".to_string(),
            interface_hash: "sha256:interface".to_string(),
            review_hash: "sha256:review".to_string(),
            native_hash: None,
            features: Vec::new(),
        }],
        metadata: rsscript::PackageLockMetadata {
            rsscript_version: "0.5".to_string(),
            created_by: "dogfood".to_string(),
        },
    };
    let mut new_lock = old_lock.clone();
    new_lock.packages[0].features = vec!["native-tls".to_string()];
    fs::write(&old_lock_path, format_package_lock_toml(&old_lock))
        .expect("old lock should be written");
    fs::write(&new_lock_path, format_package_lock_toml(&new_lock))
        .expect("new lock should be written");

    let diff =
        diff_package_locks(&old_lock_path, &new_lock_path).expect("lock diff should succeed");
    serde_json::from_str(&rsscript::format_package_lock_diff_json(&diff))
        .expect("package lock diff JSON should parse")
}

fn generated_dogfood_rustc_remap_json(temp_dir: &Path) -> Value {
    let package_dir = temp_dir.join("remap-package");
    write_named_package_fixture(
        &package_dir,
        "rss-dogfood-rustc-remap",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(package_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(package_dir.join("native/rust/src"))
        .expect("native src dir should be created");
    fs::create_dir_all(package_dir.join("native")).expect("native dir should be created");
    fs::write(
        package_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello")
    Log.write(message: read message)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        package_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        package_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        package_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: String) -> String { message }\n",
    )
    .expect("native source should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("run")
        .arg("--json")
        .arg(&package_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss run --json should execute");
    assert!(
        !output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).trim().is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("remap diagnostics JSON should parse")
}

#[test]
fn parser_accepts_bodyless_rssi_interface() {
    let source = r#"
struct JsonValue

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>

pub fn field(
    value: read JsonValue,
    name: read String,
) -> Result<fresh JsonValue, JsonError>

pub fn field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
"#;
    let program = parse_source("json.rssi", source);

    assert!(program.features.is_empty());
    assert_eq!(program.items.len(), 4);
    assert!(matches!(&program.items[0], Item::Type(type_decl) if type_decl.name == "JsonValue"));
    assert!(
        matches!(&program.items[1], Item::Function(function) if function.name == "parse" && function.is_public && function.body.statements.is_empty())
    );
    assert!(
        matches!(&program.items[2], Item::Function(function) if function.name == "field" && function.is_public && function.body.statements.is_empty())
    );
    assert!(
        matches!(&program.items[3], Item::Function(function) if function.name == "field_string" && function.is_public && function.body.statements.is_empty())
    );
    assert!(analyze_source("json.rssi", source).is_empty());
}

#[test]
fn parser_rejects_namespace_interface_shorthand() {
    let source = r#"
features: native
namespace Json

opaque struct JsonValue
opaque struct JsonError

native fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#;
    let program = parse_source("json.rssi", source);

    assert_eq!(program.unknown_top_level_spans.len(), 1);
    let diagnostics = analyze_source("json.rssi", source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "unsupported top-level item"
    }));
}

#[test]
fn parser_preserves_async_function_kind() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>
"#;
    let program = parse_source("net.rssi", source);

    assert!(
        matches!(&program.items[0], Item::Function(function) if function.name == "fetch" && function.is_async)
    );
    assert!(analyze_source("net.rssi", source).is_empty());
}

#[test]
fn parser_preserves_spawn_expression() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn schedule(url: read Url) -> Unit {
    let task = spawn fetch(url: read url)
    return Unit
}
"#;
    let program = parse_source("spawn.rss", source);
    let schedule = program
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(function) if function.name == "schedule" => Some(function),
            _ => None,
        })
        .expect("expected schedule function");
    let stmt = schedule
        .body
        .statements
        .first()
        .expect("expected task binding");

    assert!(matches!(
        stmt,
        rsscript::syntax::ast::Stmt::Let(let_stmt)
            if matches!(let_stmt.value.as_ref(), Some(Expr::Spawn { .. }))
    ));
}

#[test]
fn parser_preserves_await_expression() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = await fetch(url: read url)
    return Unit
}
"#;
    let program = parse_source("await.rss", source);
    let receive = program
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(function) if function.name == "receive" => Some(function),
            _ => None,
        })
        .expect("expected receive function");
    let stmt = receive
        .body
        .statements
        .first()
        .expect("expected bytes binding");

    assert!(matches!(
        stmt,
        rsscript::syntax::ast::Stmt::Let(let_stmt)
            if matches!(let_stmt.value.as_ref(), Some(Expr::Await { .. }))
    ));
}

#[test]
fn checker_reports_async_bodies_as_unsupported_until_async_lowering_exists() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError> {
    return Network.fetch(url: read url)
}
"#;
    let diagnostics = analyze_source("async-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported async function body")
    );
}

#[test]
fn checker_reports_await_as_unsupported_until_async_lowering_exists() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = await fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported await expression"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_spawn_as_unsupported_until_async_lowering_exists() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn schedule(url: read Url) -> Unit {
    let task = spawn fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("spawn-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported spawn expression")
    );
}

#[test]
fn checker_rejects_async_call_without_await_or_spawn() {
    let source = r#"
features: async

async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("async-call-direct.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0022"
                && diagnostic.label == "async call must be awaited or spawned"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_gates_async_interface_calls_on_async_feature() {
    let interface = r#"
async fn Http.get(url: read Url) -> Result<fresh Bytes, NetworkError>
"#;
    let source = r#"
fn receive(url: read Url) -> Unit {
    let bytes = Http.get(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source_with_interfaces(
        "async-interface-call.rss",
        source,
        &[("net.rssi", interface)],
    );

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0101" && diagnostic.summary.contains("async")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0022"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_gates_await_on_async_feature() {
    let source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn receive(url: read Url) -> Unit {
    let bytes = await fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-feature.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0101" && diagnostic.summary.contains("await")),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_gates_spawn_on_async_feature() {
    let source = r#"
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>

fn schedule(url: read Url) -> Unit {
    let task = spawn fetch(url: read url)
    return Unit
}
"#;
    let diagnostics = analyze_source("spawn-feature.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0101" && diagnostic.summary.contains("spawn")),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_spawn_capturing_local_value() {
    let source = r#"
features: async, local

struct Image

fn work(image: read Image) -> Unit

fn schedule(path: read Path) -> Unit {
    local image = Image()
    let task = spawn work(image: read image)
    return Unit
}
"#;
    let diagnostics = analyze_source("spawn-local.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0501"
                && diagnostic.label == "local captured by spawn"),
        "{diagnostics:?}"
    );
}

#[test]
fn parser_accepts_native_function_declaration() {
    let source = r#"
features: native

native fn Host.emit(message: read String) -> Unit
    effects(native)
"#;
    let program = parse_source("host.rssi", source);

    let Item::Function(function) = &program.items[0] else {
        panic!("expected native function declaration");
    };
    assert_eq!(function.name, "Host.emit");
    assert!(function.is_native);
    assert!(
        function
            .effects
            .iter()
            .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "native"))
    );
    assert!(analyze_source("host.rssi", source).is_empty());
}

#[test]
fn checker_reports_native_bodies_as_unsupported_until_native_binding_exists() {
    let source = r#"
features: native

native fn Host.emit(message: read String) -> Unit
    effects(native)
{
    return Unit
}
"#;
    let diagnostics = analyze_source("native-body.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported native function body")
    );
}

#[test]
fn checker_rejects_unknown_file_features() {
    let source = r#"
features: local, locall

fn main() -> Unit {
    return Unit
}
"#;
    let program = parse_source("features.rss", source);

    assert_eq!(program.features.len(), 1);
    assert_eq!(program.unknown_features.len(), 1);
    assert_eq!(program.unknown_features[0].name, "locall");
    let diagnostics = analyze_source("features.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0016")
    );
}

#[test]
fn checker_rejects_duplicate_file_features() {
    let source = r#"
features: local, local

fn main() -> Unit {
    return Unit
}
"#;
    let program = parse_source("features.rss", source);

    assert_eq!(program.features.len(), 2);
    assert_eq!(program.duplicate_features.len(), 1);
    assert_eq!(program.duplicate_features[0].name, "local");
    let diagnostics = analyze_source("features.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0017")
    );
}

#[test]
fn checker_accepts_noescape_callback_that_temporarily_uses_local() {
    let source = r#"
features: local

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn use_local(path: read Path) -> Result<fresh Image, ImageError> {
    local image = Image.load(path: read path)?
    apply(callback: || {
        Image.inspect(image: read image)
    })
    return Ok(image)
}

fn main() -> Result<Unit, ImageError> {
    let path = Path.from_string(value: read "rsscript-image-input.bin")
    use_local(path: read path)?
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("noescape.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let program = parse_source("noescape.rss", source);
    let function = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .find(|function| function.name == "apply")
        .expect("apply should parse");
    assert!(function.params[0].ty.is_noescape);
    assert_eq!(function.params[0].ty.name, "Fn");

    let lowered = lower_source_to_rust("noescape.rss", source)
        .expect("noescape callback source should lower");
    assert!(lowered.contains("mut callback: impl FnMut()"));
    assert!(lowered.contains("callback();"));
}

#[test]
fn rust_lowering_noescape_callbacks_are_non_consuming_fnmut() {
    let source = r#"
features: local

fn apply_twice(callback: noescape Fn()) -> Unit {
    callback()
    callback()
    return Unit
}

fn use_local_buffer() -> Unit {
    local buffer = Buffer.new(size: 16)
    apply_twice(callback: || {
        Buffer.clear(buffer: mut buffer)
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("noescape-twice.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("noescape-twice.rss", source)
        .expect("noescape callback source should lower");
    assert!(lowered.contains("fn apply_twice(mut callback: impl FnMut())"));
    assert_eq!(lowered.matches("callback();").count(), 2);
}

#[test]
fn rust_lowering_marks_local_closure_mut_when_it_mutates_capture() {
    let source = r#"
features: local

fn run() -> Unit {
    local buffer = Buffer.new(size: 16)
    local callback = || {
        Buffer.clear(buffer: mut buffer)
    }
    callback()
    return Unit
}
"#;
    let diagnostics = analyze_source("local-closure-fnmut.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("local-closure-fnmut.rss", source)
        .expect("local closure source should lower");
    assert!(lowered.contains("let mut callback = ||"));
    assert!(lowered.contains("callback();"));
}

#[test]
fn parser_preserves_noescape_function_return_types() {
    let source = r#"fn build(create: noescape Fn() -> Result<DbConnection, DbError>) -> Unit"#;
    let program = parse_source("fn-return-type.rssi", source);
    let function = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .find(|function| function.name == "build")
        .expect("function should parse");
    let create = &function.params[0].ty;

    assert!(create.is_noescape);
    assert_eq!(create.name, "Fn");
    assert_eq!(
        create
            .fn_return
            .as_ref()
            .map(|return_ty| return_ty.name.as_str()),
        Some("Result")
    );
    assert_eq!(
        create
            .fn_return
            .as_ref()
            .map(|return_ty| return_ty.args.len()),
        Some(2)
    );
}

#[test]
fn resource_pool_core_signatures_use_typed_noescape_factories() {
    let program = core_interfaces()
        .iter()
        .find_map(|(file, source)| {
            (file.contains("resource_pool.rssi")).then(|| parse_source(file, source))
        })
        .expect("ResourcePool interface should be available");
    let functions = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    let new_signature = functions
        .iter()
        .find(|function| function.name == "ResourcePool.new")
        .expect("ResourcePool.new should be available from core interfaces");
    let new_create = new_signature
        .params
        .iter()
        .find(|param| param.name == "create")
        .expect("ResourcePool.new should have create parameter");
    let try_new_signature = functions
        .iter()
        .find(|function| function.name == "ResourcePool.try_new")
        .expect("ResourcePool.try_new should be available from core interfaces");
    let try_create = try_new_signature
        .params
        .iter()
        .find(|param| param.name == "create")
        .expect("ResourcePool.try_new should have create parameter");

    assert!(new_create.ty.is_noescape);
    assert_eq!(new_create.ty.name, "Fn");
    assert_eq!(
        new_create
            .ty
            .fn_return
            .as_ref()
            .map(|return_ty| return_ty.name.as_str()),
        Some("T")
    );
    assert!(try_create.ty.is_noescape);
    assert_eq!(try_create.ty.name, "Fn");
    assert_eq!(
        try_create
            .ty
            .fn_return
            .as_ref()
            .map(|return_ty| return_ty.name.as_str()),
        Some("Result")
    );
}

#[test]
fn checker_accepts_exhaustive_option_match() {
    let source = r#"
fn pick() -> Int {
    let value = Some(42)
    match value {
        Some(result) => return result
        None => return 0
    }
}
"#;
    let diagnostics = analyze_source("match.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("match.rss", source).expect("match should lower");
    assert!(lowered.contains("match value"));
    assert!(lowered.contains("Some(result) =>"));
    assert!(lowered.contains("None =>"));
}

#[test]
fn checker_reports_non_exhaustive_option_match() {
    let source = r#"
fn pick() -> Int {
    let value = Some(42)
    match value {
        Some(result) => return result
    }
}
"#;
    let diagnostics = analyze_source("match.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0021" && diagnostic.label == "non-exhaustive match"
    ));
}

#[test]
fn checker_reports_for_as_unsupported_until_iteration_lowering_exists() {
    let source = r#"
fn run(items: read List<Int>) -> Unit {
    for item in items {
        Log.write(message: read "x")
    }
}
"#;
    let diagnostics = analyze_source("for.rss", source);

    assert!(diagnostics.iter().any(
        |diagnostic| diagnostic.code == "RS0015" && diagnostic.label == "unsupported statement"
    ));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0206")
    );
}

#[test]
fn review_map_marks_public_rssi_signatures_review_required() {
    let source = r#"
struct JsonValue

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#;
    let map = review_map_sources(vec![("json.rssi", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "parse")
        .expect("expected parse function in review map");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "public entry point")
    );
}

#[test]
fn review_map_marks_unknown_qualified_calls_unknown() {
    let source = r#"
fn delegated(value: read Int) -> Int {
    return Mystery.run(value: read value)
}
"#;
    let map = review_map_sources(vec![("map.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "delegated")
        .expect("expected delegated function in review map");

    assert_eq!(region.classification, ReviewMapClassification::Unknown);
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason.contains("Mystery.run"))
    );
}

#[test]
fn review_map_marks_public_direct_unknown_calls_unknown() {
    let source = r#"
pub fn run(value: read Int) -> Int {
    return Mystery.run(value: read value)
}
"#;
    let map = review_map_sources(vec![("public-unknown.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "run")
        .expect("expected run function in review map");

    assert_eq!(region.classification, ReviewMapClassification::Unknown);
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "public entry point")
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason.contains("Mystery.run"))
    );
    assert_eq!(map.summary.unknown.functions, 1);
}

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
fn review_map_marks_callers_of_unknown_functions_unknown() {
    let source = r#"
fn delegated(value: read Int) -> Int {
    return Mystery.run(value: read value)
}

fn wrapper(value: read Int) -> Int {
    return delegated(value: read value)
}
"#;
    let map = review_map_sources(vec![("unknown-call.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.unknown.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "wrapper"
            && region.classification == ReviewMapClassification::Unknown
            && region
                .reasons
                .iter()
                .any(|reason| reason == "calls unknown `delegated`")
    }));
}

#[test]
fn review_map_marks_private_entry_functions_review_required() {
    let source = r#"
fn helper(value: read Int) -> Int {
    return value
}

fn main() -> Unit {
    helper(value: read 1)
}

fn handle_request(request: read Request) -> Response {
    return Response.ok()
}
"#;
    let map = review_map_sources(vec![("entry.rss", source)]);

    assert_eq!(map.summary.total_functions, 3);
    assert_eq!(map.summary.review_required.functions, 2);
    assert_eq!(map.summary.foldable.functions, 1);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "main"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region.reasons.iter().any(|reason| reason == "entry point")
    }));
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "handle_request"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region.reasons.iter().any(|reason| reason == "entry point")
    }));
}

#[test]
fn review_map_marks_callers_of_review_required_functions() {
    let source = r#"
fn store(value: read Payload) -> Unit
    effects(retains(value))
{
    return Unit
}

fn wrapper(value: read Payload) -> Unit {
    store(value: read value)
}
"#;
    let map = review_map_sources(vec![("calls.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "wrapper"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "calls must-review `store`")
    }));
}

#[test]
fn review_map_marks_callers_of_qualified_review_required_functions() {
    let source = r#"
struct Payload
struct Cache

fn Cache.remember(cache: read Cache, value: read Payload) -> Unit
    effects(retains(value))

fn wrapper(cache: read Cache, value: read Payload) -> Unit {
    Cache.remember(cache: read cache, value: read value)
}
"#;
    let map = review_map_sources(vec![("qualified-calls.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert_eq!(map.summary.foldable.functions, 0);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "wrapper"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "calls must-review `Cache.remember`")
    }));
}

#[test]
fn review_map_marks_resourcepool_and_fresh_boundaries() {
    let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Image {
    pixels: Buffer
}

struct Buffer

fn make_image() -> fresh Image {
    return Image(pixels: Buffer())
}

fn pooled(pool: mut ResourcePool<DbConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        return Unit
    }
}
"#;
    let map = review_map_sources(vec![("resourcepool.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "make_image"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "fresh guarantee boundary")
    }));
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "pooled"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "ResourcePool usage")
    }));
}

#[test]
fn review_map_marks_runtime_guarantee_boundaries() {
    let source = r#"
fn checksum(data: read Bytes) -> UInt64
    effects(noalloc, no_panic)
{
    return 1
}

fn pure_helper(value: read Int) -> Int
    effects(pure)
{
    return value
}
"#;
    let map = review_map_sources(vec![("guarantees.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert_eq!(map.summary.foldable.functions, 0);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "checksum"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "guarantee `noalloc`")
            && region
                .reasons
                .iter()
                .any(|reason| reason == "guarantee `no_panic`")
    }));
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "pure_helper"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "guarantee `pure`")
    }));
}

#[test]
fn review_map_marks_native_calls_as_review_required_boundaries() {
    let source = r#"
features: native

native fn Native.echo(message: read String) -> String
    effects(native)

fn caller(message: read String) -> String {
    return Native.echo(message: read message)
}
"#;
    let map = review_map_sources(vec![("native-call-map.rss", source)]);

    assert_eq!(map.summary.total_functions, 2);
    assert_eq!(map.summary.review_required.functions, 2);
    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "caller"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "native call `Native.echo`")
    }));
}

#[test]
fn review_map_marks_error_handling_boundaries() {
    let source = r#"
fn may_fail() -> Result<Unit, IOError>

fn load() -> Result<Unit, IOError> {
    may_fail()?
    return Ok(Unit)
}
"#;
    let map = review_map_sources(vec![("error-boundary.rss", source)]);

    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "load"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "error handling boundary")
    }));
}

#[test]
fn review_map_marks_mut_call_site_effects() {
    let source = r#"
struct Counter {
    value: Int
}

fn bump(counter: mut Counter) -> Unit

fn update(counter: read Counter) -> Unit {
    bump(counter: mut counter)
}
"#;
    let map = review_map_sources(vec![("mut-call-site.rss", source)]);

    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "update"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "mut call-site effect")
    }));
}

#[test]
fn review_map_marks_writes_to_managed_state() {
    let source = r#"
features: local

struct Counter {
    value: Int
}

fn bump(counter: mut Counter) -> Unit

fn update_managed(counter: read Counter) -> Unit {
    bump(counter: mut counter)
}

fn update_local() -> Unit {
    local counter = Counter(value: 1)
    bump(counter: mut counter)
}
"#;
    let map = review_map_sources(vec![("managed-write.rss", source)]);

    let managed = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "update_managed")
        .expect("expected managed update region");
    assert_eq!(
        managed.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        managed
            .reasons
            .iter()
            .any(|reason| reason == "writes to managed state")
    );

    let local = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "update_local")
        .expect("expected local update region");
    assert!(
        local
            .reasons
            .iter()
            .all(|reason| reason != "writes to managed state")
    );
}

#[test]
fn review_map_marks_writes_through_handle_fields() {
    let source = r#"
class Cache {
    value: Int
}

struct State {
    cache: handle Cache
}

fn touch(cache: mut Cache) -> Unit

fn update(state: read State) -> Unit {
    touch(cache: mut state.cache)
}
"#;
    let map = review_map_sources(vec![("handle-write.rss", source)]);

    assert!(map.files[0].regions.iter().any(|region| {
        region.function == "update"
            && region.classification == ReviewMapClassification::ReviewRequired
            && region
                .reasons
                .iter()
                .any(|reason| reason == "writes through handle field")
    }));
}

#[test]
fn review_map_reports_file_features() {
    let source = r#"
features: local, native, device, ffi, reflection

fn process() -> Unit {
    return Unit
}
"#;
    let map = review_map_sources(vec![("features.rss", source)]);

    assert_eq!(
        map.files[0].features,
        vec!["device", "ffi", "local", "native", "reflection"]
    );
    assert_eq!(map.files[0].risk, ReviewMapFileRisk::High);
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "local capability enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "native boundary capability enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "reserved device review marker enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "reserved ffi review marker enabled")
    );
    assert!(
        map.files[0]
            .reasons
            .iter()
            .any(|reason| reason == "reserved reflection review marker enabled")
    );
    let human = format_review_map_human(&map);
    assert!(
        human.contains("features.rss: features device, ffi, local, native, reflection; risk high")
    );
    let json: Value =
        serde_json::from_str(&format_review_map_json(&map)).expect("review map JSON should parse");
    assert_eq!(json["files"][0]["features"][0], "device");
    assert_eq!(json["files"][0]["features"][1], "ffi");
    assert_eq!(json["files"][0]["features"][2], "local");
    assert_eq!(json["files"][0]["features"][3], "native");
    assert_eq!(json["files"][0]["features"][4], "reflection");
    assert_eq!(json["files"][0]["risk"], "high");
    assert!(
        json["files"][0]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native boundary capability enabled"))
    );
    assert!(
        json["files"][0]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "reserved ffi review marker enabled"))
    );
}

#[test]
fn package_review_reads_manifest_and_reports_semantic_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-review");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-json"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[sources]
paths = ["src"]

[dependencies]
rss-core = "0.5"

[features]
streaming = []

[review.expect]
risk = "low"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
proc_macros = "forbid"
unsafe = "forbid"
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/json.rssi"),
        r#"features: native

struct JsonValue
struct JsonError

native fn Json.parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    )
    .expect("interface should be written");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn helper(text: read String) -> String {
    return text
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["package"]["name"], "rss-json");
    assert_eq!(json["risk"], "high");
    assert_eq!(json["features"], serde_json::json!(["streaming"]));
    assert_eq!(json["summary"]["interface_files"], 1);
    assert_eq!(json["summary"]["source_files"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust wrapper enabled")
    }));
    assert!(human.contains("package features: streaming"));
}

#[test]
fn package_review_reports_feature_boundary_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-feature-risk");
    write_named_package_fixture(
        &temp_dir,
        "rss-feature-risk",
        "0.1.0",
        r#"[features]
native-tls = ["native"]
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "high");
    assert_eq!(json["features"], serde_json::json!(["native-tls"]));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason == "package feature `native-tls` may change native/unsafe/build risk"
        })
    }));
}

#[test]
fn package_review_selects_feature_conditioned_interface_paths() {
    let temp_dir = unique_temp_dir("rsscript-package-feature-interface");
    write_named_package_fixture(
        &temp_dir,
        "rss-feature-interface",
        "0.1.0",
        r#"[features]
streaming = []

[interfaces.features.streaming]
paths = ["interface/streaming"]
"#,
        r#"pub fn Json.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("interface/streaming"))
        .expect("feature interface dir should be created");
    fs::write(
        temp_dir.join("interface/streaming/lib.rssi"),
        r#"pub fn Json.stream(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["interface_files"], 2);
    assert_eq!(json["summary"]["public_functions"], 2);
    assert!(
        json["exports"].as_array().is_some_and(|exports| {
            exports.iter().any(|export| export["name"] == "Json.stream")
        })
    );
}

#[test]
fn package_review_loads_path_dependency_interfaces_for_source_checks() {
    let root_dir = unique_temp_dir("rsscript-package-dep-interface-root");
    let dep_dir = unique_temp_dir("rsscript-package-dep-interface-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.parse(text: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(review.summary.source_files, 1);
    assert_eq!(review.diagnostics, Vec::new());
}

#[test]
fn package_review_uses_selected_dependency_feature_interfaces_for_source_checks() {
    let root_dir = unique_temp_dir("rsscript-package-dep-feature-interface-root");
    let dep_dir = unique_temp_dir("rsscript-package-dep-feature-interface-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = ["simd"]
simd = []

[interfaces.features.fast]
paths = ["interface/fast"]

[interfaces.features.simd]
paths = ["interface/simd"]
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(dep_dir.join("interface/fast"))
        .expect("feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/fast/lib.rssi"),
        r#"pub fn Dep.fast(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");
    fs::create_dir_all(dep_dir.join("interface/simd"))
        .expect("transitive feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/simd/lib.rssi"),
        r#"pub fn Dep.simd(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            dep_dir.display()
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.simd(text: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(review.diagnostics, Vec::new());
}

#[test]
fn package_check_reports_unknown_selected_dependency_feature() {
    let root_dir = unique_temp_dir("rsscript-package-dep-unknown-feature-root");
    let dep_dir = unique_temp_dir("rsscript-package-dep-unknown-feature-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["turbo"] }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0101" && diagnostic["label"] == "unknown package feature"
        })
    }));
}

#[test]
fn package_review_reports_missing_interface_implementation() {
    let temp_dir = unique_temp_dir("rsscript-package-missing-interface-impl");
    write_named_package_fixture(
        &temp_dir,
        "rss-missing-impl",
        "0.1.0",
        "",
        r#"pub fn render(body: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn helper(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(!check.ok);
}

#[test]
fn package_review_reports_interface_implementation_signature_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-package-interface-mismatch");
    write_named_package_fixture(
        &temp_dir,
        "rss-interface-mismatch",
        "0.1.0",
        "",
        r#"pub fn render(body: read String) -> fresh String
    effects(no_panic)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub fn render(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(
        causes.iter().any(|cause| cause.contains(
            "interface: pub fn render(body: read String) -> fresh String effects(no_panic)"
        )),
        "{causes:?}"
    );
    assert!(
        causes
            .iter()
            .any(|cause| cause.contains("source: pub fn render(body: read String) -> String")),
        "{causes:?}"
    );
}

#[test]
fn package_review_reports_missing_interface_type_declaration() {
    let temp_dir = unique_temp_dir("rsscript-package-missing-interface-type");
    write_named_package_fixture(
        &temp_dir,
        "rss-missing-type",
        "0.1.0",
        "",
        r#"struct PublicConfig {
    name: String
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(!check.ok);
}

#[test]
fn package_review_reports_interface_type_contract_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-package-interface-type-mismatch");
    write_named_package_fixture(
        &temp_dir,
        "rss-type-mismatch",
        "0.1.0",
        "",
        r#"class Session<T: Managed> {
    user: handle User
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"struct Session<T: Managed> {
    user: User
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(
        causes
            .iter()
            .any(|cause| cause
                .contains("interface: class Session<T: Managed> { user: handle User }")),
        "{causes:?}"
    );
    assert!(
        causes
            .iter()
            .any(|cause| cause.contains("source: struct Session<T: Managed> { user: User }")),
        "{causes:?}"
    );
}

#[test]
fn package_review_rejects_namespace_interface_shorthand() {
    let temp_dir = unique_temp_dir("rsscript-package-namespace-opaque-interface");
    write_named_package_fixture(
        &temp_dir,
        "rss-namespace-opaque",
        "0.1.0",
        r#"[sources]
paths = ["src"]
"#,
        r#"namespace Json

opaque struct JsonValue
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"struct Json.JsonValue {
    text: String
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should complete");
    let _ = fs::remove_dir_all(&temp_dir);

    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"RS0015"), "{:?}", review.diagnostics);
}

#[test]
fn package_review_reports_path_dependency_interface_call_violations() {
    let root_dir = unique_temp_dir("rsscript-package-dep-interface-violation-root");
    let dep_dir = unique_temp_dir("rsscript-package-dep-interface-violation-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.parse(value: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(codes.contains(&"RS0203"), "{codes:?}");
    assert!(codes.contains(&"RS0204"), "{codes:?}");
    assert!(!codes.contains(&"RS0206"), "{codes:?}");
}

#[test]
fn package_review_reports_dependency_interface_symbol_conflicts_without_sources() {
    let root_dir = unique_temp_dir("rsscript-package-interface-conflict-root");
    let dep_dir = unique_temp_dir("rsscript-package-interface-conflict-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Shared.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn Shared.parse(text: read String) -> String
"#,
    );

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(codes.contains(&"RS0005"), "{codes:?}");
}

#[test]
fn package_review_includes_lint_warnings_for_public_contracts() {
    let temp_dir = unique_temp_dir("rsscript-package-review-lint");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: native

struct Error

pub fn Api.overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Image>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Image>>>, Error>
    effects(no_panic, noalloc, no_block, pure, native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["errors"], 0);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package contains frontend warnings")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "RSL001"
                && diagnostic["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("7 parameters"))
        })
    }));
}

#[test]
fn package_review_explains_manifest_unknown_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-review-manifest-unknown");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.expect]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "manifest declares unknown package risk")
    }));
}

#[test]
fn rss_pkg_review_json_reports_package_metadata() {
    let temp_dir = unique_temp_dir("rsscript-package-review-cli");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-math"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/math.rssi"),
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    )
    .expect("interface should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("review")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg review should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package review JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["package"]["name"], "rss-math");
    assert_eq!(json["summary"]["interface_files"], 1);
}

#[test]
fn package_review_json_counts_native_and_unsafe_apis_separately() {
    let temp_dir = unique_temp_dir("rsscript-package-review-api-effects");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native, unsafe

native fn Native.echo(message: read String) -> String
    effects(native)
fn Native.danger(message: read String) -> String
    effects(unsafe)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["native_apis"], 1);
    assert_eq!(json["summary"]["unsafe_apis"], 1);
}

#[test]
fn package_review_json_counts_public_api_review_categories() {
    let temp_dir = unique_temp_dir("rsscript-package-review-api-categories");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"struct Image
resource DbConnection

pub fn Image.load(path: read String) -> fresh Image
pub fn Cache.store(conn: mut DbConnection, image: read Image) -> Unit
    effects(retains(image))
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_types"], 2);
    assert_eq!(json["summary"]["public_functions"], 2);
    assert_eq!(json["summary"]["public_apis"], 4);
    assert_eq!(json["summary"]["mutating_apis"], 1);
    assert_eq!(json["summary"]["retaining_apis"], 1);
    assert_eq!(json["summary"]["resource_apis"], 1);
    assert_eq!(json["summary"]["fresh_returning_apis"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 0);
    let exports = json["exports"]
        .as_array()
        .expect("exports should be an array");
    assert!(exports.iter().any(|export| {
        export["name"] == "DbConnection"
            && export["kind"] == "type"
            && export["reasons"]
                .as_array()
                .is_some_and(|reasons| reasons.iter().any(|reason| reason == "resource type"))
    }));
    assert!(exports.iter().any(|export| {
        export["name"] == "Cache.store"
            && export["kind"] == "function"
            && export["classification"] == "review_if_changed"
            && export["reasons"].as_array().is_some_and(|reasons| {
                reasons
                    .iter()
                    .any(|reason| reason == "mut parameter `conn`")
                    && reasons.iter().any(|reason| reason == "retains(image)")
                    && reasons
                        .iter()
                        .any(|reason| reason == "resource parameter `conn`")
            })
    }));
    assert!(human.contains("exports:"));
    assert!(human.contains("function Cache.store: review_if_changed"));
    assert!(human.contains("retains(image)"));
    assert!(human.contains("type DbConnection: review_if_changed"));
}

#[test]
fn package_review_json_counts_public_apis_with_unknown_review_regions() {
    let temp_dir = unique_temp_dir("rsscript-package-review-unknown-api");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    helper()
    return Unit
}

fn helper() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 2);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "unknown review-map region")
                })
        })
    }));
    assert!(human.contains("function Api.run: unknown"));
    assert!(human.contains("unknown review-map region"));
}

#[test]
fn package_review_json_counts_public_api_with_direct_unknown_call() {
    let temp_dir = unique_temp_dir("rsscript-package-review-direct-unknown-api");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 1);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "unknown review-map region")
                })
        })
    }));
}

#[test]
fn package_check_fails_unknown_review_when_configured_as_error() {
    let temp_dir = unique_temp_dir("rsscript-package-check-unknown-is-error");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[sources]
paths = ["src"]

[review.expect]
risk = "unknown"

[review.policy]
deny_unknown = true
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["lock"]["matches"], true);
    assert_eq!(json["summary"]["errors"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package policy denies unknown review risk")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "deny_unknown"
        })
    }));
}

#[test]
fn package_manifest_rejects_legacy_review_policy_aliases() {
    let temp_dir = unique_temp_dir("rsscript-package-review-policy-alias");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review]
unknown_is_error = true
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let error =
        review_package_dir(&temp_dir).expect_err("legacy review aliases should be rejected");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(error.contains("unknown field"), "{error}");
    assert!(error.contains("unknown_is_error"), "{error}");
}

#[test]
fn package_check_fails_when_policy_denies_native_api() {
    let temp_dir = unique_temp_dir("rsscript-package-check-deny-native");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
deny_native = true
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package policy denies native public APIs")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "deny_native"
        })
    }));
}

#[test]
fn package_check_fails_when_policy_denies_unsafe_api() {
    let temp_dir = unique_temp_dir("rsscript-package-check-deny-unsafe");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
deny_unsafe_apis = true
"#,
        r#"features: unsafe

fn Native.danger(message: read String) -> String
    effects(unsafe)
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package policy denies unsafe public APIs")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "deny_unsafe_apis"
        })
    }));
}

#[test]
fn package_check_applies_public_signature_policy_limits() {
    let temp_dir = unique_temp_dir("rsscript-package-check-signature-policy");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
max_public_params = 2
max_nested_type_depth = 2
"#,
        r#"struct Error

pub fn Api.run(
    first: read String,
    second: read String,
    third: read String,
) -> Result<List<String>, Error>
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "high");
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "max_public_params"
        }) && diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "max_nested_type_depth"
        })
    }));
}

#[test]
fn package_review_policy_maps_native_api_risk_to_elevated() {
    let temp_dir = unique_temp_dir("rsscript-package-native-api-risk");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
native_api_risk = "elevated"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "elevated");
    assert_eq!(json["summary"]["native_apis"], 1);
}

#[test]
fn package_check_reports_invalid_review_policy_values() {
    let temp_dir = unique_temp_dir("rsscript-package-invalid-review-policy");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
native_api_risk = "low"
build_execution_default = "sometimes"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "native_api_risk"
        }) && diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "build_execution_default"
        })
    }));
}

#[test]
fn package_check_applies_build_execution_default_to_native_wrapper() {
    let temp_dir = unique_temp_dir("rsscript-package-build-default-forbid");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
build_execution_default = "forbid"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    let _ = std::env::var("OUT_DIR");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_env_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script reads environment"))
    );
}

#[test]
fn package_review_uses_build_execution_default_as_native_review_policy() {
    let temp_dir = unique_temp_dir("rsscript-package-build-default-review");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
build_execution_default = "review"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["native_rust"]["build_scripts"], "review");
    assert_eq!(json["native_rust"]["proc_macros"], "review");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
            && reasons
                .iter()
                .any(|reason| reason == "native Rust proc macros require review")
    }));
}

#[test]
fn package_review_marks_broken_rssi_contract_diagnostics_unknown() {
    let temp_dir = unique_temp_dir("rsscript-package-review-broken-rssi");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"fn (value: read String) -> Unit
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "public .rssi contract contains frontend errors")
    }));
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["kind"] == "contract_diagnostic"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "frontend error RS0015")
                })
        })
    }));
    assert!(human.contains("contract_diagnostic interface/lib.rssi:1:1: unknown"));
}

#[test]
fn package_metadata_dry_run_reports_review_metadata_without_writing() {
    let temp_dir = unique_temp_dir("rsscript-package-metadata-dry-run");
    write_named_package_fixture(
        &temp_dir,
        "rss-metadata",
        "0.1.0",
        r#"[features]
fast = []
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let metadata = package_metadata(&temp_dir, true).expect("metadata dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&metadata))
        .expect("metadata JSON should parse");
    let metadata_path_exists = temp_dir.join("review").join("package-review.json").exists();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(metadata.ok);
    assert!(!metadata_path_exists);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["written"], false);
    assert_eq!(json["metadata"]["schema"], "rss.review.package.v1");
    assert_eq!(json["metadata"]["package"]["name"], "rss-metadata");
    assert_eq!(json["metadata"]["features"], serde_json::json!(["fast"]));
}

#[test]
fn package_metadata_reports_unknown_review_risk_not_ok() {
    let temp_dir = unique_temp_dir("rsscript-package-metadata-unknown-risk");
    write_named_package_fixture(
        &temp_dir,
        "rss-metadata-unknown",
        "0.1.0",
        r#"[review.expect]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let metadata = package_metadata(&temp_dir, true).expect("metadata dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&metadata))
        .expect("metadata JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!metadata.ok);
    assert_eq!(json["ok"], false);
    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["metadata"]["summary"]["errors"], 0);
}

#[test]
fn rss_package_command_is_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("package")
        .arg("check")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss package command should execute");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr={stderr}");
    assert!(
        stderr.contains("unknown command `package`; use `rsscript pkg ...`."),
        "{stderr}"
    );
    assert!(stderr.contains("rsscript pkg check"), "{stderr}");
    assert!(!stderr.contains("rsscript package"), "{stderr}");
}

#[test]
fn docs_do_not_reintroduce_legacy_gc_runtime_surface() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let legacy_runtime_name = ["runtime ", "G", "c"].concat();
    let legacy_runtime_path = ["rsscript_runtime::", "G", "c"].concat();
    let legacy_review_category = ["safe", "_to_", "skip"].concat();

    for relative_path in [
        "README.md",
        "RSScript_v0.5_Spec.md",
        "RSScript_Package_Manager_Design_v0.5.md",
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
fn rss_pkg_metadata_json_writes_review_metadata_file() {
    let temp_dir = unique_temp_dir("rsscript-package-metadata-cli");
    write_named_package_fixture(
        &temp_dir,
        "rss-metadata",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("metadata")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg metadata should execute");
    let metadata_path = temp_dir.join("review").join("package-review.json");
    let metadata_source =
        fs::read_to_string(&metadata_path).expect("metadata file should be written");
    let metadata_file_json: Value =
        serde_json::from_str(&metadata_source).expect("metadata file JSON should parse");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be metadata JSON");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["written"], true);
    assert_eq!(json["metadata"]["schema"], "rss.review.package.v1");
    assert_eq!(metadata_file_json["schema"], "rss.review.package.v1");
    assert_eq!(metadata_file_json["package"]["name"], "rss-metadata");
}

#[test]
fn package_diff_reports_manifest_and_interface_contract_changes() {
    let old_dir = unique_temp_dir("rsscript-package-diff-old");
    let new_dir = unique_temp_dir("rsscript-package-diff-new");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[dependencies]
rss-core = "0.5"
rss-cache = "0.1"

[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
"#,
        r#"features: native

struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["new_package"]["version"], "0.2.0");
    assert_eq!(json["risk"], "high");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "dependency" && change["name"] == "rss-cache")
    }));
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "native-rust" && change["name"] == "build_scripts")
    }));
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["file"] == "interface/lib.rssi" && change["risk"] == "high")
    }));
}

#[test]
fn package_diff_marks_added_boundary_interface_files_high_risk() {
    let old_dir = unique_temp_dir("rsscript-package-added-boundary-interface-old");
    let new_dir = unique_temp_dir("rsscript-package-added-boundary-interface-new");
    write_named_package_fixture(
        &old_dir,
        "rss-added-interface",
        "0.1.0",
        "",
        r#"struct Cache
struct Bytes

pub fn Cache.get(cache: read Cache) -> Bytes
"#,
    );
    write_named_package_fixture(
        &new_dir,
        "rss-added-interface",
        "0.1.0",
        "",
        r#"struct Cache
struct Bytes

pub fn Cache.get(cache: read Cache) -> Bytes
"#,
    );
    fs::write(
        new_dir.join("interface/retention.rssi"),
        r#"pub fn Cache.put(cache: mut Cache, value: read Bytes) -> Unit
    effects(retains(value))
"#,
    )
    .expect("added boundary interface should be written");

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "high-risk interface change detected")
    }));
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/retention.rssi"
                && change["change"] == "added"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_added_boundary_contracts_in_modified_interface_high_risk() {
    let old_dir = unique_temp_dir("rsscript-package-modified-boundary-interface-old");
    let new_dir = unique_temp_dir("rsscript-package-modified-boundary-interface-new");
    write_named_package_fixture(
        &old_dir,
        "rss-modified-interface",
        "0.1.0",
        "",
        r#"features: native

struct Bytes

pub fn Bytes.len(value: read Bytes) -> Int
"#,
    );
    write_named_package_fixture(
        &new_dir,
        "rss-modified-interface",
        "0.1.0",
        "",
        r#"features: native

struct Bytes

pub fn Bytes.len(value: read Bytes) -> Int

native fn Bytes.decode(value: read Bytes) -> String
    effects(native)
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_handle_field_contract_changes_high_risk() {
    let old_dir = unique_temp_dir("rsscript-package-handle-field-interface-old");
    let new_dir = unique_temp_dir("rsscript-package-handle-field-interface-new");
    write_named_package_fixture(
        &old_dir,
        "rss-handle-interface",
        "0.1.0",
        "",
        r#"class Rules

struct Config {
    rules: Rules
}
"#,
    );
    write_named_package_fixture(
        &new_dir,
        "rss-handle-interface",
        "0.1.0",
        "",
        r#"class Rules

struct Config {
    rules: handle Rules
}
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_noescape_callback_contract_changes_high_risk() {
    let old_dir = unique_temp_dir("rsscript-package-noescape-interface-old");
    let new_dir = unique_temp_dir("rsscript-package-noescape-interface-new");
    write_named_package_fixture(
        &old_dir,
        "rss-noescape-interface",
        "0.1.0",
        "",
        r#"pub fn Scheduler.run(callback: Closure) -> Unit
"#,
    );
    write_named_package_fixture(
        &new_dir,
        "rss-noescape-interface",
        "0.1.0",
        "",
        r#"pub fn Scheduler.run(callback: noescape Fn()) -> Unit
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_boundary_package_feature_changes_high_risk() {
    let old_dir = unique_temp_dir("rsscript-package-feature-diff-old");
    let new_dir = unique_temp_dir("rsscript-package-feature-diff-new");
    write_named_package_fixture(
        &old_dir,
        "rss-feature-diff",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_named_package_fixture(
        &new_dir,
        "rss-feature-diff",
        "0.1.0",
        r#"[features]
native-tls = ["native"]
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["kind"] == "package-feature"
                && change["name"] == "native-tls"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn rss_pkg_diff_json_reports_dependency_upgrade() {
    let old_dir = unique_temp_dir("rsscript-package-diff-cli-old");
    let new_dir = unique_temp_dir("rsscript-package-diff-cli-new");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.1.1",
        r#"[dependencies]
rss-core = "0.6"
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("diff")
        .arg("--json")
        .arg(&old_dir)
        .arg(&new_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg diff should execute");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package diff JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["risk"], "high");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "dependency" && change["name"] == "rss-core")
    }));
}

#[test]
fn package_lock_records_contract_review_and_native_hashes() {
    let temp_dir = unique_temp_dir("rsscript-package-lock");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse(_: &str) {}\n",
    )
    .expect("native source should be written");

    let lock = lock_package_dir(&temp_dir).expect("package lock should succeed");
    let toml = format_package_lock_toml(&lock);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(lock.version, 1);
    assert_eq!(lock.packages.len(), 1);
    assert_eq!(lock.packages[0].name, "rss-json");
    assert_eq!(lock.packages[0].features, vec!["streaming".to_string()]);
    assert!(lock.packages[0].checksum.starts_with("sha256:"));
    assert!(lock.packages[0].interface_hash.starts_with("sha256:"));
    assert!(lock.packages[0].review_hash.starts_with("sha256:"));
    assert!(
        lock.packages[0]
            .native_hash
            .as_ref()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(toml.contains("[[package]]"));
    assert!(toml.contains("rss_version = \""));
    assert!(toml.contains("interface_hash = \"sha256:"));
}

#[test]
fn package_lock_review_hash_tracks_native_api_count_changes() {
    let old_dir = unique_temp_dir("rsscript-package-lock-native-api-old");
    let new_dir = unique_temp_dir("rsscript-package-lock-native-api-new");
    let native_manifest = r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#;
    write_package_fixture(
        &old_dir,
        "0.1.0",
        native_manifest,
        r#"features: native

native fn Native.one(message: read String) -> String
    effects(native)
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.1.0",
        native_manifest,
        r#"features: native

native fn Native.one(message: read String) -> String
    effects(native)
native fn Native.two(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(old_dir.join("native")).expect("old native dir should be created");
    fs::write(
        old_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.one" = "rss_native::one"
"#,
    )
    .expect("old native bindings should be written");
    fs::create_dir_all(new_dir.join("native")).expect("new native dir should be created");
    fs::write(
        new_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.one" = "rss_native::one"
"Native.two" = "rss_native::two"
"#,
    )
    .expect("new native bindings should be written");

    let old_review = review_package_dir(&old_dir).expect("old package review should succeed");
    let new_review = review_package_dir(&new_dir).expect("new package review should succeed");
    let old_lock = lock_package_dir(&old_dir).expect("old package lock should succeed");
    let new_lock = lock_package_dir(&new_dir).expect("new package lock should succeed");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(old_review.summary.native_apis, 1);
    assert_eq!(new_review.summary.native_apis, 2);
    assert_ne!(
        old_lock.packages[0].review_hash,
        new_lock.packages[0].review_hash
    );
}

#[test]
fn package_lock_records_local_path_dependency_graph() {
    let root_dir = unique_temp_dir("rsscript-package-lock-graph-root");
    let dep_dir = unique_temp_dir("rsscript-package-lock-graph-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );

    let lock = lock_package_dir(&root_dir).expect("package lock should include path deps");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_json(&lock))
        .expect("package lock JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(lock.packages.len(), 2);
    assert_eq!(json["package"][0]["name"], "rss-app");
    assert_eq!(json["package"][1]["name"], "rss-dep");
    assert_eq!(json["package"][1]["features"][0], "fast");
    assert!(
        json["package"][1]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
}

#[test]
fn package_lock_hashes_dependency_effective_interface_for_selected_features() {
    let root_base_dir = unique_temp_dir("rsscript-package-lock-feature-root-base");
    let root_fast_dir = unique_temp_dir("rsscript-package-lock-feature-root-fast");
    let dep_dir = unique_temp_dir("rsscript-package-lock-feature-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = ["simd"]
simd = []

[interfaces.features.fast]
paths = ["interface/fast"]

[interfaces.features.simd]
paths = ["interface/simd"]
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(dep_dir.join("interface/fast"))
        .expect("feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/fast/lib.rssi"),
        r#"pub fn Dep.fast(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");
    fs::create_dir_all(dep_dir.join("interface/simd"))
        .expect("transitive feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/simd/lib.rssi"),
        r#"pub fn Dep.simd(text: read String) -> String
"#,
    )
    .expect("transitive feature interface should be written");
    write_named_package_fixture(
        &root_base_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    write_named_package_fixture(
        &root_fast_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );

    let base_lock = lock_package_dir(&root_base_dir).expect("base lock should succeed");
    let fast_lock = lock_package_dir(&root_fast_dir).expect("fast lock should succeed");
    let _ = fs::remove_dir_all(&root_base_dir);
    let _ = fs::remove_dir_all(&root_fast_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    let base_dep = base_lock
        .packages
        .iter()
        .find(|package| package.name == "rss-dep")
        .expect("base dependency should be locked");
    let fast_dep = fast_lock
        .packages
        .iter()
        .find(|package| package.name == "rss-dep")
        .expect("fast dependency should be locked");
    assert_eq!(base_dep.features, Vec::<String>::new());
    assert_eq!(
        fast_dep.features,
        vec!["fast".to_string(), "simd".to_string()]
    );
    assert_ne!(base_dep.interface_hash, fast_dep.interface_hash);
}

#[test]
fn rss_pkg_lock_json_reports_hashes() {
    let temp_dir = unique_temp_dir("rsscript-package-lock-cli");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("lock")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg lock should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package lock JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["version"], 1);
    assert_eq!(json["package"][0]["name"], "rss-json");
    assert!(
        json["package"][0]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(
        json["package"][0]["review_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
}

#[test]
fn package_check_reports_stale_dependency_interface_lock() {
    let root_dir = unique_temp_dir("rsscript-package-check-dep-lock-root");
    let dep_dir = unique_temp_dir("rsscript-package-check-dep-lock-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::write(
        dep_dir.join("interface/lib.rssi"),
        r#"pub fn Dep.parse(value: read String) -> String
"#,
    )
    .expect("dependency interface should be changed");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert_eq!(json["lock"]["matches"], false);
    assert!(
        json["lock"]["package_changes"]
            .as_array()
            .is_some_and(|changes| {
                changes.iter().any(|change| {
                    change["name"] == "rss-dep"
                        && change["changes"].as_array().is_some_and(|fields| {
                            fields.iter().any(|field| {
                                field["field"] == "interface_hash" && field["risk"] == "high"
                            })
                        })
                })
            })
    );
}

#[test]
fn package_check_reports_local_dependency_version_conflict() {
    let root_dir = unique_temp_dir("rsscript-package-check-conflict-root");
    let dep_a_dir = unique_temp_dir("rsscript-package-check-conflict-dep-a");
    let dep_b_dir = unique_temp_dir("rsscript-package-check-conflict-dep-b");
    let shared_v1_dir = unique_temp_dir("rsscript-package-check-conflict-shared-v1");
    let shared_v2_dir = unique_temp_dir("rsscript-package-check-conflict-shared-v2");
    write_named_package_fixture(
        &shared_v1_dir,
        "rss-shared",
        "0.1.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    write_named_package_fixture(
        &shared_v2_dir,
        "rss-shared",
        "0.2.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    write_named_package_fixture(
        &dep_a_dir,
        "rss-dep-a",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            shared_v1_dir.display()
        ),
        r#"pub fn DepA.run() -> Unit
"#,
    );
    write_named_package_fixture(
        &dep_b_dir,
        "rss-dep-b",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            shared_v2_dir.display()
        ),
        r#"pub fn DepB.run() -> Unit
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep-a = {{ path = "{}" }}
rss-dep-b = {{ path = "{}" }}
"#,
            dep_a_dir.display(),
            dep_b_dir.display()
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_a_dir);
    let _ = fs::remove_dir_all(&dep_b_dir);
    let _ = fs::remove_dir_all(&shared_v1_dir);
    let _ = fs::remove_dir_all(&shared_v2_dir);

    assert!(!check.ok);
    assert_eq!(json["graph"]["ok"], false);
    assert_eq!(json["graph"]["risk"], "high");
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason
                .as_str()
                .is_some_and(|reason| reason.contains("rss-shared"))
        })
    }));
}

#[test]
fn package_review_update_reports_lockfile_contract_changes() {
    let old_dir = unique_temp_dir("rsscript-package-update-old");
    let new_dir = unique_temp_dir("rsscript-package-update-new");
    let lock_dir = unique_temp_dir("rsscript-package-update-locks");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    );
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    fs::write(
        &old_lock_path,
        format_package_lock_toml(&lock_package_dir(&old_dir).expect("old lock should be built")),
    )
    .expect("old lock should be written");
    fs::write(
        &new_lock_path,
        format_package_lock_toml(&lock_package_dir(&new_dir).expect("new lock should be built")),
    )
    .expect("new lock should be written");

    let diff =
        diff_package_locks(&old_lock_path, &new_lock_path).expect("lock diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_diff_json(&diff))
        .expect("lock diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let _ = fs::remove_dir_all(&lock_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == ".rssi interface hash changed")
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package feature selection changed")
    }));
    assert!(json["package_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["name"] == "rss-json"
                && change["changes"].as_array().is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| field["field"] == "interface_hash" && field["risk"] == "high")
                })
        })
    }));
}

#[test]
fn package_lock_diff_marks_boundary_feature_selection_high_risk() {
    let lock_dir = unique_temp_dir("rsscript-package-lock-boundary-feature");
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    let old_lock = rsscript::PackageLock {
        version: 1,
        packages: vec![rsscript::PackageLockPackage {
            name: "rss-net".to_string(),
            version: "0.1.0".to_string(),
            source: "path:.".to_string(),
            checksum: "sha256:old".to_string(),
            interface_hash: "sha256:interface".to_string(),
            review_hash: "sha256:review".to_string(),
            native_hash: None,
            features: Vec::new(),
        }],
        metadata: rsscript::PackageLockMetadata {
            rsscript_version: "0.5".to_string(),
            created_by: "test".to_string(),
        },
    };
    let mut new_lock = old_lock.clone();
    new_lock.packages[0].features = vec!["native-tls".to_string()];
    fs::write(&old_lock_path, format_package_lock_toml(&old_lock))
        .expect("old lock should be written");
    fs::write(&new_lock_path, format_package_lock_toml(&new_lock))
        .expect("new lock should be written");

    let diff =
        diff_package_locks(&old_lock_path, &new_lock_path).expect("lock diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_diff_json(&diff))
        .expect("lock diff JSON should parse");
    let _ = fs::remove_dir_all(&lock_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package feature selection changed")
    }));
    assert!(json["package_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["name"] == "rss-net"
                && change["risk"] == "high"
                && change["changes"].as_array().is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| field["field"] == "features" && field["risk"] == "high")
                })
        })
    }));
}

#[test]
fn rss_pkg_review_update_json_reports_lock_changes() {
    let old_dir = unique_temp_dir("rsscript-package-update-cli-old");
    let new_dir = unique_temp_dir("rsscript-package-update-cli-new");
    let lock_dir = unique_temp_dir("rsscript-package-update-cli-locks");
    write_package_fixture(
        &old_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    write_package_fixture(
        &new_dir,
        "0.1.1",
        "",
        r#"pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    );
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    fs::write(
        &old_lock_path,
        format_package_lock_toml(&lock_package_dir(&old_dir).expect("old lock should be built")),
    )
    .expect("old lock should be written");
    fs::write(
        &new_lock_path,
        format_package_lock_toml(&lock_package_dir(&new_dir).expect("new lock should be built")),
    )
    .expect("new lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("review")
        .arg("update")
        .arg("--json")
        .arg("--from")
        .arg(&old_lock_path)
        .arg("--to")
        .arg(&new_lock_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg review update should execute");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let _ = fs::remove_dir_all(&lock_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value =
        serde_json::from_str(&stdout).expect("stdout should be package review update JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["risk"], "high");
    assert!(
        json["package_changes"][0]["changes"]
            .as_array()
            .is_some_and(|changes| changes
                .iter()
                .any(|change| change["field"] == "interface_hash"))
    );
}

#[test]
fn package_check_reports_stale_semantic_lock() {
    let temp_dir = unique_temp_dir("rsscript-package-check-stale");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::write(
        temp_dir.join("interface/lib.rssi"),
        r#"struct MathError

pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    )
    .expect("interface should be changed");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "high");
    assert_eq!(json["lock"]["present"], true);
    assert_eq!(json["lock"]["matches"], false);
    assert!(json["lock"]["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == ".rssi interface hash changed")
    }));
}

#[test]
fn rss_pkg_check_json_reports_consistent_package() {
    let temp_dir = unique_temp_dir("rsscript-package-check-cli");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("check")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg check should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package check JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["lock"]["present"], true);
    assert_eq!(json["lock"]["matches"], true);
}

#[test]
fn rss_check_json_accepts_package_directory() {
    let temp_dir = unique_temp_dir("rsscript-check-package-cli");
    write_named_package_fixture(
        &temp_dir,
        "rss-check-package",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("check")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss check package directory should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package check JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["package"]["name"], "rss-check-package");
}

#[test]
fn rss_pkg_check_fails_when_lock_missing() {
    let temp_dir = unique_temp_dir("rsscript-package-check-missing-lock");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("check")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg check should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package check JSON");

    assert!(!output.status.success(), "stdout={stdout}");
    assert_eq!(json["ok"], false);
    assert_eq!(json["lock"]["present"], false);
    assert!(
        json["lock"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == "rsspkg.lock missing"))
    );
}

#[test]
fn package_check_reports_native_rust_consistency_issues() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["cargo_toml_present"], false);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], false);
    assert_eq!(json["native_rust"]["risk"], "high");
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust Cargo.toml missing"))
    );
}

#[test]
fn package_check_reports_native_cargo_metadata() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-metadata");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok);
    assert_eq!(json["native_rust"]["cargo_toml_present"], true);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], true);
    assert_eq!(json["native_rust"]["cargo_package_name"], "rss_json_native");
    assert_eq!(json["native_rust"]["unsafe_detected"], false);
    assert_eq!(json["native_rust"]["build_env_detected"], false);
    assert_eq!(json["native_rust"]["build_download_detected"], false);
    assert_eq!(
        json["native_rust"]["linked_libraries"],
        serde_json::json!([])
    );
    assert!(
        json["native_rust"]["target_kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
    );
}

#[test]
fn package_check_accepts_bound_native_interface_functions() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok, "{:?}", check.diagnostics);
    assert!(
        check
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS1301")
    );
}

#[test]
fn package_check_reports_unknown_native_binding_symbols() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-unknown");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.ehco" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601" && diagnostic.label == "unknown native binding symbol"
    }));
}

#[test]
fn package_check_reports_native_binding_crate_mismatch() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-crate");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "other_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601" && diagnostic.label == "native binding crate mismatch"
    }));
}

#[test]
fn package_check_reports_native_binding_without_native_rust() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-no-native");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601"
            && diagnostic.label == "native binding without native Rust wrapper"
    }));
}

#[test]
fn package_check_reports_native_binding_missing_crate() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-binding-no-crate");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601" && diagnostic.label == "native binding crate missing"
    }));
}

#[test]
fn package_check_reports_native_unsafe_usage() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-unsafe");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        r#"pub fn parse() {
    let _ = "unsafe in a string";
    // unsafe in a comment should not count
    unsafe {}
}
"#,
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["unsafe_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust unsafe usage detected"))
    );
}

#[test]
fn package_check_reports_native_linked_libraries() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-links");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
links = ["ssl"]
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok);
    assert_eq!(check.risk, rsscript::PackageRisk::High);
    assert_eq!(
        json["native_rust"]["linked_libraries"],
        serde_json::json!(["ssl"])
    );
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust links external libraries")
    }));
}

#[test]
fn package_check_reports_native_build_script_environment_usage() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-build-env");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    let _ = std::env::var("OUT_DIR");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_env_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script reads environment"))
    );
}

#[test]
fn package_check_reports_native_build_script_download_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-build-download");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    // https://example.invalid/commented-out should not be the only signal
    let _ = std::process::Command::new("curl").arg("https://example.invalid/archive.tar.gz");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_download_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script may download code"))
    );
}

#[test]
fn package_check_reports_native_build_script_from_cargo_metadata() {
    let temp_dir = unique_temp_dir("rsscript-package-check-native-build-script");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(temp_dir.join("native/rust/build.rs"), "fn main() {}\n")
        .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], true);
    assert!(
        json["native_rust"]["target_kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "custom-build"))
    );
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script target present"))
    );
}

#[test]
fn package_lowering_input_records_native_wrapper_dependency() {
    let temp_dir = unique_temp_dir("rsscript-package-native-lowering-input");
    write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");

    let input = package_lowering_input(&temp_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with native dependency");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(input.native_dependencies[0].crate_name, "rss_json_native");
    assert!(input.native_dependencies[0].path.ends_with("native/rust"));
    assert!(
        package
            .cargo_toml
            .contains("\"rss_json_native\" = { path = ")
    );
}

#[test]
fn package_tree_expands_path_dependencies_and_marks_unresolved() {
    let root_dir = unique_temp_dir("rsscript-package-tree-root");
    let dep_dir = unique_temp_dir("rsscript-package-tree-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_dep_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    fs::create_dir_all(dep_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        dep_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_dep_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        dep_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["streaming"] }}
rss-remote = "0.5"
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let tree = package_tree(&root_dir).expect("package tree should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_tree_json(&tree))
        .expect("package tree JSON should parse");
    let human = rsscript::format_package_tree_human(&tree);
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(json["root"]["name"], "rss-json");
    assert_eq!(json["summary"]["packages"], 3);
    assert_eq!(json["summary"]["path_dependencies"], 1);
    assert_eq!(json["summary"]["unresolved_dependencies"], 1);
    assert_eq!(json["summary"]["native_packages"], 1);
    assert!(json["root"]["dependencies"].as_array().is_some_and(|deps| {
        deps.iter().any(|dep| {
            dep["name"] == "rss-dep"
                && dep["version"] == "0.2.0"
                && dep["features"][0] == "streaming"
                && dep["native"] == true
        }) && deps
            .iter()
            .any(|dep| dep["name"] == "rss-remote" && dep["risk"] == "unknown")
    }));
    assert!(human.contains("|-- rss-dep 0.2.0 [elevated, native, features streaming]"));
    assert!(human.contains("`-- rss-remote req 0.5 [unknown]"));
}

#[test]
fn rss_pkg_tree_json_reports_dependency_summary() {
    let root_dir = unique_temp_dir("rsscript-package-tree-cli-root");
    let dep_dir = unique_temp_dir("rsscript-package-tree-cli-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn parse(text: read String) -> String
"#,
    );
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("tree")
        .arg("--json")
        .arg(&root_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg tree should execute");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be package tree JSON");

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["summary"]["packages"], 2);
    assert_eq!(json["summary"]["path_dependencies"], 1);
    assert_eq!(json["root"]["dependencies"][0]["name"], "rss-dep");
}

#[test]
fn package_publish_dry_run_reports_ready_package() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-ready");
    write_named_package_fixture(
        &temp_dir,
        "rss-ready",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::create_dir_all(temp_dir.join("target/debug")).expect("target dir should be created");
    fs::write(temp_dir.join("target/debug/junk"), "do not publish")
        .expect("target file should be written");
    fs::create_dir_all(temp_dir.join("review")).expect("review dir should be created");
    fs::write(temp_dir.join("review/package-review.json"), "{}\n")
        .expect("generated review metadata should be written");

    let publish = publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    let publish_again =
        publish_package_dry_run(&temp_dir).expect("publish dry-run should be deterministic");
    let registry_dir = temp_dir.join("registry");
    let publish_with_registry =
        rsscript::publish_package_dry_run_with_registry(&temp_dir, Some(&registry_dir))
            .expect("publish dry-run should report registry paths");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("publish JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);
    let archive_paths = publish
        .archive_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert!(publish.ready);
    assert_eq!(publish.archive_hash, publish_again.archive_hash);
    assert!(
        publish_with_registry
            .registry_target
            .as_ref()
            .is_some_and(|target| target.index_path.ends_with("index/rss-ready/0.1.0.json"))
    );
    assert!(
        publish_with_registry
            .registry_target
            .as_ref()
            .is_some_and(|target| target
                .archive_manifest_path
                .ends_with("archives/rss-ready/0.1.0/archive-manifest.json"))
    );
    assert_eq!(json["package"]["name"], "rss-ready");
    assert_eq!(json["registry_index"]["schema"], "rss.registry.index.v1");
    assert_eq!(json["registry_index"]["name"], "rss-ready");
    assert_eq!(json["registry_index"]["version"], "0.1.0");
    assert_eq!(json["registry_index"]["checksum"], json["archive_hash"]);
    assert_eq!(json["registry_index"]["risk"], "elevated");
    assert_eq!(json["registry_index"]["native"], false);
    assert_eq!(json["registry_index"]["unsafe"], false);
    assert!(
        json["registry_index"]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(
        json["registry_index"]["review_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert_eq!(json["archive_format"], "rss.package.archive.v1");
    assert!(
        json["archive_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(archive_paths.contains(&"rsspkg.toml"), "{archive_paths:?}");
    assert!(
        archive_paths.contains(&"interface/lib.rssi"),
        "{archive_paths:?}"
    );
    assert!(archive_paths.contains(&"rsspkg.lock"), "{archive_paths:?}");
    assert!(
        !archive_paths.iter().any(|path| path.starts_with("target/")),
        "{archive_paths:?}"
    );
    assert!(
        !archive_paths
            .iter()
            .any(|path| *path == "review/package-review.json"),
        "{archive_paths:?}"
    );
    assert!(json["archive_files"].as_array().is_some_and(|files| {
        files.iter().any(|file| {
            file["path"] == "rsspkg.toml"
                && file["sha256"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("sha256:"))
        })
    }));
    assert!(json["checks"].as_array().is_some_and(|checks| {
        checks
            .iter()
            .any(|check| check["name"] == "package archive reproducible" && check["ok"] == true)
    }));
}

#[test]
fn package_publish_dry_run_blocks_unknown_review_risk() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-unknown-risk");
    write_named_package_fixture(
        &temp_dir,
        "rss-unknown",
        "0.1.0",
        r#"[review.expect]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let publish = publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("publish JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!publish.ready);
    assert_eq!(json["risk"], "unknown");
    assert!(json["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["name"] == "package review risk classified"
                && check["ok"] == false
                && check["risk"] == "unknown"
        })
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package review risk classified failed: review risk unknown")
    }));
}

#[test]
fn rss_pkg_publish_dry_run_reports_local_registry_target() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-registry-cli");
    let registry_dir = unique_temp_dir("rsscript-package-publish-registry-target");
    write_named_package_fixture(
        &temp_dir,
        "rss-registry",
        "0.1.0",
        "",
        r#"pub fn Registry.value() -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("publish")
        .arg("--dry-run")
        .arg("--json")
        .arg("--registry")
        .arg(&registry_dir)
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg publish should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be publish JSON");
    let index_written = registry_dir
        .join("index")
        .join("rss-registry")
        .join("0.1.0.json")
        .exists();
    let _ = fs::remove_dir_all(&temp_dir);
    let _ = fs::remove_dir_all(&registry_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(
        json["registry_target"]["index_path"]
            .as_str()
            .map(|path| path.ends_with("index/rss-registry/0.1.0.json")),
        Some(true)
    );
    assert_eq!(
        json["registry_target"]["archive_manifest_path"]
            .as_str()
            .map(|path| path.ends_with("archives/rss-registry/0.1.0/archive-manifest.json")),
        Some(true)
    );
    assert!(!index_written);
}

#[test]
fn package_publish_dry_run_reports_registry_index_dependencies() {
    let root_dir = unique_temp_dir("rsscript-package-publish-index-root");
    let dep_dir = unique_temp_dir("rsscript-package-publish-index-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.value() -> Int
"#,
    );
    write_named_package_fixture(
        &root_dir,
        "rss-index-root",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ version = "0.2.0", path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn Root.value() -> Int
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let publish = publish_package_dry_run(&root_dir).expect("publish dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("publish JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(publish.ready);
    assert_eq!(json["registry_index"]["dependencies"]["rss-dep"], "0.2.0");
}

#[test]
fn rss_pkg_publish_dry_run_json_reports_unresolved_dependency() {
    let temp_dir = unique_temp_dir("rsscript-package-publish-blocked");
    write_named_package_fixture(
        &temp_dir,
        "rss-blocked",
        "0.1.0",
        r#"[dependencies]
rss-remote = "0.5.0"
"#,
        r#"pub fn main() -> Unit
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("publish")
        .arg("--dry-run")
        .arg("--json")
        .arg(&temp_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg publish should execute");
    let _ = fs::remove_dir_all(&temp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be publish JSON");

    assert!(!output.status.success(), "stdout={stdout}");
    assert_eq!(json["ready"], false);
    assert_eq!(json["risk"], "unknown");
    assert!(json["checks"].as_array().is_some_and(|checks| {
        checks
            .iter()
            .any(|check| check["name"] == "dependency graph review" && check["ok"] == false)
    }));
}

#[test]
fn package_vendor_dry_run_reports_path_and_unresolved_dependencies() {
    let root_dir = unique_temp_dir("rsscript-package-vendor-root");
    let dep_dir = unique_temp_dir("rsscript-package-vendor-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn parse(text: read String) -> String
"#,
    );
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
rss-remote = "0.5.0"
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let vendor =
        vendor_package_dir(&root_dir, true).expect("vendor dry-run should produce a report");
    let json: Value = serde_json::from_str(&rsscript::format_package_vendor_json(&vendor))
        .expect("vendor JSON should parse");
    let vendor_dir_exists = root_dir.join("vendor").exists();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!vendor.ok);
    assert!(!vendor_dir_exists);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["entries"][0]["name"], "rss-dep");
    assert_eq!(json["unresolved"][0]["name"], "rss-remote");
    assert_eq!(json["risk"], "unknown");
}

#[test]
fn rss_pkg_vendor_json_writes_vendor_directory_and_metadata() {
    let root_dir = unique_temp_dir("rsscript-package-vendor-cli-root");
    let dep_dir = unique_temp_dir("rsscript-package-vendor-cli-dep");
    write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn parse(text: read String) -> String
"#,
    );
    write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            dep_dir.display()
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("pkg")
        .arg("vendor")
        .arg("--json")
        .arg(&root_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rss pkg vendor should execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be vendor JSON");
    let vendored_manifest = root_dir
        .join("vendor")
        .join("rss-dep-0.2.0")
        .join("rsspkg.toml");
    let vendor_metadata = root_dir.join("vendor").join("rss-vendor.json");
    let vendored_manifest_exists = vendored_manifest.exists();
    let vendor_metadata_exists = vendor_metadata.exists();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["entries"][0]["name"], "rss-dep");
    assert!(vendored_manifest_exists);
    assert!(vendor_metadata_exists);
}

fn fixture_paths(directory: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory}: {error}"))
        .map(|entry| entry.expect("fixture entry should be readable").path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
        })
        .collect();
    paths.sort();
    paths
}

fn recursive_fixture_paths(directory: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_fixture_paths(Path::new(directory), &mut paths);
    paths.sort();
    paths
}

fn recursive_paths_with_extension(directory: &str, extension: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_paths_with_extension(Path::new(directory), extension, &mut paths);
    paths.sort();
    paths
}

fn collect_paths_with_extension(directory: &Path, extension: &str, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory:?}: {error}"))
    {
        let path = entry.expect("fixture entry should be readable").path();
        if path.is_dir() {
            collect_paths_with_extension(&path, extension, paths);
        } else if path
            .extension()
            .and_then(|path_extension| path_extension.to_str())
            == Some(extension)
        {
            paths.push(path);
        }
    }
}

fn collect_fixture_paths(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory:?}: {error}"))
    {
        let path = entry.expect("fixture entry should be readable").path();
        if path.is_dir() {
            collect_fixture_paths(&path, paths);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
        {
            paths.push(path);
        }
    }
}

fn read_fixture(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{counter}", std::process::id()))
}

fn write_package_fixture(
    directory: &Path,
    version: &str,
    extra_manifest: &str,
    interface_source: &str,
) {
    write_named_package_fixture(
        directory,
        "rss-json",
        version,
        extra_manifest,
        interface_source,
    );
}

fn write_named_package_fixture(
    directory: &Path,
    name: &str,
    version: &str,
    extra_manifest: &str,
    interface_source: &str,
) {
    fs::create_dir_all(directory.join("interface")).expect("interface dir should be created");
    fs::write(
        directory.join("rsspkg.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "{version}"
edition = "2026"

[interfaces]
paths = ["interface"]

{extra_manifest}
"#
        ),
    )
    .expect("package manifest should be written");
    fs::write(directory.join("interface/lib.rssi"), interface_source)
        .expect("interface should be written");
}

fn write_empty_named_package_fixture(
    directory: &Path,
    name: &str,
    version: &str,
    extra_manifest: &str,
) {
    fs::create_dir_all(directory).expect("package dir should be created");
    fs::write(
        directory.join("rsspkg.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "{version}"
edition = "2026"

{extra_manifest}
"#
        ),
    )
    .expect("package manifest should be written");
}

fn write_empty_resource_pool_fixture(path: &Path) {
    fs::write(
        path,
        r#"features: local

fn main() -> Unit {
    let url = Url.from_string(value: read "db://local")
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 0,
    )

    with ResourcePool.borrow(pool: mut pool) as conn {
        Log.write(message: read "unreachable")
    }

    return Unit
}
"#,
    )
    .expect("runtime diagnostic fixture should be written");
}

fn expected_codes(source: &str) -> Vec<String> {
    let first_line = source.lines().next().unwrap_or_default();
    let Some(codes) = first_line.strip_prefix("// expect:") else {
        panic!("fail fixture must start with `// expect:`");
    };
    codes.split_whitespace().map(str::to_string).collect()
}

fn fail_fixture_expected_code_set() -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for path in fixture_paths("tests/fixtures/fail") {
        let source = read_fixture(&path);
        for code in expected_codes(&source) {
            codes.insert(code);
        }
    }
    codes
}

fn source_map_summary(entries: &[rsscript::RustSourceMapEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{} {}:{}:{} -> {}:{}:{}\n",
                entry.kind,
                entry.source.file,
                entry.source.line,
                entry.source.column,
                entry.generated.file,
                entry.generated.line,
                entry.generated.column
            )
        })
        .collect()
}
