//! Spec §5A — statement & control-flow lowering
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn rust_lowering_allows_positional_args_for_private_helpers() {
    let source = r#"
fn join_pair(left: read String, right: read String) -> String {
    return left.concat(right)
}

pub fn main() -> String {
    return join_pair("a", "b")
}
"#;
    let rust = lower_source_to_rust("private-positional.rss", source).expect("source should lower");

    assert!(
        rust.contains("join_pair(&(\"a\".to_string()), &(\"b\".to_string()))"),
        "private positional read arguments should lower as borrows, got:\n{rust}"
    );
}

#[test]
fn checker_rejects_positional_args_for_public_functions() {
    let source = r#"
pub fn join_pair(left: read String, right: read String) -> String {
    return left.concat(right)
}

pub fn main() -> String {
    return join_pair("a", "b")
}
"#;
    let diagnostics = analyze_source("public-positional.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0201"),
        "{diagnostics:#?}"
    );
}

#[test]
fn checker_rejects_constructor_shorthand_without_matching_field() {
    let source = r#"
struct User {
    id: String
}

fn main() -> fresh User {
    let value = "rss"
    return User(value)
}
"#;
    let diagnostics = analyze_source("constructor-field-shorthand-mismatch.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0201"),
        "{diagnostics:#?}"
    );
}

#[test]
fn rust_package_adds_serde_for_json_derives() {
    let source = r#"
struct User derives(JsonEncode, JsonDecode) {
    id: Int
}
"#;
    let package = lower_source_to_rust_package(
        "derive-json.rss",
        source,
        "derive-json-package",
        "../runtime",
    )
    .expect("source should lower");

    assert!(
        package
            .cargo_toml
            .contains("serde = { version = \"1\", features = [\"derive\"] }")
    );
}

#[test]
fn rust_package_release_profile_pins_overflow_checks() {
    // §6.8: integer overflow traps in every build profile. The compiled tier relies
    // on the generated release profile keeping `overflow-checks = true`; if this is
    // ever dropped, release builds would silently wrap and diverge from the reg-VM
    // (which traps via checked arithmetic). Guard the manifest so that can't regress.
    let source = r#"
fn main() -> Unit {
    return Unit
}
"#;
    let package = lower_source_to_rust_package(
        "overflow-profile.rss",
        source,
        "overflow-profile-package",
        "../runtime",
    )
    .expect("source should lower");

    assert!(
        package.cargo_toml.contains("[profile.release]")
            && package.cargo_toml.contains("overflow-checks = true"),
        "generated release profile must pin overflow-checks = true, got:\n{}",
        package.cargo_toml
    );
}

#[test]
fn rust_lowering_supports_while_is_pattern() {
    let source = r#"
fn main(value: read Option<Int>) -> Int {
    let mut total = 0
    while read value is Some(item) {
        total = total + item
        break
    }
    return total
}
"#;
    let rust = lower_source_to_rust("while-is.rss", source).expect("source should lower");

    assert!(rust.contains("loop {"));
    assert!(rust.contains("match "));
    assert!(rust.contains("Some(item)"));
    assert!(rust.contains("_ => {"));
    assert!(rust.contains("break;"));
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
    let source = common::read_fixture(Path::new("tests/golden/lowering/simple.rss"));
    let expected_rust = common::read_fixture(Path::new("tests/golden/lowering/simple.rs"));
    let expected_source_map =
        common::read_fixture(Path::new("tests/golden/lowering/simple.source-map.txt"));
    let lowered =
        lower_source_to_rust_with_map("simple.rss", &source).expect("source should lower");

    assert_eq!(lowered.rust_source, expected_rust);
    assert_eq!(
        common::source_map_summary(&lowered.source_map),
        expected_source_map
    );
}

#[test]
fn rust_lowering_source_map_covers_match_patterns_closures_and_binary_exprs() {
    let source = r#"
fn maybe() -> Option<Int> {
    return Some(1)
}

fn inspect() -> Int {
    let values = List<Int>.new()
    List.push(list: mut values, value: read 1)
    let count = List.count_where(
        list: read values,
        predicate: |item| {
            return item + 1 == 2
        },
    )
    let value = maybe()
    return match value {
        Some(result) => { read result + count }
        None => { 0 }
    }
}
"#;
    let lowered =
        lower_source_to_rust_with_map("source-map.rss", source).expect("source should lower");

    assert!(
        lowered.source_map.iter().any(|entry| {
            entry.kind == "match_pattern" && entry.source.file == "source-map.rss"
        })
    );
    assert!(
        lowered
            .source_map
            .iter()
            .any(|entry| { entry.kind == "closure" && entry.source.file == "source-map.rss" })
    );
    assert!(
        lowered
            .source_map
            .iter()
            .any(|entry| { entry.kind == "binary" && entry.source.file == "source-map.rss" })
    );
}

