//! Spec §6 — type, constructor, and literal lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_allows_constructor_field_shorthand() {
    let source = r#"
struct Names {
    value: String
    count: Int
}

fn make(value: read String, count: Int) -> fresh Names {
    return Names(value, count)
}
"#;
    let rust = lower_source_to_rust("constructor-field-shorthand.rss", source)
        .expect("source should lower");

    assert!(
        rust.contains("return Names { value: value.clone(), count: count };"),
        "constructor field shorthand should bind by field name, got:\n{rust}"
    );
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
    assert!(rust.contains("return Some(42i64);"));
    assert!(rust.contains("return None;"));
}

#[test]
fn rust_lowering_instantiates_generic_constructor_field_types() {
    let source = r#"
struct Holder<T> {
    value: T
}

fn make_list() -> Holder<List<Int>> {
    return Holder<List<Int>>(value: [])
}

fn make_option() -> Holder<Option<Int>> {
    return Holder<Option<Int>>(value: None)
}

fn make_callback() -> Holder<owned Fn(Int) -> Int> {
    return Holder<owned Fn(Int) -> Int>(value: |value| value + 0)
}
"#;
    let rust = lower_source_to_rust("generic-constructor-fields.rss", source)
        .expect("generic constructor fields should lower");

    assert!(rust.contains("return Holder { value: vec![] };"), "{rust}");
    assert!(rust.contains("return Holder { value: None };"), "{rust}");
    assert!(
        rust.contains(
            "return Holder { value: std::rc::Rc::new(move |value: &i64| value + 0i64) };"
        ),
        "{rust}"
    );
}

#[test]
fn rust_lowering_expands_compiler_owned_derives() {
    let source = r#"
struct User derives(Clone, Eq, Ord, Hash, JsonEncode, JsonDecode, Schema) {
    id: Int
    name: String
}
"#;
    let rust = lower_source_to_rust("derive.rss", source).expect("source should lower");

    assert!(rust.contains(
        "#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]"
    ));
    assert!(rust.contains("pub struct User"));
}

#[test]
fn rust_lowering_supports_controlled_field_and_list_index_assignment() {
    let source = r#"
struct Account {
    balance: Int
}

fn run() -> Int {
    let mut account = Account(balance: 0)
    account.balance = 10

    let mut items = List<Int>.new()
    List.push(list: mut items, value: read 1)
    items[0] = account.balance
    return List.get(list: read items, index: 0)
}
"#;
    let rust = lower_source_to_rust("assign-compound.rss", source).expect("source should lower");

    assert!(rust.contains("let mut account = Account { balance: 0i64 };"));
    assert!(rust.contains("account.balance = 10i64;"));
    assert!(rust.contains("items[rsscript_runtime::checked_list_index(0i64)] = account.balance;"));
}

#[test]
fn rust_lowering_treats_json_literals_as_json_values_when_expected() {
    let source = r##"
fn accept(value: read JsonValue) -> String {
    return Json.to_string(value: read value)
}

fn make_object() -> fresh JsonValue {
    return {"role": "user", "content": "hello"}
}

fn main() -> String {
    let message: JsonValue = {"role": "assistant", "content": "ok"}
    let array_value: JsonValue = [{"name": "read_file"}]
    let rendered = accept(value: read {"tool_call_id": "call_1", "content": "done"})
    return Json.to_string(value: read message)
}
"##;
    let rust = lower_source_to_rust("json-value-literal.rss", source)
        .expect("JsonValue literal contexts should lower");

    assert!(rust.contains("rsscript_runtime::json_value(&rsscript_runtime::json_object"));
    assert!(rust.contains("rsscript_runtime::json_value(&rsscript_runtime::json_array"));
    assert!(rust.contains("fn make_object() -> rsscript_runtime::JsonValue"));
}

#[test]
fn rust_lowering_treats_array_literals_as_typed_lists_when_expected() {
    let source = r##"
fn accept(values: read List<JsonValue>) -> JsonValue {
    return values.to_json_values()
}

fn make_values() -> Result<fresh List<JsonValue>, JsonError> {
    return Ok([
        {"name": "read_file"},
        {"name": "write_file"},
    ])
}

fn main() -> JsonValue {
    let values: List<JsonValue> = [
        {"name": "read_file"},
        {"name": "write_file"},
    ]
    return accept([
        {"name": "check"},
    ])
}
"##;
    let rust = lower_source_to_rust("typed-list-literal.rss", source)
        .expect("typed List<JsonValue> literals should lower");

    assert!(rust.contains("let values: Vec<rsscript_runtime::JsonValue> = vec!["));
    assert!(rust.contains("rsscript_runtime::json_value(&rsscript_runtime::json_object"));
    assert!(rust.contains("accept(&vec![rsscript_runtime::json_value("));
    assert!(rust.contains("return Ok(vec![rsscript_runtime::json_value("));
}

