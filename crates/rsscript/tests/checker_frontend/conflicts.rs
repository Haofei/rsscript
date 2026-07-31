//! Spec §8 — places, conflict roots, same-call conflicts
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn checker_rejects_misplaced_or_duplicate_module_use_declarations() {
    let source = r#"
use rss.review.ReviewMap

module rss.package.review

fn first() -> Unit {
    return Unit
}

use rss.package.contract.PackageContract

module rss.package.other
"#;
    let diagnostics = analyze_source("bad-module-layout.rss", source);

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "misplaced use declaration"
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "duplicate module declaration"
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS0015" && diagnostic.label == "misplaced module declaration"
    }));
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

    assert!(rust.contains("callback(&41i64)"), "{rust}");
    assert!(rust.contains("|item: &i64|"));
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
fn checker_accepts_noescape_callback_that_temporarily_uses_local() {
    let source = r#"
features: local

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

struct Asset {
    id: Int
}

struct AssetError

fn load_asset(path: read Path) -> Result<fresh Asset, AssetError> {
    return Ok(Asset(id: 1))
}

fn inspect_asset(asset: read Asset) -> Unit {
    return Unit
}

fn use_local(path: read Path) -> Result<fresh Asset, AssetError> {
    local asset = load_asset(path: read path)?
    apply(callback: || {
        inspect_asset(asset: read asset)
    })
    return Ok(asset)
}

fn main() -> Result<Unit, AssetError> {
    let path = Path.from_string(value: read "asset-input.bin")
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
fn rust_lowering_accepts_explicit_fn_capture_contract() {
    let source = r#"
features: local

fn run() -> Int {
    let offset = 2
    local add = fn(value) captures(read offset) effects(pure) {
        return value + offset
    }
    return add(40)
}
"#;
    let diagnostics = analyze_source("explicit-fn-capture.rss", source);
    assert_eq!(diagnostics, Vec::new());

    let lowered = lower_source_to_rust("explicit-fn-capture.rss", source)
        .expect("explicit fn capture source should lower");
    assert!(lowered.contains("let add = |value|"));
    assert!(lowered.contains("value + offset"));
    assert!(lowered.contains("add(40i64)"));
}

#[test]
fn checker_accepts_user_authored_owned_fn_parameter_with_explicit_capture_contract() {
    let source = r#"
fn apply(callback: owned Fn(Int) -> Int, value: Int) -> Int {
    return callback(value)
}

fn run() -> Int {
    let offset = 2
    return apply(
        callback: fn(value) captures(read offset) effects(pure) {
            return value + offset
        },
        value: 40,
    )
}
"#;
    let diagnostics = analyze_source("owned-fn.rss", source);

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn rust_lowering_uses_move_for_owned_explicit_fn_capture() {
    let source = r#"
fn apply(callback: owned Fn(Int) -> Int, value: Int) -> Int {
    return callback(value)
}

fn run() -> Int {
    let offset = 2
    return apply(
        callback: fn(value) captures(read offset) effects(pure) {
            return value + offset
        },
        value: 40,
    )
}
"#;

    let lowered = lower_source_to_rust("owned-fn-lowering.rss", source)
        .expect("owned explicit fn should lower");

    assert!(
        lowered.contains("mut callback: impl FnMut(&i64) -> i64"),
        "{lowered}"
    );
    assert!(lowered.contains("callback(&value)"), "{lowered}");
    assert!(lowered.contains("move |value: &i64|"), "{lowered}");
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
fn long_finite_type_alias_chain_is_not_truncated() {
    let mut source = String::new();
    for index in 0..40 {
        source.push_str(&format!("type A{index:02} = A{:02}\n", index + 1));
    }
    source.push_str("type A40 = Int\n\nfn identity(value: A00) -> Int { return value }\n");

    let diagnostics = analyze_source("long-alias-chain.rss", &source);
    assert!(
        diagnostics.iter().all(|diagnostic| {
            diagnostic.code != "RS0024"
                && diagnostic.code != "RS0208"
                && diagnostic.code != "RS0039"
        }),
        "finite aliases must expand to completion: {diagnostics:?}"
    );
}

#[test]
fn cyclic_type_aliases_are_rejected_explicitly() {
    let source = r#"
type A = B
type B = List<A>
"#;

    let diagnostics = analyze_source("cyclic-alias.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0039"),
        "cyclic aliases need a dedicated diagnostic: {diagnostics:?}"
    );
}

#[test]
fn growing_generic_alias_cycles_are_rejected_without_recursing() {
    for source in [
        r#"
type A<T> = A<List<T>>

fn sink(value: read A<Int>) -> Unit {
    return Unit
}
"#,
        r#"
type A<T> = B<List<T>>
type B<T> = A<List<T>>

fn sink(value: read A<Int>) -> Unit {
    return Unit
}
"#,
    ] {
        let diagnostics = analyze_source("growing-cyclic-alias.rss", source);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "RS0039"),
            "growing generic cycles need RS0039: {diagnostics:?}"
        );
    }
}

#[test]
fn generic_alias_parameters_do_not_resolve_to_global_aliases() {
    let source = r#"
type Boxed<T> = List<T>
type T = Boxed<Int>

fn identity(value: T) -> T {
    return value
}
"#;

    let diagnostics = analyze_source("generic-alias-shadow.rss", source);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS0039"),
        "a bound alias parameter must not create a false alias cycle: {diagnostics:?}"
    );
}