#[test]
fn rust_lowering_uses_trait_qualified_ufcs_for_receiver_protocol_calls() {
    let source = r#"
features: local

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn write_line<W: Writer>(writer: mut W, message: read String) -> Unit {
    mut writer.write(message: read message)
}
"#;
    let rust =
        lower_source_to_rust("receiver-protocol-lower.rss", source).expect("source should lower");

    assert!(rust.contains("<W as Writer>::write(writer, message);"));
    assert!(!rust.contains("Writer::write(&mut writer, &message);"));
}

#[test]
fn rust_lowering_uses_trait_qualified_ufcs_for_concrete_receiver_protocol_impl_calls() {
    let source = r#"
features: local

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriterImpl.write(self: mut BufferWriter, message: read String) -> Unit {
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriterImpl.write
}

fn write_line(writer: mut BufferWriter, message: read String) -> Unit {
    mut writer.write(message: read message)
}
"#;
    let rust = lower_source_to_rust("receiver-concrete-protocol-lower.rss", source)
        .expect("source should lower");

    assert!(rust.contains("<BufferWriter as Writer>::write(writer, message);"));
    assert!(!rust.contains("BufferWriter::write(&mut writer, &message);"));
}

#[test]
fn rust_lowering_generates_sealed_capability_object_for_protocol_impls() {
    let source = r#"
features: local

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn write_dynamic(writer: take BufferWriter, message: read String) -> Unit {
    local cap: Capability<Writer> = Capability<Writer>.from(value: take writer)
    Writer.write(self: mut cap, message: read message)
}
"#;
    let rust =
        lower_source_to_rust("capability-object-lower.rss", source).expect("source should lower");

    assert!(rust.contains("pub enum CapabilityWriter"));
    assert!(rust.contains("BufferWriter(BufferWriter)"));
    assert!(rust.contains("impl Writer for CapabilityWriter"));
    assert!(rust.contains("CapabilityWriter::BufferWriter(writer)"));
    assert!(rust.contains("Writer::write(&mut cap, message);"));
    assert!(!rust.contains("dyn Writer"));
}

#[test]
fn rust_lowering_allows_receiver_calls_for_core_namespace_functions() {
    let source = r#"
features: local

fn main() -> Int {
    let mut items = List<Int>.new()
    mut items.push(value: read 1)
    mut items.push(value: read 2)
    let count = items.len()
    let first = items.get(0)
    return count
}
"#;
    let rust =
        lower_source_to_rust("core-receiver-call-lower.rss", source).expect("source should lower");

    assert!(rust.contains("rsscript_runtime::list_push"));
    assert!(rust.contains("&mut items"));
    assert!(rust.contains("&(1i64)"));
    assert!(rust.contains("&(2i64)"));
    assert!(rust.contains("let count = rsscript_runtime::list_len(&items);"));
    assert!(rust.contains("let first = rsscript_runtime::list_get(&items, 0i64);"));
    assert!(rust.contains("return count;"));
}

#[test]
fn rust_lowering_supports_structured_match_patterns_and_guards() {
    let source = r#"
sum Expr {
    Call(callee: String, arg_count: Int)
    Literal(value: String)
}

fn describe(expr: read Expr) -> String {
    return match read expr {
        Call { callee, arg_count } if arg_count == 0 => {
            read callee
        }
        Call { callee, .. } => {
            read callee
        }
        Literal { value } => {
            read value
        }
    }
}
"#;
    let rust = lower_source_to_rust("structured-match.rss", source)
        .expect("structured match should lower");

    assert!(rust.contains("match expr"));
    assert!(rust.contains("Expr::Call { callee, arg_count } if arg_count == 0"));
    assert!(rust.contains("Expr::Call { callee, .. }"));
    assert!(rust.contains("Expr::Literal { value }"));
}

#[test]
fn rust_lowering_supports_nested_match_patterns() {
    let source = r#"
sum Callee {
    Name(value: String)
    Builtin
}

sum Expr {
    Call(callee: Callee)
    Literal(value: String)
}

fn describe(expr: read Expr) -> String {
    return match read expr {
        Call { callee: Name(value) } => {
            read value
        }
        Call { callee: Builtin } => {
            "builtin"
        }
        Literal { value } => {
            read value
        }
    }
}
"#;
    let rust = lower_source_to_rust("nested-match.rss", source).expect("nested match should lower");

    assert!(rust.contains("Expr::Call { callee: Callee::Name { value: value } }"));
    assert!(rust.contains("Callee::Builtin"));
    assert!(rust.contains("Expr::Literal { value }"));
}

#[test]
fn rust_lowering_supports_struct_match_patterns() {
    let source = r#"
struct User {
    name: String
}

fn name_of(user: read User) -> String {
    return match read user {
        User { name } => {
            read name
        }
    }
}
"#;
    let rust = lower_source_to_rust("struct-match.rss", source).expect("struct match should lower");

    assert!(rust.contains("match user"));
    assert!(rust.contains("User { name }"));
}