#[test]
fn rust_lowering_maps_literals_and_json_encode_to_runtime_hooks() {
    let source = r#"
fn build(model: read String, prompt: read String) -> String {
    let nums = [1, 2, 3]
    let body = Json.encode(value: read {
        "model": model,
        "max_steps": 2,
        "stream": false,
        "messages": [
            {
                "role": "user",
                "content": prompt,
            },
        ],
    })
    return body
}
"#;
    let rust = lower_source_to_rust("json-literal.rss", source).expect("source should lower");

    assert!(
        rust.contains("let nums = vec![1i64, 2i64, 3i64];"),
        "list literal should lower to Vec, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::json_string_field(&\"model\".to_string(), model)"),
        "string object fields should use JSON string encoding, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::json_int_field(&\"max_steps\".to_string(), 2i64)"),
        "int object fields should use JSON int encoding, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::json_bool_field(&\"stream\".to_string(), false)"),
        "bool object fields should use JSON bool encoding, got:\n{rust}"
    );
    assert!(
        rust.contains("rsscript_runtime::json_raw_field(&\"messages\".to_string(), &rsscript_runtime::json_array"),
        "nested arrays should be embedded as raw JSON values, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_maps_typed_bool_local_in_json_literal() {
    let source = r#"
fn report(first: Bool, second: Bool) -> String {
    let ok: Bool = first && second
    return Json.encode(value: read {
        "ok": ok,
    })
}
"#;
    let rust = lower_source_to_rust("json-bool-local.rss", source).expect("source should lower");

    assert!(rust.contains("let ok = first && second;"));
    assert!(
        rust.contains("rsscript_runtime::json_bool_field(&\"ok\".to_string(), ok)"),
        "Json.encode should encode a typed Bool local as a JSON boolean, got:\n{rust}"
    );
}

#[test]
fn rust_lowering_preserves_multiline_string_literals() {
    let source = "fn prompt() -> String {\n    return \"\"\"First line\nSecond line\"\"\"\n}\n";
    let rust = lower_source_to_rust("multiline-string.rss", source).expect("source should lower");

    assert!(rust.contains("return \"First line\\nSecond line\".to_string();"));
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
fn rust_lowering_maps_try_operator_to_rust_result_propagation() {
    let source = r#"

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
fn rust_lowering_maps_identifiers_and_literals() {
    let source = r#"
fn demo() -> Unit {
    let value = 42
    let text = "ok"
    let rendered = String.from_int(value: value)
    return Unit
}
"#;
    let lowered =
        lower_source_to_rust_with_map("literals.rss", source).expect("source should lower");
    let kinds = lowered
        .source_map
        .iter()
        .map(|entry| entry.kind.as_str())
        .collect::<Vec<_>>();

    assert!(kinds.contains(&"number"));
    assert!(kinds.contains(&"string"));
    assert!(kinds.contains(&"ident"));
}

#[test]
fn rust_lowering_treats_map_literals_as_maps_when_expected() {
    let source = r#"
fn main() -> Int {
    let names: Map<Int, String> = { 1 => "one", 2 => "two" }
    let payloads: Map<Int, JsonValue> = { 7 => {"ok": true} }
    return Map.len(map: read names)
}
"#;
    let rust = lower_source_to_rust("map-literal-lower.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::map_from_entries(vec![(1i64, \"one\".to_string()), (2i64, \"two\".to_string())])"));
    assert!(rust.contains("rsscript_runtime::map_from_entries(vec![(7i64, rsscript_runtime::json_value(&rsscript_runtime::json_object"));
}

#[test]
fn review_map_marks_external_binding_construction_and_dynamic_protocol_dispatch() {
    let source = r#"
protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Output.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn write_dynamic(writer: take BufferWriter, message: read String) -> Unit {
    local cap: Dyn<Writer> = Dyn<Writer>.from(value: take writer)
    mut cap.write(message: read message)
}
"#;
    let map = review_map_sources(vec![("external_binding-review.rss", source)]);
    let region = map.files[0]
        .regions
        .iter()
        .find(|region| region.function == "write_dynamic")
        .expect("write_dynamic should be present");

    assert_eq!(
        region.classification,
        ReviewMapClassification::ReviewRequired
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "dynamic protocol object construction")
    );
    assert!(
        region
            .reasons
            .iter()
            .any(|reason| reason == "dynamic protocol dispatch")
    );
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
    // A `main` returning `Err` reports to stderr and exits non-zero rather than
    // panicking (ledger SH-005).
    assert!(main_rs.contains("if let Err(error) = result_runnable_example_rss::main()"));
    assert!(main_rs.contains("std::process::exit(1);"));
}