#[test]
fn nested_reuse_of_a_non_recursive_generic_alias_fully_expands() {
    let source = r#"
type Wrapped<T> = List<T>

fn consume(values: read List<List<Int>>) -> Unit {
    return Unit
}

fn main(values: read Wrapped<Wrapped<Int>>) -> Unit {
    consume(values: read values)
    return Unit
}
"#;
    let diagnostics = analyze_source("nested-generic-alias.rss", source);
    assert_eq!(diagnostics, Vec::new());
}

#[test]
fn generic_protocol_declarations_are_rejected_precisely() {
    let source = r#"
protocol Convert<T> {
    fn convert(self: read Self) -> T
}
"#;

    let diagnostics = analyze_source("generic-protocol.rss", source);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "RS0015" && diagnostic.label == "generic protocol declaration"
        }),
        "reserved protocol generics need a precise diagnostic: {diagnostics:?}"
    );
}

// ---- First-class `owned Fn` values: acceptance + soundness boundary ----

#[test]
fn checker_accepts_owned_fn_as_first_class_storable_value() {
    // `owned Fn` is storable: a generic argument, a struct field, a binding, and
    // a function return; a closure literal fills it and is called as a value.
    let source = r#"
features: local

struct Adder derives(Clone) {
    fxn: owned Fn(Int) -> Int
}

fn make() -> fresh List<owned Fn(Int) -> Int> {
    local fns = List.new<owned Fn(Int) -> Int>()
    let k = 10
    let g = fn(x) captures(read k) effects(pure) { return x + k }
    List.push(list: mut fns, value: read g)
    return take fns
}

fn run() -> Int {
    local adders = List.new<Adder>()
    let base = 5
    let a = Adder(fxn: fn(x) captures(read base) effects(pure) { return x * base })
    List.push(list: mut adders, value: read a)
    let r = List.get(list: read adders, index: 0)
    let f = r.fxn
    return f(3)
}
"#;
    let diagnostics = analyze_source("owned-fn-first-class.rss", source);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn checker_rejects_noescape_fn_in_storable_position() {
    // SOUNDNESS BOUNDARY: `noescape Fn` is parameter-only. Storing it in a struct
    // field (or any non-parameter position) must still be rejected — a noescape
    // callback may borrow-capture, so storing it would let a borrow escape.
    let source = r#"
features: local

struct Holder {
    fxn: noescape Fn(Int) -> Int
}

fn run() -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("noescape-stored.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0015" && d.label == "unsupported noescape position"),
        "noescape Fn must stay parameter-only: {diagnostics:?}"
    );
}

#[test]
fn checker_rejects_noescape_fn_hidden_behind_alias() {
    let source = r#"
features: local

type HiddenCallback = noescape Fn(Int) -> Int

struct Holder {
    callback: HiddenCallback
}
"#;
    let diagnostics = analyze_source("aliased-noescape-stored.rss", source);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "RS0015" && d.label == "unsupported noescape position"),
        "aliases must not conceal a noescape callback in a stored position: {diagnostics:?}"
    );
}

#[test]
fn checker_rejects_unsized_take_callbacks_after_alias_expansion() {
    let source = r#"
features: local

type Handler = Fn(Int) -> Int
type OwnedHandler = owned Fn(Int) -> Int

fn apply_read(callback: read Fn(Int) -> Int) -> Unit {
    return Unit
}

fn apply_mut(callback: mut Handler) -> Unit {
    return Unit
}

fn apply_take(callback: take Fn(Int) -> Int) -> Unit {
    return Unit
}

fn apply_aliased_take(callback: take Handler) -> Unit {
    return Unit
}

fn apply_owned_take(callback: take OwnedHandler) -> Unit {
    return Unit
}
"#;
    let diagnostics = analyze_source("callback-outer-effects.rss", source);
    let effect_diagnostics = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "RS0015"
                && diagnostic.label == "unsupported by-value callback parameter"
        })
        .count();
    assert_eq!(
        effect_diagnostics, 2,
        "only unsized direct and aliased `take Fn` parameters are invalid: {diagnostics:?}"
    );
}

#[test]
fn checker_rejects_owned_closure_capturing_non_copy_value_by_read() {
    // SOUNDNESS BOUNDARY: an escaping/stored `owned` closure may capture only
    // owned (move/`take`) or `Copy` values. A non-`Copy` `read` capture would be
    // a borrow that dangles once the closure escapes, so it is rejected: a
    // non-`Copy` `String` captured with `read` while the body needs ownership
    // (consumes/returns it) fails the capture contract.
    let source = r#"
features: local

struct Holder derives(Clone) {
    fxn: owned Fn() -> fresh String
}

fn run() -> fresh String {
    let s = "captured"
    let h = Holder(fxn: fn() captures(read s) effects(pure) { return take s })
    let f = h.fxn
    return f()
}
"#;
    let diagnostics = analyze_source("owned-read-noncopy.rss", source);
    assert!(
        diagnostics.iter().any(|d| d.code == "RS0805"),
        "a non-Copy `read` capture used as `take` must be rejected: {diagnostics:?}"
    );
}
