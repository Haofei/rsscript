mod common;

use std::collections::BTreeSet;
use std::path::Path;

use rsscript::syntax::ast::{EffectDecl, Item};
use rsscript::syntax::parse_source;
use rsscript::{
    Severity, analyze_source, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_without_core, analyze_sources_with_interfaces, core_interfaces,
    explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_json, lint_source,
    lower_source_to_rust, lower_source_to_rust_package,
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
    ("read view mutation", "RS0310"),
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
    ("spawn used before structured async task support", "RS0015"),
    ("async call not consumed", "RS0022"),
    ("unknown protocol", "RS0027"),
    ("unmappable rustc diagnostic", "RS1102"),
    ("package feature resolution violation", "PKG0101"),
    ("unsupported package dependency source", "PKG0102"),
    ("package review policy violation", "PKG0501"),
    ("package native binding metadata violation", "PKG0601"),
    ("package provider declaration violation", "PKG0901"),
];

#[test]
fn pass_fixtures_have_no_diagnostics() {
    for path in common::fixture_paths("tests/fixtures/pass") {
        let source = common::read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn fail_fixtures_report_expected_diagnostic_codes() {
    for path in common::fixture_paths("tests/fixtures/fail") {
        let source = common::read_fixture(&path);
        let expected = common::expected_codes(&source);
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
    let fixture_codes = common::fail_fixture_expected_code_set();
    let dedicated_test_codes = BTreeSet::from([
        "RS1102",  // rustc_diagnostics_report_unmappable_generated_spans
        "RS1201",  // runtime_diagnostic_lines_parse_to_rsscript_diagnostics
        "RS0310",  // checker_rejects_exclusive_use_of_for_read_view
        "PKG0101", // package feature resolution diagnostics
        "PKG0102", // unsupported package dependency source diagnostics
        "PKG0501", // package review policy diagnostics
        "PKG0601", // package native binding diagnostics
        "PKG0901", // package provider declaration diagnostics
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
    for path in common::recursive_fixture_paths("core") {
        let source = common::read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn examples_have_no_diagnostics_and_lower_to_runnable_packages() {
    for path in common::fixture_paths("examples") {
        let source = common::read_fixture(&path);
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
    for path in common::recursive_paths_with_extension("tests/generated", "rs") {
        let source = common::read_fixture(&path);
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
            .any(|(path, _)| *path == "core/process/process.rssi")
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
    with File.open(path: read path)? as file {
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

    assert!(codes.contains(&"RS0601".to_string()));
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
fn checker_rejects_exclusive_use_of_for_read_view() {
    let source = r#"
features: local

struct Buffer {
    bytes: Int
}

fn mutate(buffer: mut Buffer) -> Unit
fn consume(buffer: take Buffer) -> Unit
fn inspect(buffer: read Buffer) -> Unit

fn bad(buffers: read List<Buffer>) -> Unit {
    for buffer in buffers {
        inspect(buffer: read buffer)
        mutate(buffer: mut buffer)
        consume(buffer: take buffer)
        local copied = manage buffer
    }
}
"#;
    let diagnostics = analyze_source("for-read-view.rss", source);
    let read_view_errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RS0310")
        .count();

    assert_eq!(read_view_errors, 3, "{diagnostics:?}");
}

#[test]
fn checker_accepts_closure_parameter_without_treating_closure_as_data_effect_param() {
    let source = r#"
fn Scheduler.run(callback: Closure) -> Unit
    effects(retains(callback))
"#;
    let diagnostics = analyze_source("closure-param.rss", source);

    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0008"),
        "{diagnostics:?}"
    );
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
    let source = common::read_fixture(path);
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
fn main() -> Result<Unit, JsonError> {
    Log.write(message: read 42)
    return Ok(Unit)
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
fn main() -> Result<Unit, JsonError> {
    Log.write(message: read 42)
    return Ok(Unit)
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
fn checker_reports_function_fallthrough_return_type_mismatch() {
    let source = r#"
fn build() -> String {
    42
}
"#;
    let diagnostics = analyze_source("fallthrough-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary == "return in `build` has type `Unit`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_let_type_annotation_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let value: String = 42
    return Unit
}
"#;
    let diagnostics = analyze_source("let-annotation-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_result_binding_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn main() -> Unit {
    let value: Result<String, BuildError> = Ok(42)
    return Unit
}
"#;
    let diagnostics = analyze_source("result-binding-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_nested_result_option_binding_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn main() -> Unit {
    let value: Result<Option<String>, BuildError> = Ok(Some(42))
    return Unit
}
"#;
    let diagnostics = analyze_source("nested-result-option-binding-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_generic_binding_annotation_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let values: List<String> = List<Int>.new()
    return Unit
}
"#;
    let diagnostics = analyze_source("generic-binding-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `values` has initializer type `List<Int>`, expected `List<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_generic_call_argument_mismatch_before_backend_lowering() {
    let source = r#"
fn accept(values: read List<String>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let values = List<Int>.new()
    accept(values: read values)
    return Unit
}
"#;
    let diagnostics = analyze_source("generic-call-arg-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `values` for `accept` has type `List<Int>`, expected `List<String>`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_nested_result_option_argument_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn accept(value: read Result<Option<String>, BuildError>) -> Unit {
    return Unit
}

fn main() -> Unit {
    accept(value: read Ok(Some(42)))
    return Unit
}
"#;
    let diagnostics = analyze_source("nested-result-option-argument-payload-type.rss", source);

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
fn checker_reports_option_binding_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
fn main() -> Unit {
    let value: Option<String> = Some(42)
    return Unit
}
"#;
    let diagnostics = analyze_source("option-binding-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "binding `value` has initializer payload type `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_let_type_annotation_mismatch_before_rustc() {
    let source = r#"
fn main() -> Unit {
    let value: String = 42
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("let-annotation-type.rss", source)
        .expect_err("binding type mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_generic_binding_annotation_mismatch_before_rustc() {
    let source = r#"
fn main() -> Unit {
    let values: List<String> = List<Int>.new()
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("generic-binding-type.rss", source)
        .expect_err("generic binding mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_result_binding_payload_type_mismatch_before_rustc() {
    let source = r#"
class BuildError {
    code: Int
}

fn main() -> Unit {
    let value: Result<String, BuildError> = Ok(42)
    return Unit
}
"#;
    let diagnostics = lower_source_to_rust("result-binding-payload-type.rss", source)
        .expect_err("binding payload mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_nested_result_option_return_payload_mismatch_before_rustc() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<Option<String>, BuildError> {
    return Ok(Some(42))
}
"#;
    let diagnostics = lower_source_to_rust("nested-result-option-return-payload-type.rss", source)
        .expect_err("nested return payload mismatch should fail before Rust generation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0208"),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_rejects_function_fallthrough_before_rustc() {
    let source = r#"
fn build() -> String {
    42
}
"#;
    let diagnostics = lower_source_to_rust("fallthrough-return-type.rss", source)
        .expect_err("function fallthrough should fail before Rust generation");

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
fn checker_reports_nested_result_option_return_payload_type_mismatch_before_backend_lowering() {
    let source = r#"
class BuildError {
    code: Int
}

fn build() -> Result<Option<String>, BuildError> {
    return Ok(Some(42))
}
"#;
    let diagnostics = analyze_source("nested-result-option-return-payload-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0208"
                && diagnostic.summary
                    == "Some payload in `build` has type `Int`, expected `String`."
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
fn checker_reports_noescape_callback_return_type_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn() -> String) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || 42)
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_result_payload_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn() -> Result<String, BuildError>) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || Ok(42))
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-result-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_nested_result_payload_mismatch() {
    let source = r#"
class BuildProblem {
    code: Int
}

fn apply(callback: noescape Fn() -> Result<Option<String>, BuildProblem>) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || Ok(Some(42)))
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-nested-result-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_fresh_return_captured_managed_value() {
    let source = r#"
struct ImageData {
    size: Int
}

fn apply(callback: noescape Fn() -> fresh ImageData) -> Unit {
    return Unit
}

fn main() -> Unit {
    let image = ImageData(size: 1)
    apply(callback: || image)
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-fresh-captured-managed.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns non-fresh value `image`, expected `fresh ImageData`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_result_fresh_payload_captured_managed_value() {
    let source = r#"
struct ImageData {
    size: Int
}

class BuildProblem {
    code: Int
}

fn apply(callback: noescape Fn() -> Result<fresh ImageData, BuildProblem>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let image = ImageData(size: 1)
    apply(callback: || Ok(image))
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-result-fresh-captured-managed.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns non-fresh value `image`, expected `fresh ImageData`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_accepts_noescape_callback_result_fresh_payload_constructor() {
    let source = r#"
struct ImageData {
    size: Int
}

class BuildProblem {
    code: Int
}

fn apply(callback: noescape Fn() -> Result<fresh ImageData, BuildProblem>) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || Ok(ImageData(size: 1)))
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-result-fresh-constructor.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_noescape_callback_retaining_local_created_inside_callback() {
    let source = r#"
features: local

struct ImageData {
    size: Int
}

class BuildProblem {
    code: Int
}

fn Cache.store(image: read ImageData) -> Unit
    effects(retains(image))

fn apply(callback: noescape Fn() -> Result<fresh ImageData, BuildProblem>) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || {
        local image = ImageData(size: 1)
        Cache.store(image: read image)
        return Ok(image)
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-retains-local.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0501"
                && diagnostic.summary
                    == "retaining API `Cache.store` cannot retain local value `image`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_noescape_callback_retaining_local_inside_wrapper() {
    let source = r#"
features: local

struct ImageData {
    size: Int
}

fn Cache.store_option(image: read Option<ImageData>) -> Unit
    effects(retains(image))

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn main() -> Unit {
    apply(callback: || {
        local image = ImageData(size: 1)
        Cache.store_option(image: read Some(image))
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-retains-local-wrapper.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0501"
                && diagnostic.summary
                    == "retaining API `Cache.store_option` cannot retain local value `image`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_early_return_type_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn() -> Result<String, BuildError>) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || {
        if true {
            return Ok(42)
        }
        return Ok("ok")
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-early-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_nested_early_return_type_mismatch() {
    let source = r#"
class BuildProblem {
    code: Int
}

fn apply(callback: noescape Fn() -> Result<Option<String>, BuildProblem>) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || {
        if true {
            return Ok(Some(42))
        }
        return Ok(None)
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-nested-early-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_match_arm_return_type_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn() -> Result<String, BuildError>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let value = Some("x")
    apply(callback: || {
        match value {
            Some(result) => return Ok(result)
            None => return Ok(42)
        }
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-match-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_parameter_count_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn(Int) -> String) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: || "x")
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-arity-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `apply` has 0 parameter(s), expected 1."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_uses_noescape_callback_parameter_type_for_return_contract() {
    let source = r#"
fn stringify(callback: noescape Fn(Int) -> String) -> Unit {
    return Unit
}

fn main() -> Unit {
    stringify(callback: |value| value)
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-param-return-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback argument `callback` for `stringify` returns `Int`, expected `String`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn rust_lowering_accepts_noescape_callback_with_parameter() {
    let source = r#"
fn apply(callback: noescape Fn(Int) -> Int) -> Int {
    return callback(41)
}

fn main() -> Unit {
    let value = apply(callback: |item| item + 1)
    return Unit
}
"#;
    let rust = lower_source_to_rust("callback-param.rss", source)
        .expect("callback with parameter should lower");

    assert!(rust.contains("callback(41)"));
    assert!(rust.contains("|item|"));
}

#[test]
fn checker_reports_noescape_callback_call_argument_type_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn(Int) -> Int) -> Int {
    return callback("x")
}

fn main() -> Unit {
    let value = apply(callback: |item| item)
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-call-arg-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument 1 for callback `callback` has type `String`, expected `Int`."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_call_arity_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn(Int, Int) -> Int) -> Int {
    return callback(1)
}

fn main() -> Unit {
    let value = apply(callback: |left, right| left)
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-call-arity.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "callback `callback` called with 1 argument(s), expected 2."
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_noescape_callback_operator_type_mismatch() {
    let source = r#"
fn apply(callback: noescape Fn(Int) -> Bool) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: |value| value == "x")
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-operator-type.rss", source);

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
fn checker_uses_noescape_callback_parameter_type_for_body_call_arguments() {
    let source = r#"
fn apply(callback: noescape Fn(Int) -> Int) -> Unit {
    return Unit
}

fn main() -> Unit {
    apply(callback: |value| String.len(value: read value))
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-body-call-arg-type.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0207"
                && diagnostic.summary
                    == "argument `value` for `String.len` has type `Int`, expected `String`."
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
fn checker_can_disable_bundled_core_interfaces() {
    let source = r#"
fn log(value: read String) -> Unit {
    Log.write(message: read value)
    return Unit
}
"#;
    assert!(
        !analyze_source("with-core.rss", source)
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0206"),
        "default analysis should include bundled core interfaces"
    );

    let diagnostics = analyze_source_without_core("without-core.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0206"
                && diagnostic.summary.contains("Log.write")),
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
fn checker_accepts_executable_async_function_body() {
    let source = r#"
features: async

async fn tick() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("async-body.rss", source);

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn checker_accepts_await_inside_async_function() {
    let source = r#"
features: async

async fn Timer.sleep(ms: Int) -> Unit

async fn receive() -> Unit {
    await Timer.sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-body.rss", source);

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn checker_rejects_await_outside_async_function() {
    let source = r#"
features: async

async fn Timer.sleep(ms: Int) -> Unit

fn receive() -> Unit {
    await Timer.sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-outside-async.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0029" && diagnostic.label == "await outside async fn"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_await_of_non_async_expression() {
    let source = r#"
features: async

fn sync_sleep(ms: Int) -> Unit

async fn receive() -> Unit {
    await sync_sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-non-async.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0030" && diagnostic.label == "await non-async expression"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_await_inside_non_async_closure() {
    let source = r#"
features: async

async fn Timer.sleep(ms: Int) -> Unit
fn run(callback: noescape Fn()) -> Unit

async fn receive() -> Unit {
    run(callback: || {
        await Timer.sleep(ms: 1)
    })
    return Unit
}
"#;
    let diagnostics = analyze_source("await-closure.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0029" && diagnostic.label == "await outside async fn"
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_resource_live_across_await() {
    let source = r#"
features: async, local

resource File
struct IOError

fn File.open(path: read Path) -> Result<File, IOError>
async fn Timer.sleep(ms: Int) -> Result<Unit, IOError>

async fn bad(path: read Path) -> Result<Unit, IOError> {
    with File.open(path: read path)? as file {
        await Timer.sleep(ms: 1)?
    }
    return Ok(Unit)
}
"#;
    let diagnostics = analyze_source("await-resource.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0031"
                && diagnostic
                    .summary
                    .contains("resource `file` cannot live across `await`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_allows_dead_local_before_await() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Timer.sleep(ms: Int) -> Unit

async fn ok() -> Unit {
    local image = Image(size: 1)
    await Timer.sleep(ms: 1)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-dead-local.rss", source);

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0031"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_local_used_after_await() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Timer.sleep(ms: Int) -> Unit
fn Image.inspect(image: read Image) -> Unit

async fn bad() -> Unit {
    local image = Image(size: 1)
    await Timer.sleep(ms: 1)
    Image.inspect(image: read image)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-live-local.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0031"
                && diagnostic
                    .summary
                    .contains("local value `image` cannot live across `await`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_local_passed_into_awaited_call() {
    let source = r#"
features: async, local

struct Image {
    size: Int
}

async fn Image.upload(image: read Image) -> Unit

async fn bad() -> Unit {
    local image = Image(size: 1)
    await Image.upload(image: read image)
    return Unit
}
"#;
    let diagnostics = analyze_source("await-local-arg.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0031"
                && diagnostic
                    .summary
                    .contains("local value `image` cannot live across `await`")
        }),
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
fn checker_rejects_async_call_without_await() {
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
            diagnostic.code == "RS0022" && diagnostic.label == "async call must be awaited"
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
fn checker_keeps_file_features_scoped_across_source_sets() {
    let diagnostics = analyze_sources_with_interfaces(
        &[
            (
                "capability.rss",
                r#"
features: local

fn helper() -> Unit {
    local value = String.new()
    return Unit
}
"#,
            ),
            (
                "plain.rss",
                r#"
fn bad() -> Unit {
    local value = String.new()
    return Unit
}
"#,
            ),
        ],
        &[],
    );

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0101"
            && diagnostic.span.file == "plain.rss"
            && diagnostic.summary.contains("features: local")
    }));
    assert!(!diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0101" && diagnostic.span.file == "capability.rss"
    }));
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
    let source =
        r#"fn build(create: noescape Fn(Url, Int) -> Result<DbConnection, DbError>) -> Unit"#;
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
            .fn_params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Url", "Int"]
    );
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
fn parser_preserves_noescape_fresh_function_return_types() {
    let source =
        r#"fn build(create: noescape Fn() -> Result<fresh DbConnection, DbError>) -> Unit"#;
    let program = parse_source("fn-fresh-return-type.rssi", source);
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
    let ok_ty = create
        .fn_return
        .as_ref()
        .and_then(|return_ty| return_ty.args.first())
        .expect("Result ok type should parse");

    assert!(create.is_noescape);
    assert_eq!(create.name, "Fn");
    assert_eq!(ok_ty.name, "DbConnection");
    assert!(ok_ty.is_fresh);
}

#[test]
fn parser_keeps_top_level_fresh_as_function_contract() {
    let source = r#"fn make() -> fresh DbConnection"#;
    let program = parse_source("top-level-fresh.rssi", source);
    let function = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .find(|function| function.name == "make")
        .expect("function should parse");
    let return_ty = function
        .return_ty
        .as_ref()
        .expect("return type should parse");

    assert!(function.returns_fresh);
    assert_eq!(return_ty.name, "DbConnection");
    assert!(!return_ty.is_fresh);
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
fn file_core_resource_producers_return_result_contracts() {
    let program = core_interfaces()
        .iter()
        .find_map(|(file, source)| (file.contains("file.rssi")).then(|| parse_source(file, source)))
        .expect("File interface should be available");
    let functions = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();

    for name in ["File.open", "File.open_read", "File.open_write"] {
        let return_ty = functions
            .iter()
            .find(|function| function.name == name)
            .and_then(|function| function.return_ty.as_ref())
            .unwrap_or_else(|| panic!("{name} should have a return type"));
        assert_eq!(return_ty.name, "Result", "{name}");
        assert_eq!(
            return_ty.args.first().map(|arg| arg.name.as_str()),
            Some("File"),
            "{name}"
        );
        assert_eq!(
            return_ty.args.get(1).map(|arg| arg.name.as_str()),
            Some("FileError"),
            "{name}"
        );
    }
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
fn checker_accepts_and_lowers_list_for_loop() {
    let source = r#"
fn run(items: read List<Int>) -> Unit {
    for item in items {
        let copy = item
    }
}
"#;
    let diagnostics = analyze_source("for.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("for.rss", source).expect("for should lower");
    assert!(lowered.contains("for item in (items).iter().cloned()"));
    assert!(lowered.contains("let copy = item;"));
}

#[test]
fn rust_lowering_for_loop_uses_read_view_for_non_copy_items() {
    let source = r#"
struct ReviewFacts {
    name: String
}

fn first(items: read List<ReviewFacts>) -> String {
    for facts in items {
        return facts.name
    }
    return "none"
}
"#;
    let diagnostics = analyze_source("for-read-view.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("for-read-view.rss", source).expect("for should lower");
    assert!(lowered.contains("for facts in (items).iter()"));
    assert!(!lowered.contains("for facts in (items).iter().cloned()"));
}

#[test]
fn checker_reports_sum_type_variant_mismatch_in_match() {
    let source = r#"
sum Color {
    Red
    Green
    Blue
}

sum Size {
    Small
    Medium
    Large
}

fn describe(s: read Size) -> String {
    match s {
        Red => { return "red" }
        Green => { return "green" }
        Blue => { return "blue" }
    }
}
"#;
    let diagnostics = analyze_source("sum-match-mismatch.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0209" && d.summary.contains("cannot match scrutinee type")),
        "should reject matching Size with Color variants: {diagnostics:?}"
    );
}

#[test]
fn checker_accepts_correct_sum_type_match() {
    let source = r#"
sum Color {
    Red
    Green
    Blue
}

fn describe(c: read Color) -> String {
    match c {
        Red => { return "red" }
        Green => { return "green" }
        Blue => { return "blue" }
    }
}
"#;
    let diagnostics = analyze_source("sum-match-correct.rss", source);
    assert!(
        diagnostics.is_empty(),
        "should accept matching Color with Color variants: {diagnostics:?}"
    );
}

#[test]
fn type_alias_chain_resolves_correctly() {
    let source = r#"
type MyString = String
type Alias = MyString

fn greet(name: read Alias) -> Alias {
    return name
}
"#;
    let diagnostics = analyze_source("alias-chain.rss", source);
    // Should not report unknown type for Alias since it resolves through MyString to String
    assert!(
        !diagnostics.iter().any(|d| d.code == "RS0024"),
        "should resolve type alias chain: {diagnostics:?}"
    );
}

#[test]
fn task_group_with_async_let_passes_checker() {
    let source = r#"
features: async

struct NetworkError { message: String }

async fn fetch_user(id: read Int) -> Result<String, NetworkError> {
    return Ok("user")
}

async fn fetch_profile(id: read Int) -> Result<String, NetworkError> {
    return Ok("profile")
}

async fn load(id: read Int) -> Result<String, NetworkError> {
    task_group {
        async let user = fetch_user(id: read id)
        async let profile = fetch_profile(id: read id)

        let u = await user?
        let p = await profile?
    }
    return Ok("done")
}
"#;
    let diagnostics = analyze_source("task-group-async-let.rss", source);
    let errors: Vec<_> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(
        errors.is_empty(),
        "task_group with async let should pass: {errors:?}"
    );
}

#[test]
fn async_let_outside_task_group_is_rejected() {
    let source = r#"
features: async

async fn fetch(id: read Int) -> Int

async fn run(id: read Int) -> Int {
    async let result = fetch(id: read id)
    return await result
}
"#;
    let diagnostics = analyze_source("async-let-outside.rss", source);
    assert!(
        diagnostics.iter().any(|d| d.code == "RS0015"
            && d.causes.iter().any(|c| c.contains("async let"))),
        "async let outside task_group should be rejected: {diagnostics:?}"
    );
}

#[test]
fn task_group_requires_async_feature() {
    let source = r#"
fn run() -> Int {
    task_group {
        let x = 1
    }
    return x
}
"#;
    let diagnostics = analyze_source("task-group-no-feature.rss", source);
    assert!(
        diagnostics.iter().any(|d| d.code == "RS0101"),
        "task_group without features: async should be rejected: {diagnostics:?}"
    );
}