#[test]
fn rust_lowering_supports_local_take_match_patterns() {
    let source = r#"
features: local

struct User {
    name: String
}

fn main() -> String {
    local user = User(name: "rss")
    return match take user {
        User { name } => {
            read name
        }
    }
}
"#;
    let rust = lower_source_to_rust("take-match.rss", source).expect("take match should lower");

    assert!(rust.contains("match user"));
    assert!(rust.contains("User { name }"));
}

#[test]
fn rust_lowering_allows_receiver_calls_for_generic_map_methods() {
    let source = r#"
fn main() -> Int {
    let mut names = Map<Int, String>.new()
    mut names.insert(key: read 1, value: read "one")
    let found = read names.get(key: read 1)
    let removed = mut names.remove(key: read 1)

    let mut payloads = Map<Int, JsonValue>.new()
    mut payloads.insert(key: read 7, value: read {"ok": true})
    return Map.len(map: read names)
}
"#;
    let rust =
        lower_source_to_rust("map-receiver-call-lower.rss", source).expect("source should lower");

    assert!(
        rust.contains("rsscript_runtime::map_insert(&mut names, &(1i64), &(\"one\".to_string()));")
    );
    assert!(rust.contains("let found = rsscript_runtime::map_get(&names, &(1i64));"));
    assert!(rust.contains("let removed = rsscript_runtime::map_remove(&mut names, &(1i64));"));
    assert!(
        rust.contains(
            "rsscript_runtime::map_insert(&mut payloads, &(7i64), &(rsscript_runtime::json_object"
        ),
        "{rust}"
    );
}

#[test]
fn checker_rejects_map_literal_entry_type_mismatches() {
    let source = r#"
fn takes_names(names: read Map<Int, String>) -> Unit {
    return Unit
}

fn main() -> Unit {
    let wrong_key: Map<Int, String> = { "one" => "one" }
    let wrong_value: Map<Int, String> = { 1 => 2 }
    takes_names(names: read { "two" => 2 })
    return Unit
}
"#;
    let diagnostics = analyze_source_with_core("map-literal-type-mismatch.rss", source);
    let map_entry_mismatches = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.label == "map literal entry type mismatch")
        .count();

    assert_eq!(map_entry_mismatches, 4, "{diagnostics:?}");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("map literal key has type `String`, expected `Int`")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("map literal value has type `Int`, expected `String`")
    }));
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
fn checker_reports_plain_fn_callback_call_argument_type_mismatch() {
    let source = r#"
struct BuildError {
    message: String
}

fn run(callback: read Fn(Int) -> Result<String, BuildError>) -> Result<String, BuildError> {
    return callback("bad")
}
"#;
    let diagnostics = analyze_source("plain-fn-callback-arg.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0207"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_reports_protocol_impl_signature_mismatch() {
    let source = r#"
protocol Writer {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn BufferWriter.write(
    self: read BufferWriter,
    message: read String,
) -> Unit
    effects(retains(message))
{
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}
"#;
    let diagnostics = analyze_source_with_core("protocol-impl-mismatch.rss", source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1301"
            && diagnostic.label == "protocol implementation signature mismatch"
    }));
}

#[test]
fn deep_logical_chain_lowers_to_balanced_shallow_tree() {
    // A long `&&` chain must lower to a *balanced* Rust expression (O(log n) paren
    // nesting) rather than a left-linear nest, so rustc doesn't overflow compiling
    // the generated crate (replacing the `RUST_MIN_STACK=2g` workaround). `&&`/`||`
    // are associative for value AND short-circuit order, so regrouping is sound.
    // 40 operands: enough to trigger balanced lowering (threshold 8) and to
    // distinguish balanced (~log2(40) ≈ 6 deep) from left-linear (~40 deep). Kept
    // modest so the recursive parse/HIR passes don't overflow the small cargo-test
    // thread stack (the `rss` binary handles far deeper on its main-thread stack).
    let conds = (0..40)
        .map(|i| format!("x > {i}"))
        .collect::<Vec<_>>()
        .join(" && ");
    let source = format!("fn deep(x: Int) -> Bool {{\n    return {conds}\n}}\n");
    let rust =
        lower_source_to_rust("deep-bool-chain.rss", &source).expect("deep && chain should lower");

    // Left-linear lowering of 40 operands would produce ~40 consecutive `(`;
    // a balanced tree's deepest path is far shallower.
    assert!(
        !rust.contains(&"(".repeat(20)),
        "deep `&&` chain must lower to a balanced (shallow) tree, not a left-linear nest"
    );
    // Generated code over-parenthesizes for structural correctness; the crate
    // header allows the resulting (benign) unused_parens lint.
    assert!(
        rust.contains("unused_parens"),
        "generated crate should allow(unused_parens) for the balanced regrouping"
    );
}
