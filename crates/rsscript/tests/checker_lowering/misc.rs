//! lowering tests not yet categorized
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn keyword_named_source_lowers_to_a_valid_crate_name() {
    // Regression (found while building rss-testgen's async tier): a source whose
    // name is a Rust keyword (e.g. `async.rss`) derived a keyword crate name, so
    // the harness emitted `async::main()` — a Rust parse error (RS1102), and both
    // Cargo and rustc reject keyword crate names. The package/lib/crate-reference
    // names must all be sanitized.
    let source = "fn main() -> Unit {\n    Log.write(message: read \"ok\")\n    return Unit\n}\n";
    let package = lower_source_to_rust_package("async.rss", source, "async", "/tmp/runtime")
        .expect("keyword-named source should lower");

    assert_ne!(
        package.package_name, "async",
        "package name must not be the bare keyword"
    );
    let main_rs = package.main_rs.expect("runnable main");
    assert!(
        main_rs.contains("rss_async::main"),
        "main.rs should reference the sanitized crate name, not the keyword:\n{main_rs}"
    );
    // No bare keyword crate path (e.g. a line starting `    async::`).
    assert!(
        !main_rs.contains(" async::") && !main_rs.contains("\nasync::"),
        "main.rs must not reference a bare keyword crate:\n{main_rs}"
    );
}

