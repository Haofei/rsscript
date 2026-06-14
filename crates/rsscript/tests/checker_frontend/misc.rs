//! frontend checks not yet categorized
#![allow(unused_imports, dead_code)]
use super::*;

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
fn uninferable_binding_diagnostic_is_sound() {
    // RS0034 fires only for a bare Ok/Err/None bound to an *unused* name (the
    // type param is then provably unconstrainable). It must NOT fire when an
    // annotation or downstream use pins the param, nor for the fully-determined
    // `Some(x)`. Found-and-fixed via rss-testgen; this locks in no-false-positives.
    let has_rs0034 = |src: &str| {
        common::error_codes("uninferable.rss", src)
            .iter()
            .any(|code| code == "RS0034")
    };

    // Positive: unused bare Ok/Err/None.
    for value in ["Ok(1.0)", "Err(\"e\")", "None"] {
        let src = format!(
            "fn main() -> Unit {{\n    let v = {value}\n    Log.write(message: read \"x\")\n    return Unit\n}}\n"
        );
        assert!(
            has_rs0034(&src),
            "expected RS0034 for unused `{value}`:\n{src}"
        );
    }

    // Negative: fully determined, annotated, or constrained by downstream use.
    let sound_cases = [
        "fn main() -> Unit {\n    let v = Some(1.0)\n    Log.write(message: read \"x\")\n    return Unit\n}\n",
        "fn main() -> Unit {\n    let v: Result<Float, String> = Ok(1.0)\n    Log.write(message: read \"x\")\n    return Unit\n}\n",
        "fn pick() -> Result<Int, String> {\n    let v = Ok(1)\n    return v\n}\nfn main() -> Unit {\n    let _ = pick()\n    Log.write(message: read \"x\")\n    return Unit\n}\n",
    ];
    for src in sound_cases {
        assert!(
            !has_rs0034(src),
            "RS0034 false-positive on a constrained binding:\n{src}"
        );
    }
}

#[test]
fn core_interface_files_have_no_diagnostics() {
    for path in common::recursive_fixture_paths("stdlib") {
        let source = common::read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn examples_have_no_diagnostics_and_lower_to_runnable_packages() {
    for path in common::recursive_fixture_paths("examples/scripts") {
        let source = common::read_fixture(&path);
        let diagnostics = analyze_source_with_core(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());

        let package = lower_source_to_rust_package(
            path.to_str().unwrap(),
            &source,
            path.file_stem().and_then(|stem| stem.to_str()).unwrap(),
            &common::runtime_path(),
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
fn builtin_scalar_map_keys_remain_accepted() {
    // Widening Map/Set keys to `K: Hashable` must not regress the common
    // String/Int key cases.
    let source = r#"
fn main() -> Unit {
    let by_name = Map.new<String, Int>()
    Map.insert(map: mut by_name, key: read "a", value: read 1)
    let by_id = Map.new<Int, String>()
    Map.insert(map: mut by_id, key: read 7, value: read "seven")
    let ids = Set.new<Int>()
    let added = Set.insert(set: mut ids, value: read 7)
    return Unit
}
"#;
    assert_eq!(
        analyze_source_with_core("builtin-keys.rss", source),
        Vec::new()
    );
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
fn checker_rejects_if_is_binding_outside_then_scope() {
    let source = r#"
sum Expr {
    Call(callee: String)
    Name(value: String)
}

fn main(expr: read Expr) -> String {
    if read expr is Call { callee } {
        return callee
    }
    return callee
}
"#;
    let diagnostics = analyze_source("if-is-scope.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0026" && diagnostic.summary.contains("`callee`")
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
fn receiver_call_does_not_resolve_through_dependency_interface_protocol_impl() {
    let interface = r#"
protocol Formatter {
    fn format(self: read Self) -> fresh String
}

struct Report {
    title: String
}

pub fn ReportFormatter.format(self: read Report) -> fresh String

impl Formatter for Report {
    format = ReportFormatter.format
}
"#;
    let source = r#"
fn display(report: read Report) -> fresh String {
    return read report.format()
}
"#;
    let diagnostics =
        analyze_source_with_interfaces("caller.rss", source, &[("formatter.rssi", interface)]);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0206" && diagnostic.summary.contains("read report.format")
        }),
        "dependency interface protocol impls must not make receiver shorthand resolve: {diagnostics:#?}"
    );
}

#[test]
fn receiver_call_still_resolves_through_explicit_protocol_bound_from_interface() {
    let interface = r#"
protocol Formatter {
    fn format(self: read Self) -> fresh String
}
"#;
    let source = r#"
fn display<T: Formatter>(value: read T) -> fresh String {
    return read value.format()
}
"#;

    assert_eq!(
        analyze_source_with_interfaces("caller.rss", source, &[("formatter.rssi", interface)]),
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
