//! frontend checks not yet categorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn qualified_module_value_access_checks_clean_through_the_checker() {
    // Regression guard: qualified `module.CONST` / `module.Variant` in value
    // position must resolve through the *semantic checker* on a merged multi-file
    // program — not only through the lowering helper. (`module-value-access.md`.)
    let diagnostics = analyze_sources_with_interfaces(
        &[
            (
                "ops.rss",
                "module ops\n\nsum Ops { ADD MUL OTHER }\n\nconst MAX_OPS: Int = 64\n",
            ),
            (
                "user.rss",
                concat!(
                    "module user\n\n",
                    "use ops.Ops\n\n",
                    "fn c() -> fresh Ops { return ops.MUL }\n\n",
                    "fn e() -> Int { return ops.MAX_OPS }\n",
                ),
            ),
        ],
        &[],
    );
    assert_eq!(
        diagnostics,
        Vec::new(),
        "qualified module value access should check clean: {diagnostics:?}"
    );
}

#[test]
fn qualified_variant_in_match_pattern_checks_clean() {
    // `ops.ADD` / `ops.MUL` as match patterns resolve through the merged checker
    // (parser accepts the dotted pattern; isolation rewrites it to the bare
    // variant). (`module-qualified-variant-pattern.md`.)
    let diagnostics = analyze_sources_with_interfaces(
        &[
            ("ops.rss", "module ops\n\nsum Ops { ADD MUL OTHER }\n"),
            (
                "app.rss",
                concat!(
                    "module app\n\n",
                    "use ops.Ops\n\n",
                    "fn classify(o: read Ops) -> Int {\n",
                    "    match read o {\n",
                    "        ops.ADD => { return 1 }\n",
                    "        ops.MUL => { return 2 }\n",
                    "        _ => { return 0 }\n",
                    "    }\n",
                    "}\n",
                ),
            ),
        ],
        &[],
    );
    assert_eq!(
        diagnostics,
        Vec::new(),
        "qualified variant in match pattern should check clean: {diagnostics:?}"
    );
}

#[test]
fn read_qualified_module_call_parses_as_read_of_call() {
    // `read m.fn(args)` in argument position is read-of-the-call, not a receiver
    // call on the module — identical to `read flat()` / `read (m.fn())`.
    let diagnostics = analyze_sources_with_interfaces(
        &[
            (
                "m.rss",
                "module m\n\nfn order_names() -> fresh String { return \"x\" }\n",
            ),
            (
                "app.rss",
                concat!(
                    "module app\n\n",
                    "fn sink(v: read String) -> Unit { return Unit }\n\n",
                    "fn use_it() -> Unit { return sink(v: read m.order_names()) }\n",
                ),
            ),
        ],
        &[],
    );
    assert_eq!(
        diagnostics,
        Vec::new(),
        "`read m.fn()` should resolve as read-of-call: {diagnostics:?}"
    );
}

#[test]
fn glob_import_brings_module_symbols_into_scope() {
    // `use ops.*` imports the module's type, const, and functions; bare variants
    // resolve globally. The snippet checks clean. (`module-glob-import.md`.)
    let diagnostics = analyze_sources_with_interfaces(
        &[
            (
                "ops.rss",
                "module ops\n\nsum Ops { ADD MUL OTHER }\n\nconst MAX_OPS: Int = 64\n\nfn helper() -> Int { return 1 }\n",
            ),
            (
                "app.rss",
                concat!(
                    "module app\n\n",
                    "use ops.*\n\n",
                    "fn pick() -> fresh Ops { return ADD }\n\n",
                    "fn lim() -> Int { return MAX_OPS }\n\n",
                    "fn classify(o: read Ops) -> Int {\n",
                    "    match read o {\n",
                    "        ADD => { return 1 }\n",
                    "        _ => { return helper() }\n",
                    "    }\n",
                    "}\n",
                ),
            ),
        ],
        &[],
    );
    assert_eq!(
        diagnostics,
        Vec::new(),
        "glob import should bring module symbols into scope: {diagnostics:?}"
    );
}

#[test]
fn pass_fixtures_have_no_diagnostics() {
    for path in common::fixture_paths("tests/fixtures/pass") {
        let source = common::read_fixture(&path);
        let diagnostics = analyze_source(path.to_str().unwrap(), &source);
        assert_eq!(diagnostics, Vec::new(), "{}", path.display());
    }
}

