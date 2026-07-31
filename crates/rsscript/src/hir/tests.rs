use super::*;
use crate::analyze_source;
use crate::diagnostic::code;
use crate::syntax::parse_source;

#[test]
fn collects_type_kinds_and_handle_fields() {
    let source = r#"
features: local

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
    assert_eq!(user_field.type_name, "User");
    assert!(user_field.is_handle);
    assert!(!user_field.is_weak);
    let parent_field = hir
        .fields_named("parent")
        .next()
        .expect("parent field exists");
    assert_eq!(parent_field.type_name, "User");
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

    assert_eq!(signature.params[0].type_name, "Fn(read Int) -> Int");
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

fn cache_put(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))
{
}

"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);

    assert!(hir.resolve_function(Some("Image"), "resize").is_some());

    let signature = hir
        .resolve_function(None, "cache_put")
        .expect("user signature exists");
    assert!(signature.retained_params.contains("value"));
    assert_eq!(signature.params[0].effect, Some(ParamEffect::Mut));
    assert_eq!(signature.params[1].effect, Some(ParamEffect::Read));
    assert_eq!(signature.return_type.as_deref(), Some("Unit"));

    let load = hir
        .resolve_function(Some("Image"), "load")
        .expect("builtin signature exists");
    assert_eq!(
        load.return_type.as_deref(),
        Some("Result<fresh Image, ImageError>")
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
features: local

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
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code::MISSING_PARAMETER_EFFECT),
        "omitted ordinary parameter effects default to read"
    );
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

struct Response {
    status: Int
    body: String
}

fn render(body: read String) -> Result<fresh Response, HttpError> {
    let response = Response(status: 200, body: read body)
    Log.write(message: read body)
    Missing.call(value: read body)
    return response
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);
    let sites = &hir.call_sites;

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
    assert_eq!(bindings[0].type_name.as_deref(), Some("String"));
    assert_eq!(bindings[1].kind, HirBindingKind::ManagedLet);
    assert_eq!(bindings[1].name, "response");
    assert_eq!(bindings[1].type_name.as_deref(), Some("Response"));

    let returns = &hir.returns;
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
        }) if type_name == "Response"
    ));
}

#[test]
fn records_local_binding_facts() {
    let source = r#"
features: local

fn load(path: read Path) -> Unit {
    local image = Image.load(path: read path)?
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
    assert_eq!(bindings[1].name, "image");
    assert_eq!(bindings[1].type_name.as_deref(), Some("Image"));
    assert!(matches!(
        hir.function_body("load")
            .and_then(|body| body.block.as_ref())
            .and_then(|block| block.statements.first()),
        Some(HirStmt::Let {
            kind: HirBindingKind::LocalLet,
            type_name: Some(type_name),
            ..
        }) if type_name == "Image"
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
features: local

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
        .field_accesses
        .first()
        .expect("field access is recorded");

    assert_eq!(field.function_name, "take_rules");
    assert_eq!(field.name, "rules");
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
features: local

class RetainedImageStore {
}

fn RetainedImageStore.store(cache: mut RetainedImageStore, image: read Image) -> Unit
    effects(retains(image))

fn publish(cache: mut RetainedImageStore, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    RetainedImageStore.store(cache: mut cache, image: read shared)
    Buffer.consume(buffer: take image)
}
"#;

    let program = parse_source("test.rss", source);
    let hir = Hir::from_syntax(&program);

    assert_eq!(hir.effect_events.len(), 3);
    assert!(matches!(
        hir.effect_events[0].kind,
        HirEffectEventKind::Manage
    ));
    assert_eq!(hir.effect_events[0].binding_name, "image");
    assert!(matches!(
        hir.effect_events[1].kind,
        HirEffectEventKind::Retain { .. }
    ));
    assert_eq!(hir.effect_events[1].binding_name, "shared");
    assert!(matches!(
        hir.effect_events[2].kind,
        HirEffectEventKind::Take
    ));
    assert_eq!(hir.effect_events[2].binding_name, "image");
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
features: local

class Rules {
}

struct Config {
    rules: handle Rules
}

class RetainedImageStore {
}

fn RetainedImageStore.store(cache: mut RetainedImageStore, image: read Image) -> Unit
    effects(retains(image))

fn update(cache: mut RetainedImageStore, config: mut Config, path: read Path) -> Unit {
    local image = Image.load(path: read path)?
    RetainedImageStore.store(cache: mut cache, image: read image)
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
    assert_eq!(name, "image");
    assert_eq!(type_name.as_deref(), Some("Image"));
    assert_eq!(binding_type, "Image");

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
    assert_eq!(events[0].binding_name, "image");

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
    let return_fact = hir.returns.first().expect("return fact exists");

    assert_eq!(return_fact.function_name, "make_response");
    assert!(matches!(
        return_fact.proof,
        HirReturnProof::StructConstructor
    ));
}
