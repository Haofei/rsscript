//! Spec §10 — data effects, retention, managed closure capture
#![allow(unused_imports, dead_code)]
use super::*;

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
fn checker_accepts_inline_manage_of_fresh_rvalue() {
    // A struct constructor and a `fresh`-returning call are freshly produced,
    // owned rvalues: inline `manage` of them is sound and must be accepted.
    let source = r#"
features: local

struct Frame {
    pixels: Int
}

fn make_frame() -> fresh Frame

fn ok() -> Unit {
    let shared = manage Frame(pixels: 0)
    let from_call = manage make_frame()
    return Unit
}
"#;
    let diagnostics = analyze_source("inline-manage-fresh.rss", source);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0307"),
        "inline manage of a fresh rvalue must not be rejected: {diagnostics:?}"
    );
}

#[test]
fn checker_rejects_inline_manage_of_unsound_rvalue() {
    // Non-fresh rvalues may alias live state and must still be rejected with
    // RS0307: a class constructor (managed identity, not fresh) and a plain
    // (non-fresh) function call result.
    let class_diags = analyze_source(
        "inline-manage-class.rss",
        r#"
features: local

class Session {
    id: Int
}

fn bad_class() -> Unit {
    let s = manage Session(id: 1)
    return Unit
}
"#,
    );
    assert!(
        class_diags.iter().any(|d| d.code == "RS0307"),
        "inline manage of a class constructor must be rejected: {class_diags:?}"
    );

    let plain_diags = analyze_source(
        "inline-manage-plain.rss",
        r#"
features: local

struct Frame {
    pixels: Int
}

fn plain_frame() -> Frame

fn bad_plain_call() -> Unit {
    let v = manage plain_frame()
    return Unit
}
"#,
    );
    assert!(
        plain_diags.iter().any(|d| d.code == "RS0307"),
        "inline manage of a non-fresh call result must be rejected: {plain_diags:?}"
    );
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
fn checker_requires_effect_for_structured_match_patterns() {
    let source = r#"
sum Expr {
    Call(callee: String)
}

fn main(expr: read Expr) -> Unit {
    match expr {
        Call { callee } => {
            return Unit
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-structured-effect.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0202"
                && diagnostic
                    .summary
                    .contains("structured match patterns require an explicit scrutinee effect")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_mutating_match_guards() {
    let source = r#"
sum Expr {
    Call(callee: String)
}

fn main(expr: read Expr) -> Unit {
    match read expr {
        Call { callee } if mut callee => {
            return Unit
        }
        Call { callee } => {
            return Unit
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-guard-mut.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0310"
                && diagnostic.summary.contains("match guard cannot use `mut`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_requires_local_for_take_match() {
    let source = r#"
sum Expr {
    Call(callee: String)
}

fn main(expr: read Expr) -> Unit {
    match take expr {
        Call { callee } => {
            return Unit
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-take-local.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0101"
                && diagnostic
                    .summary
                    .contains("`match take` requires `features: local`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_mut_field_binding_from_managed_class_pattern() {
    let source = r#"
class Node {
    name: String
}

fn main(node: read Node) -> Unit {
    match mut node {
        Node { name: mut value } => {
            return Unit
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-managed-mut-field.rss", source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0310"
                && diagnostic
                    .summary
                    .contains("managed pattern field `name` cannot request `mut`")
        }),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_marks_scrutinee_moved_after_take_match() {
    let source = r#"
features: local

struct User {
    name: String
}

fn main() -> String {
    local user = User(name: "x")
    match take user {
        User { name } => {
        }
    }
    return describe(user: read user)
}

fn describe(user: read User) -> String {
    return "user"
}
"#;
    let diagnostics = analyze_source("match-take-use-after.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0401"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_marks_scrutinee_moved_after_take_match_expression() {
    let source = r#"
features: local

struct User {
    name: String
}

fn main() -> String {
    local user = User(name: "x")
    let label = match take user {
        User { name } => {
            "done"
        }
    }
    return describe(user: read user)
}

fn describe(user: read User) -> String {
    return "user"
}
"#;
    let diagnostics = analyze_source("match-take-expr-use-after.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0401"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_rejects_overlapping_mut_pattern_fields() {
    let source = r#"
features: local

struct User {
    name: String
}

fn main(user: mut User) -> Unit {
    match mut user {
        User { name: mut left, name: mut right } => {
            return Unit
        }
    }
    return Unit
}
"#;
    let diagnostics = analyze_source("match-pattern-field-conflict.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0302"),
        "{diagnostics:?}"
    );
}

#[test]
fn checker_does_not_accept_if_is_without_explicit_effect() {
    let source = r#"
sum Expr {
    Call(callee: String)
    Name(value: String)
}

fn main(expr: read Expr) -> String {
    if expr is Call { callee } {
        return callee
    }
    return "done"
}
"#;
    let diagnostics = analyze_source("if-is-no-effect.rss", source);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015" || diagnostic.code == "RS0209"),
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

// A `mut`-annotated `Fn`-type parameter (`owned Fn(mut Ctx) -> Unit`) makes the
// matching closure parameter a mutable binding: the body may update its fields,
// exactly like a regular `mut` function parameter. This must check cleanly.
#[test]
fn mut_fn_param_is_mutable_in_closure_body() {
    let source = r#"
features: local

struct Ctx derives(Clone) {
    fired: Int
}

struct Rule derives(Clone) {
    fxn: owned Fn(mut Ctx) -> Unit
}

fn build() -> fresh Rule {
    return Rule(fxn: fn(ctx) captures() effects(pure) {
        ctx.fired = ctx.fired + 1
        return Unit
    })
}
"#;
    let errors = analyze_source("mut-fn-param-clean.rss", source)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "a `mut` Fn-param mutating its fields must check cleanly, got {errors:?}"
    );
}

// Soundness: a `mut` Fn-param is an exclusive mutable BORROW for the call — its
// fields/elements may be updated, but the parameter binding itself is not
// reassignable, exactly like a regular `mut` parameter. Rebinding it is RS0311.
#[test]
fn mut_fn_param_binding_is_not_reassignable() {
    let source = r#"
features: local

struct Ctx derives(Clone) {
    fired: Int
}

struct Rule derives(Clone) {
    fxn: owned Fn(mut Ctx) -> Unit
}

fn build() -> fresh Rule {
    return Rule(fxn: fn(ctx) captures() effects(pure) {
        ctx = Ctx(fired: 99)
        return Unit
    })
}
"#;
    let codes = analyze_source("mut-fn-param-rebind.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0311".to_string()),
        "rebinding a `mut` Fn-param binding must be rejected, got {codes:?}"
    );
}