#[test]
fn inherent_impl_block_desugars_to_qualified_methods() {
    // `impl Type { fn m(<effect> self, ...) }` is parse-time sugar for flat
    // `fn Type.m(self: <effect> Type, ...)`. Shorthand `mut self`/`read self`
    // fills the receiver type from the block header; the explicit
    // `self: read Type` form is also accepted. Receiver calls resolve to the
    // desugared methods and `mut self` reassigns fields with caller write-back,
    // so a program that builds, mutates, and reads through the block checks clean.
    //
    // NOTE: this is an inline test, not a `tests/fixtures/pass/*.rss` file, on
    // purpose — that directory is part of the self-hosting parity corpus
    // (`selfhost_parity`), and the self-hosted parser/checker do not yet
    // recognize `impl` blocks. A corpus fixture can be added once they do.
    let source = concat!(
        "features: local\n\n",
        "struct Tally {\n    n: Int\n}\n\n",
        "impl Tally {\n",
        "    fn bump(mut self) -> Unit {\n        self.n = self.n + 1\n    }\n\n",
        "    fn add(mut self, delta: read Int) -> Unit {\n        self.n = self.n + delta\n    }\n\n",
        "    fn get(read self) -> Int {\n        return self.n\n    }\n\n",
        "    fn get_explicit(self: read Tally) -> Int {\n        return self.n\n    }\n",
        "}\n\n",
        "fn main() -> Unit {\n",
        "    let mut t = Tally(n: 0)\n",
        "    mut t.bump()\n",
        "    mut t.add(delta: read 10)\n",
        "    let total = t.get()\n",
        "    let same = t.get_explicit()\n",
        "    Log.write(message: read Int.to_string(value: read total))\n",
        "    Log.write(message: read Int.to_string(value: read same))\n",
        "}\n",
    );
    let diagnostics = analyze_source("inherent-impl-block.rss", source);
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
}

#[test]
fn protocol_impl_block_still_parses_after_inherent_impl() {
    // The inherent-vs-protocol split keys on the `for` keyword, so
    // `impl Protocol for Type { ... }` must still route to protocol-impl parsing.
    let source = concat!(
        "features: local\n\n",
        "protocol Named {\n    fn name(self: read Self) -> fresh String\n}\n\n",
        "struct Point {\n    x: Int\n}\n\n",
        "fn Point.name(self: read Point) -> fresh String {\n",
        "    return String.concat(left: read \"p\", right: read \"!\")\n}\n\n",
        "impl Named for Point {\n    name = Point.name\n}\n",
    );
    let diagnostics = analyze_source("protocol-after-inherent.rss", source);
    assert_eq!(diagnostics, Vec::new(), "{diagnostics:?}");
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
fn char_literal_is_a_real_char_value_and_type_checks() {
    // SH-016: `'x'` is now a real `Char` value (not a diagnostic). Comparing a
    // `Char` binding against a char literal type-checks cleanly, with neither the
    // old RS0015 ("character literal") nor any RS0013 (try-operator) cascade.
    let source = "fn f(c: read Char) -> Bool {\n    return c == '_'\n}\n";
    let codes = common::error_codes("char-literal.rss", source);
    assert!(
        !codes.iter().any(|code| code == "RS0015"),
        "char literal must no longer emit RS0015, got {codes:?}"
    );
    assert!(
        !codes.iter().any(|code| code == "RS0013"),
        "RS0013 try-operator cascade must be gone, got {codes:?}"
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

#[test]
fn missing_data_effect_fix_carries_machine_applicable_edit() {
    // `read` is the default, but an omitted exclusive effect must still carry a
    // concrete machine-applicable edit so `rss fix` can restore the contract.
    let source = concat!(
        "fn use_it(value: mut String) -> Unit {\n",
        "    return Unit\n",
        "}\n",
        "fn main() -> Unit {\n",
        "    let mut v = \"x\"\n",
        "    use_it(value: v)\n",
        "    return Unit\n",
        "}\n",
    );
    let diagnostics = analyze_source("fix-edit.rss", source);
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "RS0202")
        .expect("missing-data-effect diagnostic");
    let fix = diagnostic
        .fixes
        .iter()
        .find(|fix| fix.kind == "add_data_effect")
        .expect("add_data_effect fix");
    assert_eq!(fix.applicability, "machine-applicable");
    let edit = fix.edit.as_ref().expect("fix carries a concrete edit");
    assert_eq!(edit.replacement, "mut ");
    assert_eq!(edit.span.length, 0, "an insertion has zero-length span");
    assert_eq!(edit.span.line, 6, "edit points at the call argument line");
}
