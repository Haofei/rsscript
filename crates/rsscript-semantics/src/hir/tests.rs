use super::*;
use crate::analyze_source;
use crate::diagnostic::code;
use crate::syntax::parse_source;

#[test]
fn collects_type_kinds_and_handle_fields() {
    let source = r#"

class User {
    name: String
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Session {
    user: handle User
    parent: weak User
    file_name: String
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);

    assert_eq!(hir.type_kind("User"), Some(HirTypeKind::Class));
    assert_eq!(hir.type_kind("File"), Some(HirTypeKind::Resource));
    assert_eq!(hir.type_kind("Session"), Some(HirTypeKind::Struct));

    let user_field = hir.fields_named("user").next().expect("user field exists");
    assert_eq!(user_field.ty.to_string(), "User");
    assert!(user_field.is_handle);
    assert!(!user_field.is_weak);
    let parent_field = hir
        .fields_named("parent")
        .next()
        .expect("parent field exists");
    assert_eq!(parent_field.ty.to_string(), "User");
    assert!(!parent_field.is_handle);
    assert!(parent_field.is_weak);
    let session = hir.type_info("Session").expect("session type exists");
    assert!(session.fields["user"].is_handle);
    assert!(session.fields["parent"].is_weak);
    assert!(!session.fields["file_name"].is_handle);
    assert!(hir.is_handle_field_name("user"));
    assert!(hir.is_handle_field_name("parent"));
    assert!(!hir.is_handle_field_name("file_name"));
}

#[test]
fn class_alias_fields_keep_handle_metadata() {
    let source = r#"
class User {
    name: String
}

type UserAlias = User

struct Session {
    user: UserAlias
}
"#;
    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let session = hir.type_info("Session").expect("session type exists");

    assert!(session.fields["user"].is_handle);
    assert_eq!(hir.canonical_type_name("UserAlias"), "User");
}

#[test]
fn normalizes_omitted_function_type_effects() {
    let program = parse_source(
        "test.rss",
        "fn apply(f: Fn(Int) -> Int) -> Int { return f(1) }",
    );
    let hir = Hir::from_syntax(&program);
    let signature = hir
        .resolve_function(None, "apply")
        .expect("apply signature");

    assert_eq!(signature.params[0].ty.to_string(), "Fn(read Int) -> Int");
}

#[test]
fn preserves_declared_field_order_in_type_info_and_constructor_sig() {
    let source = r#"
struct Pair {
    z: Int
    a: String
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let pair = hir.type_info("Pair").expect("pair type exists");

    assert_eq!(
        pair.fields_ordered
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["z", "a"]
    );
    let constructor = hir
        .resolve_function(None, "Pair")
        .expect("constructor exists");
    assert_eq!(
        constructor
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec!["z", "a"]
    );
}

#[test]
fn promotes_class_typed_fields_to_handle_without_keyword() {
    let source = r#"
class User {
    name: String
}

struct Session {
    owner: User
    label: String
    tags: List<String>
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let session = hir.type_info("Session").expect("session type exists");

    // A class-typed field is a handle even without the `handle` keyword.
    assert!(session.fields["owner"].is_handle);
    assert!(!session.fields["owner"].is_weak);
    // Non-class fields stay inline.
    assert!(!session.fields["label"].is_handle);
    assert!(!session.fields["tags"].is_handle);
}

#[test]
fn keeps_builtin_and_user_function_signatures() {
    let source = r#"
struct Store
struct Asset

fn store_put(store: mut Store, value: read Asset) -> Unit
    retains(value)
{
}

"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);

    assert!(hir.resolve_function(Some("String"), "concat").is_some());

    let signature = hir
        .resolve_function(None, "store_put")
        .expect("user signature exists");
    assert!(signature.retained_params.contains("value"));
    assert_eq!(signature.params[0].effect, Some(ParamEffect::Mut));
    assert_eq!(signature.params[1].effect, Some(ParamEffect::Read));
    assert_eq!(
        signature
            .return_ty
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("Unit")
    );

    let concat = hir
        .resolve_function(Some("String"), "concat")
        .expect("builtin signature exists");
    assert_eq!(
        concat
            .return_ty
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("String")
    );
}

#[test]
fn normalizes_omitted_read_effects_for_all_ordinary_parameters() {
    let source = r#"
fn inspect(value: String) -> Unit {
}

fn caller(value: String, count: Int) -> Unit {
    inspect(value: value)
}
"#;

    let program = parse_source("default-read.rss", source);
    let hir = Hir::from_syntax(&program);
    let inspect = hir
        .resolve_function(None, "inspect")
        .expect("inspect signature exists");
    assert_eq!(inspect.params[0].effect, Some(ParamEffect::Read));
    let caller = hir
        .resolve_function(None, "caller")
        .expect("caller signature exists");
    assert_eq!(caller.params[0].effect, Some(ParamEffect::Read));
    assert_eq!(caller.params[1].effect, Some(ParamEffect::Read));

    let body = hir.function_body("caller").expect("caller body exists");
    let Some(HirStmt::Expr(HirExpr::Call { args, .. })) = body
        .block
        .as_ref()
        .and_then(|block| block.statements.first())
    else {
        panic!("caller should contain a call expression");
    };
    assert!(matches!(
        args.first().map(|arg| &arg.value),
        Some(HirExpr::Effect {
            effect: ParamEffect::Read,
            ..
        })
    ));
}

#[test]
fn omitted_read_is_accepted_but_never_upgrades_to_mut_or_take() {
    let source = r#"

fn inspect(value: String) -> Unit {
}

fn rewrite(value: mut String) -> Unit {
}

fn consume(value: take String) -> Unit {
}

fn caller() -> Unit {
    let value = "value"
    inspect(value: value)
    rewrite(value: value)
    consume(value: value)
}
"#;

    let diagnostics = analyze_source("default-read-calls.rss", source);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == code::MISSING_DATA_EFFECT)
            .count(),
        2,
        "bare arguments satisfy read only; mut and take remain explicit"
    );
}

#[test]
fn same_name_read_argument_may_omit_its_label() {
    let source = r#"
fn inspect(value: String) -> Unit {}
fn caller(value: String) -> Unit { inspect(value) }
"#;
    let diagnostics = analyze_source("same-name-argument.rss", source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let hir = Hir::from_syntax(&parse_source("same-name-argument.rss", source));
    let body = hir.function_body("caller").expect("caller body exists");
    let Some(HirStmt::Expr(HirExpr::Call { args, .. })) = body
        .block
        .as_ref()
        .and_then(|block| block.statements.first())
    else {
        panic!("caller should contain a call expression");
    };
    assert_eq!(args[0].name.as_deref(), Some("value"));
}

#[test]
fn receiver_call_default_read_arguments_skip_the_receiver_parameter() {
    let source = r#"
fn String.inspect(self: String, count: Int) -> Unit {}
fn caller(text: String, count: Int) -> Unit { text.inspect(count) }
"#;
    let diagnostics = analyze_source("receiver-default-read.rss", source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let hir = Hir::from_syntax(&parse_source("receiver-default-read.rss", source));
    let body = hir.function_body("caller").expect("caller body exists");
    let Some(HirStmt::Expr(HirExpr::Call { args, .. })) = body
        .block
        .as_ref()
        .and_then(|block| block.statements.first())
    else {
        panic!("caller should contain a receiver call");
    };
    assert_eq!(args[0].name.as_deref(), Some("count"));
    assert!(matches!(
        &args[0].value,
        HirExpr::Effect {
            effect: ParamEffect::Read,
            type_name: Some(type_name),
            ..
        } if type_name == "Int"
    ));
}

#[test]
fn call_arguments_record_parameter_slots_without_losing_evaluation_order() {
    let source = r#"
fn digits(a: Int = 1, b: Int, c: Int = 3) -> Int { return a * 100 + b * 10 + c }
fn caller() -> Int { return digits(c: 9, b: 2) }
"#;
    let hir = Hir::from_syntax(&parse_source("bound-call.rss", source));
    let body = hir.function_body("caller").expect("caller body exists");
    let Some(HirStmt::Return {
        value: Some(HirExpr::Call { args, .. }),
        ..
    }) = body
        .block
        .as_ref()
        .and_then(|block| block.statements.first())
    else {
        panic!("caller should return a call");
    };

    assert_eq!(
        args.iter()
            .map(|arg| (
                arg.name.as_deref(),
                arg.parameter_index,
                arg.evaluation_index
            ))
            .collect::<Vec<_>>(),
        vec![
            (Some("c"), Some(2), 0),
            (Some("b"), Some(1), 1),
            (Some("a"), Some(0), 2),
        ]
    );
}

#[test]
fn qualified_protocol_call_preserves_the_concrete_receiver_type() {
    let source = r#"
protocol Formatter {
    fn format(self: Self) -> fresh String
}
fn render<F: Formatter>(item: F) -> fresh String {
    return Formatter.format(self: item)
}
"#;
    let diagnostics = analyze_source("protocol-default-read.rss", source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let hir = Hir::from_syntax(&parse_source("protocol-default-read.rss", source));
    let body = hir.function_body("render").expect("render body exists");
    let Some(HirStmt::Return {
        value: Some(HirExpr::Call { args, .. }),
        ..
    }) = body
        .block
        .as_ref()
        .and_then(|block| block.statements.first())
    else {
        panic!("render should return a protocol call");
    };
    assert!(matches!(
        &args[0].value,
        HirExpr::Effect {
            effect: ParamEffect::Read,
            type_name: Some(type_name),
            ..
        } if type_name == "F"
    ));
}

#[test]
fn records_duplicate_callable_symbols() {
    let source = r#"

struct Image {
    pixels: Buffer
}

fn Image(path: read Path) -> Image {
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let duplicate = hir
        .duplicate_symbols()
        .first()
        .expect("constructor/function duplicate is recorded");

    assert_eq!(duplicate.kind, DuplicateSymbolKind::Constructor);
    assert_eq!(duplicate.name, "Image");
    assert_eq!(duplicate.first_span.line, 3);
    assert_eq!(duplicate.duplicate_span.line, 7);
}

#[test]
fn records_duplicate_fields() {
    let source = r#"

struct Response {
    status: Int
    status: String
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let duplicate = hir
        .duplicate_symbols()
        .first()
        .expect("duplicate field is recorded");

    assert_eq!(duplicate.kind, DuplicateSymbolKind::Field);
    assert_eq!(duplicate.name, "Response.status");
    assert_eq!(duplicate.first_span.line, 4);
    assert_eq!(duplicate.duplicate_span.line, 5);
}

#[test]
fn resolves_body_call_sites() {
    let source = r#"

struct Rendered {
    status: Int
    body: String
}

fn render(body: read String) -> Result<fresh Rendered, HttpError> {
    let response = Rendered(status: 200, body: read body)
    Output.write(message: read body)
    Missing.call(value: read body)
    return response
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let sites = hir.call_sites();

    assert_eq!(sites.len(), 3);
    assert!(matches!(
        sites[0].resolution,
        CallResolution::Resolved {
            kind: ResolvedCalleeKind::Constructor {
                type_kind: HirTypeKind::Struct
            },
            ..
        }
    ));
    assert!(matches!(
        sites[1].resolution,
        CallResolution::Resolved {
            kind: ResolvedCalleeKind::BuiltinFunction,
            ..
        }
    ));
    assert!(matches!(sites[2].resolution, CallResolution::Unknown));

    let bindings = &hir
        .function_body("render")
        .expect("render body exists")
        .bindings;
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].kind, HirBindingKind::Param);
    assert_eq!(bindings[0].name, "body");
    assert_eq!(
        bindings[0].ty.as_ref().map(ToString::to_string).as_deref(),
        Some("String")
    );
    assert_eq!(bindings[0].type_name.as_deref(), Some("String"));
    assert_eq!(bindings[1].kind, HirBindingKind::ManagedLet);
    assert_eq!(bindings[1].name, "response");
    assert_eq!(
        bindings[1].ty.as_ref().map(ToString::to_string).as_deref(),
        Some("Rendered")
    );
    assert_eq!(bindings[1].type_name.as_deref(), Some("Rendered"));

    let returns = hir.returns();
    assert_eq!(returns.len(), 1);
    assert_eq!(returns[0].function_name, "render");
    assert!(matches!(
        returns[0].proof,
        HirReturnProof::Ident { ref name } if name == "response"
    ));

    let body = hir.function_body("render").expect("function body exists");
    assert_eq!(body.function_name, "render");
    assert_eq!(body.bindings.len(), 2);
    assert_eq!(body.call_sites.len(), 3);
    assert_eq!(body.effect_events.len(), 0);
    assert_eq!(body.returns.len(), 1);
    assert!(matches!(
        body.block
            .as_ref()
            .expect("resolved body block exists")
            .statements
            .first(),
        Some(HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            type_name: Some(type_name),
            ..
        }) if type_name == "Rendered"
    ));
}

#[test]
fn records_local_binding_facts() {
    let source = r#"

struct Asset
struct AssetError

fn Asset.load(path: read Path) -> Result<fresh Asset, AssetError>

fn load(path: read Path) -> Unit {
    local asset = Asset.load(path: read path)?
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let bindings = &hir
        .function_body("load")
        .expect("load body exists")
        .bindings;

    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[1].kind, HirBindingKind::LocalLet);
    assert_eq!(bindings[1].name, "asset");
    assert_eq!(bindings[1].type_name.as_deref(), Some("Asset"));
    assert!(matches!(
        hir.function_body("load")
            .and_then(|body| body.block.as_ref())
            .and_then(|block| block.statements.first()),
        Some(HirStmt::Let {
            kind: HirBindingKind::LocalLet,
            type_name: Some(type_name),
            ..
        }) if type_name == "Asset"
    ));
}

#[test]
fn substitutes_generic_return_types_from_call_arguments() {
    let source = r#"
struct Config {
    name: String
}

struct Holder<T: Struct>

fn Holder.unwrap<T: Struct>(holder: read Holder<T>) -> T

fn run(holder: read Holder<Config>) -> Unit {
    let config = Holder.unwrap(holder: read holder)
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let body = hir.function_body("run").expect("run body exists");

    assert!(matches!(
        body.block
            .as_ref()
            .expect("resolved body block exists")
            .statements
            .first(),
        Some(HirStmt::Let {
            name,
            type_name: Some(type_name),
            value: Some(HirExpr::Call {
                type_name: Some(call_type),
                ..
            }),
            ..
        }) if name == "config" && type_name == "Config" && call_type == "Config"
    ));
}

#[test]
fn records_field_access_facts() {
    let source = r#"

class Rules {
}

struct Config {
    rules: handle Rules
}

fn take_rules(config: mut Config) -> Unit {
    List.consume(list: take config.rules)
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let field = hir
        .field_accesses()
        .first()
        .expect("field access is recorded");

    assert_eq!(field.function_name, "take_rules");
    assert_eq!(field.name, "rules");
    assert_eq!(
        field.base_ty.as_ref().map(ToString::to_string).as_deref(),
        Some("Config")
    );
    assert_eq!(
        field.ty.as_ref().map(ToString::to_string).as_deref(),
        Some("Rules")
    );
    assert_eq!(field.base_type.as_deref(), Some("Config"));
    assert_eq!(field.type_name.as_deref(), Some("Rules"));
    assert!(field.is_handle);
    assert!(
        hir.function_body("take_rules")
            .expect("body exists")
            .field_accesses
            .iter()
            .any(|access| access.name == "rules" && access.is_handle)
    );
}

#[test]
fn records_effect_events() {
    let source = r#"

struct Image

fn Image.load(path: read Path) -> fresh Image

class RetainedImageStore {
}

fn RetainedImageStore.store(cache: mut RetainedImageStore, image: read Image) -> Unit
    retains(image)

fn publish(cache: mut RetainedImageStore, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    RetainedImageStore.store(cache: mut cache, image: read shared)
    Buffer.consume(buffer: take image)
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);

    assert_eq!(hir.effect_events().len(), 3);
    assert!(matches!(
        hir.effect_events()[0].kind,
        HirEffectEventKind::Manage
    ));
    assert_eq!(hir.effect_events()[0].binding_name, "image");
    assert!(matches!(
        hir.effect_events()[1].kind,
        HirEffectEventKind::Retain { .. }
    ));
    assert_eq!(hir.effect_events()[1].binding_name, "shared");
    assert!(matches!(
        hir.effect_events()[2].kind,
        HirEffectEventKind::Take
    ));
    assert_eq!(hir.effect_events()[2].binding_name, "image");
    assert_eq!(
        hir.function_body("publish")
            .expect("publish body exists")
            .effect_events
            .len(),
        3
    );
}

#[test]
fn lowers_resolved_statement_expression_tree_for_function_body() {
    let source = r#"

class Rules {
}

struct Config {
    rules: handle Rules
}

class RetainedImageStore {
}

struct Asset
struct AssetError

fn Asset.load(path: read Path) -> Result<fresh Asset, AssetError>

fn RetainedImageStore.store(cache: mut RetainedImageStore, asset: read Asset) -> Unit
    retains(asset)

fn update(cache: mut RetainedImageStore, config: mut Config, path: read Path) -> Unit {
    local asset = Asset.load(path: read path)?
    RetainedImageStore.store(cache: mut cache, asset: read asset)
    List.consume(list: take config.rules)
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let body = hir.function_body("update").expect("body exists");
    let block = body.block.as_ref().expect("resolved HIR block exists");

    assert_eq!(block.statements.len(), 3);
    let HirStmt::Let {
        kind: HirBindingKind::LocalLet,
        name,
        value: Some(HirExpr::Try { type_name, .. }),
        type_name: Some(binding_type),
        ..
    } = &block.statements[0]
    else {
        panic!("first statement should be a typed local call binding");
    };
    assert_eq!(name, "asset");
    assert_eq!(type_name.as_deref(), Some("Asset"));
    assert_eq!(binding_type, "Asset");

    let HirStmt::Expr(HirExpr::Call {
        resolution, events, ..
    }) = &block.statements[1]
    else {
        panic!("second statement should be a resolved retaining call");
    };
    assert!(matches!(
        resolution,
        CallResolution::Resolved {
            kind: ResolvedCalleeKind::UserFunction,
            ..
        }
    ));
    assert!(matches!(events[0].kind, HirEffectEventKind::Retain { .. }));
    assert_eq!(events[0].binding_name, "asset");

    let HirStmt::Expr(HirExpr::Call { args, .. }) = &block.statements[2] else {
        panic!("third statement should be a call");
    };
    let HirExpr::Effect {
        effect: ParamEffect::Take,
        value,
        events,
        ..
    } = &args[0].value
    else {
        panic!("call argument should be a take expression");
    };
    assert!(matches!(events[0].kind, HirEffectEventKind::Take));
    assert_eq!(events[0].binding_name, "config.rules");
    let HirExpr::Field { access, .. } = value.as_ref() else {
        panic!("take value should be a field access");
    };
    assert_eq!(access.base_type.as_deref(), Some("Config"));
    assert_eq!(access.type_name.as_deref(), Some("Rules"));
    assert!(access.is_handle);
}

fn binding_type(source: &str, binding: &str) -> Option<String> {
    let program = parse_source("binary-inference.rss", source);
    let hir = Hir::from_syntax(&program);
    hir.function_body("check")
        .expect("check body")
        .bindings
        .iter()
        .find(|candidate| candidate.name == binding)
        .expect("binding exists")
        .ty
        .as_ref()
        .map(ToString::to_string)
}

#[test]
fn infers_the_operand_type_of_an_arithmetic_expression() {
    let source =
        "fn check(a: Int, b: Int) -> Unit {\n    let sum = a + b\n    let shifted = a << b\n}\n";
    assert_eq!(binding_type(source, "sum").as_deref(), Some("Int"));
    assert_eq!(binding_type(source, "shifted").as_deref(), Some("Int"));

    let floats = "fn check(a: Float, b: Float) -> Unit {\n    let scaled = a * b\n}\n";
    assert_eq!(binding_type(floats, "scaled").as_deref(), Some("Float"));
}

#[test]
fn infers_bool_for_comparison_and_logical_expressions() {
    let source = "fn check(a: Int, b: Int, flag: Bool) -> Unit {\n    let ordered = a < b\n    let same = a == b\n    let both = flag && ordered\n}\n";
    assert_eq!(binding_type(source, "ordered").as_deref(), Some("Bool"));
    assert_eq!(binding_type(source, "same").as_deref(), Some("Bool"));
    assert_eq!(binding_type(source, "both").as_deref(), Some("Bool"));
}

#[test]
fn leaves_a_non_numeric_or_mismatched_arithmetic_expression_untyped() {
    // `operators.rs` reports these as RS1001/RS0210; inference must not invent a
    // type and add a second, derived error on top.
    let text = "fn check(a: String, b: String) -> Unit {\n    let joined = a + b\n}\n";
    assert_eq!(binding_type(text, "joined"), None);

    let mixed = "fn check(a: Int, b: Float) -> Unit {\n    let mixed = a + b\n}\n";
    assert_eq!(binding_type(mixed, "mixed"), None);

    let unknown = "fn check(b: Int) -> Unit {\n    let total = missing + b\n}\n";
    assert_eq!(binding_type(unknown, "total"), None);
}

#[test]
fn an_inferred_binary_type_reaches_downstream_argument_checks() {
    let source = r#"
fn takes_string(value: String) -> Unit {
    return Unit
}

fn check(a: Int, b: Int) -> Unit {
    let sum = a + b
    takes_string(value: sum)
    return Unit
}
"#;
    let diagnostics = analyze_source("binary-argument.rss", source);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == code::ARGUMENT_TYPE_MISMATCH)
            .count(),
        1,
        "{diagnostics:?}"
    );
}

/// The three `RS0207` shapes whose repair is mechanical, checked end to end
/// through the analyzer rather than through the diagnostic constructor.
///
/// Measured on 2026-09-19: `RS0207` is the one class haiku's repair loop
/// introduces more often than it clears (five against one), and it was the
/// only large class whose fix named no replacement.
#[test]
fn argument_type_mismatches_carry_the_edit_the_checker_can_derive() {
    let source = r#"
fn use_int(value: Int) -> Int {
    return value
}

fn scale(ratio: Float) -> Float {
    return ratio
}

fn propagates(text: String) -> Option<Int> {
    let parsed = String.parse_int(value: text)
    return Some(use_int(value: parsed))
}

fn widens() -> Float {
    return scale(ratio: 7)
}

fn parses() -> Int {
    return use_int(value: "12")
}
"#;
    let diagnostics = analyze_source("argument-repair.rss", source);
    let mismatches = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == code::ARGUMENT_TYPE_MISMATCH)
        .collect::<Vec<_>>();
    assert_eq!(mismatches.len(), 3, "{diagnostics:#?}");

    let fix = |kind: &str| {
        mismatches
            .iter()
            .flat_map(|diagnostic| diagnostic.fixes.iter())
            .find(|fix| fix.kind == kind)
            .unwrap_or_else(|| panic!("no `{kind}` fix in {mismatches:#?}"))
    };

    // An `Option<Int>` where an `Int` is wanted, in a function returning
    // `Option<Int>`: `?` is the whole edit, inserted after the identifier.
    let propagate = fix("propagate_with_try");
    assert_eq!(propagate.applicability, "machine-applicable");
    let edit = propagate.edit.as_ref().expect("an edit");
    assert_eq!(edit.replacement, "?");
    assert_eq!(edit.span.length, 0);

    // An `Int` literal where a `Float` is wanted.
    let widen = fix("widen_int_literal_to_float");
    assert_eq!(widen.applicability, "machine-applicable");
    assert_eq!(
        widen.edit.as_ref().map(|edit| edit.replacement.as_str()),
        Some("7.0")
    );

    // A `String` literal where an `Int` is wanted stays advice: the literal's
    // text may not be a number, so no edit is safe to apply unseen.
    let parse = fix("parse_string_literal");
    assert_eq!(parse.applicability, "manual");
    assert!(parse.edit.is_none());

    // Every mismatch still carries the advisory fix it always had.
    for mismatch in &mismatches {
        assert!(
            mismatch
                .fixes
                .iter()
                .any(|fix| fix.kind == "match_argument_type"),
            "{mismatch:#?}"
        );
    }
}

/// `?` is offered only where the try checker would accept it. A function that
/// returns neither `Result` nor `Option` cannot propagate a failure, and a
/// `Result` whose error type differs from the enclosing function's would trade
/// `RS0207` for `RS0013`.
#[test]
fn a_try_repair_is_withheld_where_the_function_cannot_propagate() {
    let source = r#"
fn use_int(value: Int) -> Int {
    return value
}

fn no_target(text: String) -> Int {
    let parsed = String.parse_int(value: text)
    return use_int(value: parsed)
}
"#;
    let diagnostics = analyze_source("argument-repair-withheld.rss", source);
    let mismatch = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == code::ARGUMENT_TYPE_MISMATCH)
        .unwrap_or_else(|| panic!("{diagnostics:#?}"));
    assert!(
        mismatch
            .fixes
            .iter()
            .all(|fix| fix.kind != "propagate_with_try"),
        "{mismatch:#?}"
    );
    assert!(
        mismatch
            .fixes
            .iter()
            .any(|fix| fix.kind == "match_argument_type"),
        "{mismatch:#?}"
    );
}

#[test]
fn classifies_fresh_return_facts() {
    let source = r#"

struct Response {
    status: Int
}

fn make_response() -> fresh Response {
    return Response(status: 200)
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let return_fact = hir.returns().first().expect("return fact exists");

    assert_eq!(return_fact.function_name, "make_response");
    assert!(matches!(
        return_fact.proof,
        HirReturnProof::StructConstructor
    ));
}

/// A bare identifier arm resolves declared case first, then binding, and an
/// unknown capitalized name stays a case pattern so a typo is diagnosed.
#[test]
fn a_bare_arm_name_resolves_declared_case_first_then_binding() {
    let source = r#"
sum Direction {
    North
    South
}

sum Color {
    red
    green
}

sum Shape {
    Circle(radius: Int)
}
"#;
    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let span = rsscript_syntax::Span {
        file: "test.rss".to_string(),
        line: 1,
        column: 1,
        length: 1,
    };
    let bare = |name: &str| MatchPattern::Variant {
        name: name.to_string(),
        bindings: Vec::new(),
        span: span.clone(),
    };
    let resolves_to_binding = |name: &str| {
        matches!(
            hir.resolve_bare_arm_pattern(&bare(name)),
            MatchPattern::Binding { name: bound, .. } if bound == name
        )
    };

    for binding in ["other", "value", "_rest"] {
        assert!(resolves_to_binding(binding), "`{binding}` binds");
    }
    // Declared cases stay cases, including a lowercase one, a payload-carrying
    // one written bare, and the builtin cases.
    for case in [
        "North", "South", "red", "green", "Circle", "None", "Some", "Ok", "Err",
    ] {
        assert_eq!(hir.resolve_bare_arm_pattern(&bare(case)), bare(case));
    }
    // An unknown capitalized name and a qualified name are never bindings.
    for case in ["Nroth", "MAX", "ops.ADD"] {
        assert_eq!(hir.resolve_bare_arm_pattern(&bare(case)), bare(case));
    }
    // A case pattern with a payload is left alone.
    let with_payload = MatchPattern::Variant {
        name: "other".to_string(),
        bindings: vec![MatchPattern::Wildcard(span.clone())],
        span: span.clone(),
    };
    assert_eq!(hir.resolve_bare_arm_pattern(&with_payload), with_payload);
}

/// A non-`String` interpolated value is reported at the value itself, as an
/// interpolation mismatch — not as a `List<String>` literal item at 1:1, which
/// is what the desugared `String.format(args: [...])` made it look like. An
/// `Int`, `Float`, or `Bool` value carries the edit that wraps it in its core
/// conversion; any other type gets advice only.
#[test]
fn a_non_string_interpolated_value_is_reported_where_it_is_with_its_conversion() {
    let source = "fn show(n: Int, ratio: Float, done: Bool, name: String, xs: List<Int>) -> fresh String {\n    return $\"{name}: {n} {  ratio  } {done} {xs}\"\n}\n";
    let diagnostics = analyze_source("interpolation.rss", source);
    let mismatches = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == code::ARGUMENT_TYPE_MISMATCH)
        .collect::<Vec<_>>();
    assert_eq!(mismatches.len(), 4, "{diagnostics:#?}");

    let expected = [
        ("Int", 23, 1, Some("String.from_int(value: n)")),
        ("Float", 29, 5, Some("String.from_float(value: ratio)")),
        ("Bool", 39, 4, Some("String.from_bool(value: done)")),
        ("List<Int>", 46, 2, None),
    ];
    for (diagnostic, (actual, column, length, replacement)) in mismatches.iter().zip(expected) {
        assert_eq!(
            diagnostic.summary,
            format!("interpolated value has type `{actual}`, but must be a `String`.")
        );
        assert_eq!(
            (
                diagnostic.span.line,
                diagnostic.span.column,
                diagnostic.span.length
            ),
            (2, column, length),
            "{diagnostic:#?}"
        );
        let fix = diagnostic
            .fixes
            .iter()
            .find(|fix| fix.kind == "convert_interpolated_value")
            .expect("a conversion fix");
        match replacement {
            Some(replacement) => {
                assert_eq!(fix.applicability, "machine-applicable");
                let edit = fix
                    .edit
                    .as_ref()
                    .expect("a machine-applicable fix has an edit");
                assert_eq!(edit.replacement, replacement);
                assert_eq!(
                    (edit.span.line, edit.span.column, edit.span.length),
                    (2, column, length)
                );
            }
            None => {
                assert_eq!(fix.applicability, "manual");
                assert!(fix.edit.is_none());
            }
        }
    }
}