#[test]
fn checker_reports_malformed_generic_parameters_as_unsupported() {
    let source = r#"
struct Box<T: Unknown + Other> {
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
fn checker_reports_unclosed_delimiters_on_own_construct() {
    let unclosed_body = r#"
fn main() -> Unit {
    let x = 1
"#;
    let diagnostics = analyze_source("unclosed-function-body.rss", unclosed_body);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0015"
                && diagnostic.label == "malformed declaration"
                && diagnostic.span.line == 2
        }),
        "{diagnostics:?}"
    );

    let unclosed_call = r#"
fn main() -> Unit {
    let x = Output.write(message: read "hello"
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
fn const_string_lowers_to_static_str() {
    let source = r#"
const GREETING: String = "hello"

fn main() -> Unit {
    let x = GREETING
}
"#;
    let lowered =
        lower_source_to_rust("const-string.rss", source).expect("const String should lower");
    assert!(
        lowered.contains("&'static str"),
        "const String should lower to &'static str, got:\n{lowered}"
    );
    assert!(
        !lowered.contains("const GREETING: String"),
        "should not produce `const X: String` in Rust, got:\n{lowered}"
    );
}

#[test]
fn typed_let_emits_generic_container_annotation() {
    // A `let` annotated with a builtin generic container carries that annotation
    // into Rust, so an otherwise-unconstrained generic (RssChannel<_>) resolves
    // instead of failing with E0282 `type annotations needed`.
    let source = r#"

fn run() -> Result<Unit, ChannelError> {
    let mut ch: Channel<Int> = Channel.bounded(capacity: 1)?
    let rx = Channel.receiver(channel: mut ch)?
    return Ok(Unit)
}
"#;
    let lowered =
        lower_source_to_rust("typed-channel.rss", source).expect("typed channel let should lower");
    assert!(
        lowered.contains("let mut ch: rsscript_runtime::RssChannel<i64> ="),
        "typed Channel<Int> let should emit a concrete Rust annotation, got:\n{lowered}"
    );
}

#[test]
fn typed_let_emits_deque_and_sorted_container_annotations() {
    // Regression (found by rss-testgen): `Deque`, `SortedMap`, and `SortedSet`
    // were missing from the generic-container annotation allowlist, so a
    // `let v: Deque<Float> = Deque.new<Float>()` whose only use was `len()`
    // dropped its annotation and lowered to an un-inferable `VecDeque<_>`
    // (E0282) — accepted by the checker but un-compilable. Every generic
    // container must carry its annotation into Rust.
    for (decl, expected) in [
        (
            "let v: Deque<Float> = Deque.new<Float>()",
            "let v: std::collections::VecDeque<f64> =",
        ),
        (
            "let v: SortedMap<Int, String> = SortedMap.new<Int, String>()",
            "let v: std::collections::BTreeMap<i64, String> =",
        ),
        (
            "let v: SortedSet<Int> = SortedSet.new<Int>()",
            "let v: std::collections::BTreeSet<i64> =",
        ),
    ] {
        let source = format!("fn main() -> Unit {{\n    {decl}\n    return Unit\n}}\n");
        let lowered = lower_source_to_rust("typed-container.rss", &source)
            .unwrap_or_else(|diagnostics| panic!("{decl} should lower: {diagnostics:?}"));
        assert!(
            lowered.contains(expected),
            "`{decl}` should emit `{expected}`, got:\n{lowered}"
        );
    }
}

#[test]
fn typed_let_emits_annotations_for_container_aliases() {
    let source = r#"
type Ints = List<Int>
type Vector<T> = List<T>

fn main() -> Unit {
    let direct: Ints = []
    let generic: Vector<Int> = []
    return Unit
}
"#;
    let lowered =
        lower_source_to_rust("typed-container-alias.rss", source).expect("source should lower");

    assert!(
        lowered.contains("let direct: Vec<i64> = vec![];"),
        "{lowered}"
    );
    assert!(
        lowered.contains("let generic: Vec<i64> = vec![];"),
        "{lowered}"
    );
}

#[test]
fn rust_lowering_classifies_dependency_interface_types() {
    // Review #8: a class declared in a *dependency* interface (not the current
    // source) must be classified so it lowers correctly. Previously it fell through
    // to the unknown-type path and lowered as a positional `Widget(1i64)` call,
    // which can't construct a named-field struct/class. Now it lowers as a managed
    // named-field construction. Runtime-backed stdlib types are excluded from this
    // ingest (covered by the process/config lowering tests staying qualified).
    let interfaces = vec![(
        "dep/widget.rssi".to_string(),
        "class Widget {\n    v: Int\n}\n".to_string(),
    )];
    let sources = vec![(
        "main.rss".to_string(),
        "fn build() -> Widget {\n    return Widget(v: 1)\n}\n\nfn main() -> Unit {\n    let w = build()\n    return Unit\n}\n".to_string(),
    )];
    let package =
        lower_sources_to_rust_package_with_options(&sources, "dep-types", "/rt", &interfaces, &[])
            .expect("dependency-typed source should lower");
    assert!(
        package.lib_rs.contains("Widget { v: 1i64 }"),
        "dependency class should construct a named-field struct, got:\n{}",
        package.lib_rs
    );
    assert!(
        !package.lib_rs.contains("Widget(1i64)"),
        "dependency class must not lower to a positional tuple-struct call:\n{}",
        package.lib_rs
    );
}