#[test]
fn module_isolation_lets_two_modules_share_a_symbol_name() {
    // `helpers.count` and `device.count` are distinct module-scoped symbols, so
    // both must lower to unique Rust functions rather than colliding (RS0005 /
    // rustc E0428). A bare call resolves to the `use`-imported module's symbol.
    let sources = vec![
        (
            "helpers.rss".to_string(),
            "module helpers\n\nfn count() -> Int {\n    return 10\n}\n".to_string(),
        ),
        (
            "device.rss".to_string(),
            "module device\n\nfn count() -> Int {\n    return 20\n}\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "use helpers.count\n\n",
                "fn main() -> Unit {\n",
                "    Log.write(message: read String.from_int(value: count()))\n",
                "    return Unit\n",
                "}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "ns-demo", "/rt", &[], &[])
            .expect("multi-module package with a shared symbol name should lower");
    // Both modules' `count` coexist as distinct, module-qualified Rust symbols.
    assert!(
        package.lib_rs.contains("fn helpers__count("),
        "helpers.count should lower to a module-qualified symbol:\n{}",
        package.lib_rs
    );
    assert!(
        package.lib_rs.contains("fn device__count("),
        "device.count should lower to a module-qualified symbol:\n{}",
        package.lib_rs
    );
    // The bare `count()` in module `app` resolves to the imported `helpers.count`.
    assert!(
        package.lib_rs.contains("helpers__count()"),
        "the imported bare call should resolve to helpers__count:\n{}",
        package.lib_rs
    );
    // The runnable entry point keeps its global `main` symbol.
    assert!(
        package.lib_rs.contains("fn main("),
        "the entry point must stay `main`:\n{}",
        package.lib_rs
    );
}

#[test]
fn module_isolation_resolves_cross_file_associated_constant() {
    // `Device.DEFAULT` is declared in one module and read from another (via
    // `use device.Device`). The associated-const access must resolve to the
    // module-qualified, upper-cased constant rather than a dangling field access.
    let sources = vec![
        (
            "device.rss".to_string(),
            "module device\n\nstruct Device {\n    name: String\n}\n\nconst Device.DEFAULT: String = \"cpu\"\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "use device.Device\n\n",
                "fn main() -> Unit {\n",
                "    Log.write(message: read Device.DEFAULT)\n",
                "    return Unit\n",
                "}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "assoc-demo", "/rt", &[], &[])
            .expect("cross-file associated constant should lower");
    // The constant lowers to a module-qualified SCREAMING_SNAKE symbol, used at
    // both declaration and the cross-file reference.
    assert!(
        package.lib_rs.contains("DEVICE__DEVICE_DEFAULT"),
        "associated const should lower to a module-qualified symbol:\n{}",
        package.lib_rs
    );
}

#[test]
fn qualified_module_value_access_resolves_const_and_variant() {
    // `ops.MAX_OPS` (constant) and `ops.MUL` (sum variant) resolve from another
    // module in value position, without a per-symbol `use`. The const lowers to
    // its module-mangled SCREAMING_SNAKE symbol; the variant resolves through its
    // (module-mangled) sum type.
    let sources = vec![
        (
            "ops.rss".to_string(),
            "module ops\n\nsum Ops {\n    ADD\n    MUL\n}\n\nconst MAX_OPS: Int = 64\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "use ops.Ops\n\n",
                "fn pick() -> fresh Ops {\n",
                "    return ops.MUL\n",
                "}\n\n",
                "fn limit() -> Int {\n",
                "    return ops.MAX_OPS\n",
                "}\n\n",
                "fn main() -> Unit {\n",
                "    return Unit\n",
                "}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "ns-qualval", "/rt", &[], &[])
            .expect("qualified module value access should lower");
    assert!(
        package.lib_rs.contains("OPS__MAX_OPS"),
        "`ops.MAX_OPS` should lower to the module-mangled const symbol:\n{}",
        package.lib_rs
    );
    assert!(
        package.lib_rs.contains("ops__Ops") && package.lib_rs.contains("MUL"),
        "`ops.MUL` should resolve through the module-mangled sum type:\n{}",
        package.lib_rs
    );
}

#[test]
fn qualified_module_type_in_type_position_resolves() {
    // `dtype.DType` in a parameter type resolves to the module-scoped type symbol
    // without requiring a `use dtype.DType` line in the consuming file.
    let sources = vec![
        (
            "dtype.rss".to_string(),
            "module dtype\n\nstruct DType {\n    bits: Int\n}\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "fn bits_of(d: read dtype.DType) -> Int {\n",
                "    return d.bits\n",
                "}\n\n",
                "fn main() -> Unit {\n",
                "    return Unit\n",
                "}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "ns-qualtype", "/rt", &[], &[])
            .expect("qualified module.Type in type position should lower");
    assert!(
        package.lib_rs.contains("dtype__DType"),
        "qualified `dtype.DType` should lower to the module-scoped type symbol:\n{}",
        package.lib_rs
    );
}

#[test]
fn module_isolation_distinguishes_dotted_and_underscored_module_paths() {
    // `module a.b` and `module a_b` are distinct and must not collide (RS0005).
    let sources = vec![
        (
            "ab.rss".to_string(),
            "module a.b\n\nfn count() -> Int {\n    return 1\n}\n".to_string(),
        ),
        (
            "a_b.rss".to_string(),
            "module a_b\n\nfn count() -> Int {\n    return 2\n}\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            "module app\n\nfn main() -> Unit {\n    return Unit\n}\n".to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "collide-demo", "/rt", &[], &[])
            .expect("distinct module paths should lower without collision");
    assert!(
        package.lib_rs.contains("fn a_b__count(") && package.lib_rs.contains("fn a__b__count("),
        "distinct module paths must yield distinct symbols:\n{}",
        package.lib_rs
    );
}

#[test]
fn aliasing_lets_one_file_import_two_same_leaf_symbols() {
    // `helpers.count` and `device.count` share a leaf; `use ... as` binds each to
    // a distinct local name so one file can reference both unambiguously.
    let sources = vec![
        (
            "helpers.rss".to_string(),
            "module helpers\n\nfn count() -> Int {\n    return 10\n}\n".to_string(),
        ),
        (
            "device.rss".to_string(),
            "module device\n\nfn count() -> Int {\n    return 20\n}\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "use helpers.count as helpers_count\n",
                "use device.count as device_count\n\n",
                "fn main() -> Unit {\n",
                "    Log.write(message: read String.from_int(value: helpers_count()))\n",
                "    Log.write(message: read String.from_int(value: device_count()))\n",
                "    return Unit\n",
                "}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "ns-alias", "/rt", &[], &[])
            .expect("aliased multi-module package should lower");
    assert!(
        package.lib_rs.contains("helpers__count()"),
        "the `helpers_count` alias should resolve to helpers__count:\n{}",
        package.lib_rs
    );
    assert!(
        package.lib_rs.contains("device__count()"),
        "the `device_count` alias should resolve to device__count:\n{}",
        package.lib_rs
    );
}

#[test]
fn qualified_module_calls_disambiguate_same_leaf_symbols() {
    // Without any `use`, a `module.fn()` call names the owner at the call site and
    // resolves to that module's symbol — even though it parses as a receiver call.
    let sources = vec![
        (
            "helpers.rss".to_string(),
            "module helpers\n\nfn count() -> Int {\n    return 10\n}\n".to_string(),
        ),
        (
            "device.rss".to_string(),
            "module device\n\nfn count() -> Int {\n    return 20\n}\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "fn main() -> Unit {\n",
                "    Log.write(message: read String.from_int(value: helpers.count()))\n",
                "    Log.write(message: read String.from_int(value: device.count()))\n",
                "    return Unit\n",
                "}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "ns-qual", "/rt", &[], &[])
            .expect("qualified-call multi-module package should lower");
    assert!(
        package.lib_rs.contains("helpers__count()"),
        "`helpers.count()` should resolve to helpers__count:\n{}",
        package.lib_rs
    );
    assert!(
        package.lib_rs.contains("device__count()"),
        "`device.count()` should resolve to device__count:\n{}",
        package.lib_rs
    );
}

#[test]
fn lower_name_pin_renames_definition_and_call_sites() {
    let source = r#"
#lower_name("helpers__count")
fn count(value: read Int) -> Int {
    return value
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: count(value: read 3)))
    return Unit
}
"#;
    let rust = lower_source_to_rust("helpers.rss", source).expect("source should lower");
    // The pinned name is used at the definition...
    assert!(
        rust.contains("fn helpers__count("),
        "definition should use the pinned name:\n{rust}"
    );
    // ...and at the call site, with no leftover default `count` symbol.
    assert!(
        rust.contains("helpers__count("),
        "call site should use the pinned name:\n{rust}"
    );
    assert!(
        !rust.contains("fn count("),
        "the default symbol must not be emitted:\n{rust}"
    );
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
    assert!(rust.contains("return User_lookup(name);"));
    assert!(!rust.contains("fn User.lookup"));
    assert!(!rust.contains("User::lookup"));
}

#[test]
fn rust_lowering_raw_escapes_reserved_user_function_names() {
    let source = r#"
fn gen(x: Int) -> Int {
    return x + 1
}

fn main() -> Int {
    return gen(x: 2)
}
"#;
    let rust = lower_source_to_rust("reserved-fn.rss", source).expect("source should lower");

    assert!(rust.contains("fn r#gen(x: i64) -> i64"), "{rust}");
    assert!(rust.contains("return r#gen(2i64);"), "{rust}");
    assert!(!rust.contains("fn gen("), "{rust}");
}

#[test]
fn checker_rejects_capability_object_without_visible_protocol_impl() {
    let source = r#"
protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct NotWriter {
    count: Int
}

fn write_dynamic(writer: take NotWriter) -> Unit {
    local cap: Capability<Writer> = Capability<Writer>.from(value: take writer)
}
"#;
    let diagnostics = analyze_source_with_core("capability-reject.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0032"
                && diagnostic.summary.contains("Capability<Writer>")),
        "{diagnostics:?}"
    );
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
fn syntax_parser_accepts_all_fixtures() {
    let mut paths = common::fixture_paths("tests/fixtures/pass");
    paths.extend(common::fixture_paths("tests/fixtures/fail"));
    paths.extend(common::recursive_fixture_paths("examples/scripts"));

    for path in paths {
        let source = common::read_fixture(&path);
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
                    Item::Type(_)
                    | Item::Module(_)
                    | Item::Use(_)
                    | Item::SumType(_)
                    | Item::TypeAlias(_)
                    | Item::Const(_) => false,
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
fn parser_accepts_protocol_contracts_as_qualified_call_signatures() {
    let source = r#"
protocol Writer {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn write_line<W: Writer>(
    writer: mut W,
    message: read String,
) -> Unit {
    Writer.write(self: mut writer, message: read message)
}
"#;
    let diagnostics = analyze_source("protocol.rss", source);
    assert_eq!(diagnostics, Vec::new());
}

#[test]
fn parser_accepts_protocol_default_impl_marker_without_body() {
    let source = r#"
protocol Writer {
    fn flush(self: mut Self) -> Unit = _
}

struct BufferWriter

fn BufferWriter.flush(self: mut BufferWriter) -> Unit {
    return Unit
}

impl Writer for BufferWriter {
    flush = BufferWriter.flush
}
"#;
    let program = parse_source("protocol-default-marker.rss", source);
    let marker = program.items.iter().any(|item| {
        matches!(item, Item::Function(function) if function.name == "Writer.flush" && function.default_impl_marker && !function.has_body)
    });
    assert!(marker, "protocol method should retain `= _` marker");
    assert_eq!(
        analyze_source("protocol-default-marker.rss", source),
        Vec::new()
    );
}

#[test]
fn checker_rejects_default_impl_marker_outside_protocol_contract() {
    let source = r#"
fn flush() -> Unit = _
"#;
    let diagnostics = analyze_source("default-marker-outside-protocol.rss", source);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported default implementation marker"
        }),
        "{diagnostics:#?}"
    );
}

#[test]
fn checker_requires_protocol_methods_to_declare_self_first() {
    let source = r#"
protocol Writer {
    fn write(message: read String) -> Unit
}
"#;
    let diagnostics = analyze_source("protocol-self.rss", source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0028"
            && diagnostic
                .summary
                .contains("invalid `self` parameter in `Writer.write`")
    }));
}

#[test]
fn checker_requires_protocol_bounds_to_name_declared_protocols() {
    let source = r#"
fn write_line<W: MissingWriter>(
    writer: mut W,
    message: read String,
) -> Unit {
    MissingWriter.write(self: mut writer, message: read message)
}
"#;
    let diagnostics = analyze_source("unknown-protocol.rss", source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0027" && diagnostic.summary == "unknown protocol `MissingWriter`."
    }));
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
fn value_position_module_call_lowers_by_value_not_borrow() {
    // Regression: a bare `m.fn()` parses as a receiver call, but a module free
    // call has no receiver to borrow. It must lower to a VALUE call so the result
    // can be passed to a by-value parameter. Previously it was wrapped in an
    // effect and lowered to `&(m__make(..))`, which rustc rejected (E0308,
    // expected `i64` found `&i64`) only at `rss run` — `rss check` missed it.
    let sources = vec![
        (
            "m.rss".to_string(),
            "module m\n\nfn make(x: Int) -> Int {\n    return x + 1\n}\n".to_string(),
        ),
        (
            "main.rss".to_string(),
            concat!(
                "module app\n\n",
                "fn consume(y: Int) -> Int {\n    return y * 2\n}\n\n",
                "fn use_it(a: Int) -> Int {\n",
                "    let v = m.make(x: a)\n",
                "    return consume(y: v)\n",
                "}\n\n",
                "fn main() -> Unit {\n    return Unit\n}\n",
            )
            .to_string(),
        ),
    ];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "ns-modcall", "/rt", &[], &[])
            .expect("value-position module call should lower");
    // The module call collapses to the mangled free call...
    assert!(
        package.lib_rs.contains("m__make("),
        "module call should lower to the mangled free symbol:\n{}",
        package.lib_rs
    );
    // ...by VALUE: it must NOT be wrapped in a borrow.
    assert!(
        !package.lib_rs.contains("&(m__make("),
        "value-position module call must not lower to a borrow `&(m__make(..))`:\n{}",
        package.lib_rs
    );
}
