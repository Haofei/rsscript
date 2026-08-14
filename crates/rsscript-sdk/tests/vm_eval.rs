//! Register-VM execution tests: drive `eval`/`eval_package` (source -> run) for
//! single files and packages, plus async and native-host-binding parity against
//! the compiled backend. (Despite the historical name, RSScript has one VM; this
//! is the eval-level companion to `vm.rs`'s compile-level tests.)
#![allow(unused_imports)]

mod common;

use base64::Engine;
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

use common::{
    lower_test_source_to_rust_package as lower_source_to_rust_package, reg_vm_eval_source_main,
    reg_vm_eval_source_main_with_args, reg_vm_eval_source_main_with_args_and_external_bindings,
    reg_vm_eval_source_main_with_interfaces_and_external_bindings,
};
use rsscript_provider_api::AsyncInterpreterFn;
use rsscript_sdk::{
    BlockingBehavior, CancellationBehavior, EvalError, ExternalFunction, ExternalFunctionRegistry,
    ExternalSymbol, FunctionSignature, NativeRustDependency, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderError, ProviderErrorMapping, ProviderFunction,
    ProviderFunctionDescriptor, ResourceCleanupContract, VmLimits,
    lower_sources_to_rust_package_with_options, reg_vm_eval_package_main_with_args,
    write_generated_rust_package,
};

#[test]
fn eval_runs_pure_arithmetic_main() {
    let source = r#"
fn main(args: read List<String>) -> Int {
    let x = 2
    let y = 3
    return x + y * 4
}
"#;

    let output =
        reg_vm_eval_source_main("eval-arithmetic.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "14");
    assert_eq!(output.display_value, "14");
    assert_eq!(output.native_value, Some(NativeValue::Int(14)));
}

#[test]
fn eval_runs_user_function_and_assignment() {
    let source = r#"
fn add(a: Int, b: Int) -> Int {
    return a + b
}

fn main(args: read List<String>) -> Int {
    let mut total = add(a: 1, b: 2)
    total = total + 4
    return total
}
"#;

    let output = reg_vm_eval_source_main("eval-function.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "7");
}

#[test]
fn map_keys_are_stable_value_snapshots() {
    let source = r#"
struct Key derives(Eq, Hash) {
    id: Int
}

fn change_id(key: mut Key, new_id: Int) -> Unit {
    key.id = new_id
    return Unit
}

fn main(args: read List<String>) -> Bool {
    let key = Key(id: 1)
    let map = Map.new<Key, Int>()
    Map.insert<Key, Int>(map: mut map, key, value: 7)
    change_id(key: mut key, new_id: 2)
    let original = Key(id: 1)
    return Map.contains_key<Key, Int>(map: map, key: original)
        && !Map.contains_key<Key, Int>(map: map, key: key)
}
"#;

    let output =
        reg_vm_eval_source_main("stable-map-key.rss", source).expect("eval should succeed");
    assert_eq!(output.value, "true");
}

/// Build a program that folds a large `List<Float>` with `folder_body` and
/// returns the sum formatted as a string. The list values are deterministic so
/// the fast and slow folders must agree bit-for-bit.
fn float_fold_program(folder_body: &str) -> String {
    // Build the list once, then fold it many times so the fold (not the one-time
    // list construction) dominates the measured time.
    format!(
        r#"fn main(args: read List<String>) -> Float {{
    let mut index = 0
    local values = List<Float>.new()
    while index < 50000 {{
        let f = Int.to_float(value: read index)
        List.push<Float>(list: mut values, value: read (f * 0.5 - 1.0))
        index = index + 1
    }}
    let mut acc = 0.0
    let mut rep = 0
    while rep < 50 {{
        let total = List.fold<Float, Float>(
            list: read values,
            initial: read 0.0,
            folder: {folder_body},
        )
        acc = acc + total
        rep = rep + 1
    }}
    return acc
}}
"#
    )
}

/// The bulk `List<Float>.fold` fast path must be a pure performance change: a
/// recognized numeric folder (`|acc, x| acc + x`) and an equivalent folder the
/// recognizer rejects (an extra `let` binding forces the generic interpreter
/// path) must produce *bit-identical* results, and the fast path should be
/// materially faster on a large list.
#[test]
fn float_fold_fast_path_matches_slow_path_and_is_faster() {
    use std::time::Instant;

    // Recognized shape: body is exactly `<binop>; Return`.
    let fast_src = float_fold_program("|acc, x| acc + x");
    // Rejected shape: identical math, but the extra statement means the body is
    // not `[binop, Return]`, so the recognizer declines and the generic
    // per-element closure path runs.
    let slow_src = float_fold_program("|acc, x| { let y = x\n        return acc + y }");

    // Warm up + correctness: both paths must yield the same f64 string.
    let fast0 = reg_vm_eval_source_main("float-fold-fast.rss", &fast_src).expect("fast eval");
    let slow0 = reg_vm_eval_source_main("float-fold-slow.rss", &slow_src).expect("slow eval");
    assert_eq!(
        fast0.value, slow0.value,
        "fast and slow float fold must be bit-identical"
    );
    assert_eq!(fast0.native_value, slow0.native_value);

    let reps = 3;
    let mut fast_ns = u128::MAX;
    let mut slow_ns = u128::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        let r = reg_vm_eval_source_main("float-fold-fast.rss", &fast_src).expect("fast eval");
        fast_ns = fast_ns.min(t.elapsed().as_nanos());
        assert_eq!(r.value, fast0.value);

        let t = Instant::now();
        let r = reg_vm_eval_source_main("float-fold-slow.rss", &slow_src).expect("slow eval");
        slow_ns = slow_ns.min(t.elapsed().as_nanos());
        assert_eq!(r.value, slow0.value);
    }

    let speedup = slow_ns as f64 / fast_ns as f64;
    println!(
        "float fold (50k elems x50 folds): fast={:.2}ms slow={:.2}ms speedup={:.2}x result={}",
        fast_ns as f64 / 1.0e6,
        slow_ns as f64 / 1.0e6,
        speedup,
        fast0.value,
    );
    // The fast path must not regress; on a 200k-element fold it is far faster
    // than the per-element closure interpreter. Keep the assertion margin modest
    // to stay robust under CI scheduling noise while still catching a regression
    // that lost the fast path entirely.
    assert!(
        speedup > 2.0,
        "expected the bulk float-fold fast path to be materially faster, got {speedup:.2}x \
         (fast={fast_ns}ns slow={slow_ns}ns)"
    );
}

#[test]
fn eval_package_runs_merged_sources_with_args() {
    let package_dir = common::unique_temp_dir("rsscript-eval-package");
    fs::create_dir_all(package_dir.join("src")).expect("package src should create");
    fs::write(
        package_dir.join("rsspkg.toml"),
        r#"[package]
name = "rsscript-eval-package"
version = "0.1.0"
edition = "2026"

[sources]
paths = ["src"]

[dependencies]
test-host = { path = "host" }
"#,
    )
    .expect("manifest should write");
    fs::create_dir_all(package_dir.join("host/interface")).expect("interface dir should create");
    fs::write(
        package_dir.join("host/rsspkg.toml"),
        r#"[package]
name = "test-host"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[virtual]
has_default = false
provider = "test"
"#,
    )
    .expect("host manifest should write");
    fs::write(package_dir.join("host/interface/host.rssi"), "")
        .expect("host interface should write");
    fs::write(
        package_dir.join("src/helper.rss"),
        r#"
fn decorate(value: read String) -> String {
    return value
}
"#,
    )
    .expect("helper source should write");
    fs::write(
        package_dir.join("src/main.rss"),
        r#"
fn main(args: read List<String>) -> Unit {
    let args = Arguments.all(args: read args)
    let joined = List.join<String>(list: read args, separator: read "|")
    Output.write(message: read decorate(value: read joined))
    return Unit
}
"#,
    )
    .expect("main source should write");

    let output = reg_vm_eval_package_main_with_args(&package_dir, ["alpha", "beta"])
        .expect("package eval should run");

    assert_eq!(output.stdout, "alpha|beta\n");
    assert_eq!(output.value, "Unit");
    let _ = fs::remove_dir_all(package_dir);
}

#[test]
fn eval_runs_nested_pattern_match() {
    let source = r#"
fn main(args: read List<String>) -> String {
    let value = Some(Some("rss"))
    match value {
        Some(Some(text)) => {
            return read text
        }
        Some(None) => {
            return "inner none"
        }
        None => {
            return "none"
        }
    }
}
"#;

    let output =
        reg_vm_eval_source_main("eval-nested-match.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "rss");
}

#[test]
fn parity_task_group_async_let_runs_spawn_handles() {
    let source = r#"

async fn fetch_user() -> Result<String, String> {
    return Ok("user")
}

async fn fetch_profile() -> Result<String, String> {
    return Ok("profile")
}

fn main(args: read List<String>) -> Result<Unit, String> {
    task_group {
        async let user = fetch_user()
        async let profile = fetch_profile()

        let u = await user?
        let p = await profile?
        Output.write(message: read u)
        Output.write(message: read p)
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-task-group.rss",
        "rsscript_parity_task_group",
        source,
    );
}

#[test]
fn eval_matches_lowered_rust_for_pure_core_example() {
    let source_path = "examples/scripts/core/interpreter_pure_parity.rss";
    let source = fs::read_to_string(common::workspace_root().join(source_path))
        .expect("parity fixture should be readable");
    let eval = reg_vm_eval_source_main(source_path, &source).expect("eval should succeed");
    assert_eq!(eval.value, "Unit");

    let runtime_path = common::runtime_path();
    let package =
        lower_source_to_rust_package(source_path, &source, "rsscript_eval_parity", &runtime_path)
            .expect("parity fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, eval.stdout);
    assert_eq!(stderr, eval.stderr);
}

#[test]
fn eval_string_len_matches_lowered_rust_for_utf8_bytes() {
    let source = r#"
fn main(args: read List<String>) -> Unit {
    let len = String.len(value: read "é")
    Output.write(message: read String.from_int(value: len))
}
"#;
    let eval =
        reg_vm_eval_source_main("eval-string-len-utf8.rss", source).expect("eval should succeed");
    assert_eq!(eval.value, "Unit");
    assert_eq!(eval.stdout, "2\n");

    let runtime_path = common::runtime_path();
    let package = lower_source_to_rust_package(
        "eval-string-len-utf8.rss",
        source,
        "rsscript_eval_string_len_utf8",
        &runtime_path,
    )
    .expect("utf8 length fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-string-len-utf8");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "2\n");
    assert_eq!(stderr, "");
}

#[test]
fn eval_dispatches_native_host_bindings() {
    fn host_echo(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [NativeValue::String(message)] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::String(format!("host:{message}")))
    }

    fn host_tag(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [NativeValue::Int(value)] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::String(format!("tag:{value}")))
    }

    let interface = r#"
pub fn Host.echo(message: read String) -> String
pub fn Host.tag(value: Int) -> String
"#;
    let source = r#"
fn main(args: read List<String>) -> Unit {
    Output.write(message: read Host.echo(message: read "hello"))
    Output.write(message: read Host.tag(value: 7))
    return Unit
}
"#;

    let eval = reg_vm_eval_source_main_with_interfaces_and_external_bindings(
        "eval-native-host.rss",
        source,
        &[("host-bindings.rssi", interface)],
        [
            ("Host.echo", ExternalFunction::from_fn(host_echo)),
            ("Host.tag", ExternalFunction::from_fn(host_tag)),
        ],
    )
    .expect("native host binding eval should succeed");

    assert_eq!(eval.value, "Unit");
    assert_eq!(eval.stdout, "host:hello\ntag:7\n");
    assert_eq!(eval.stderr, "");
}

#[test]
fn eval_suspends_and_resumes_an_async_provider_call() {
    let symbol = ExternalSymbol::new("Host.async_value").unwrap();
    let signature = FunctionSignature {
        parameters: Vec::new(),
        result: "Int".into(),
        asynchronous: true,
    };
    let descriptor = ProviderDescriptor {
        provider_id: "test.async".into(),
        provider_version: "1.0.0".into(),
        supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "async_value".into(),
            call_mode: ProviderCallMode::Async,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::Cooperative,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    };
    let callable = AsyncInterpreterFn::new(|_, _| async {
        let mut first_poll = true;
        std::future::poll_fn(move |context| {
            if first_poll {
                first_poll = false;
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(Ok(NativeValue::Int(42)))
            }
        })
        .await
    });
    let mut registry = ExternalFunctionRegistry::new();
    registry
        .register_provider(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable,
                },
            )]),
        )
        .unwrap();

    let output = reg_vm_eval_source_main_with_interfaces_and_external_bindings(
        "eval-async-provider.rss",
        "async fn main() -> Int { return await Host.async_value() }",
        &[(
            "host-async.rssi",
            "pub async fn Host.async_value() -> Int\n",
        )],
        registry.into_bindings(),
    )
    .expect("async provider should resume the suspended VM task");

    assert_eq!(output.native_value, Some(NativeValue::Int(42)));
    assert_eq!(output.usage.provider_calls, 1);
}

#[test]
fn eval_reports_unbound_external_declarations() {
    let interface = "pub fn Host.echo(message: read String) -> String\n";
    let source = r#"
fn main(args: read List<String>) -> Unit {
    Output.write(message: read Host.echo(message: read "hello"))
    return Unit
}
"#;

    let error = common::compile_vm_source_with_interfaces(
        "eval-unbound-external.rss",
        source,
        &[("host-bindings.rssi", interface)],
    )
    .expect("external declaration should compile")
    .eval_main_with_args(std::iter::empty::<String>())
    .expect_err("unbound external declaration should fail");

    assert!(
        matches!(error, EvalError::Runtime(ref message) if message.contains("Host.echo") && message.contains("no host binding")),
        "{error:?}"
    );
}

#[test]
fn eval_receiver_external_bindings_use_resolved_receiver_namespace() {
    fn alpha_open(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::Native {
            type_name: "Alpha".to_string(),
            id: 1,
        })
    }

    fn beta_open(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::Native {
            type_name: "Beta".to_string(),
            id: 2,
        })
    }

    fn alpha_describe(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::String(format!("alpha:{type_name}:{id}")))
    }

    fn beta_describe(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::String(format!("beta:{type_name}:{id}")))
    }

    let interface = r#"
opaque struct Alpha
opaque struct Beta
pub fn Alpha.open() -> Alpha
pub fn Alpha.describe(self: read Alpha) -> String
pub fn Beta.open() -> Beta
pub fn Beta.describe(self: read Beta) -> String
"#;
    let source = r#"
fn main(args: read List<String>) -> Unit {
    let alpha = Alpha.open()
    let beta = Beta.open()
    Output.write(message: read alpha.describe())
    Output.write(message: read beta.describe())
    return Unit
}
"#;

    let output = reg_vm_eval_source_main_with_interfaces_and_external_bindings(
        "receiver-native-bindings.rss",
        source,
        &[("receiver-bindings.rssi", interface)],
        [
            ("Alpha.open", ExternalFunction::from_fn(alpha_open)),
            ("Alpha.describe", ExternalFunction::from_fn(alpha_describe)),
            ("Beta.open", ExternalFunction::from_fn(beta_open)),
            ("Beta.describe", ExternalFunction::from_fn(beta_describe)),
        ],
    )
    .expect("receiver native host binding eval should succeed");

    assert_eq!(output.stdout, "alpha:Alpha:1\nbeta:Beta:2\n");
}

/// Pin the reg-VM behavior of every closure-taking List/Map intrinsic that now
/// iterates the receiver live (no defensive whole-collection snapshot clone).
/// Exercises each converted op — including early-return paths (any/find hits)
/// and a `try_fold` Err path — and asserts the exact emitted output.
#[test]
fn eval_reg_vm_closure_intrinsics_iterate_live() {
    let source = r#"

fn is_even(value: Int) -> Bool {
    let half = value / 2
    return half * 2 == value
}

fn main(args: read List<String>) -> Unit {
    let numbers: List<Int> = [1, 2, 3, 4, 5]

    Output.write(message: read String.from_bool(value: List.all<Int>(list: read numbers, predicate: |item| {
        return item > 0
    })))
    Output.write(message: read String.from_bool(value: List.any<Int>(list: read numbers, predicate: |item| {
        return item == 2
    })))
    Output.write(message: read String.from_int(value: List.count_where<Int>(list: read numbers, predicate: |item| {
        return item > 3
    })))

    match List.find<Int>(list: read numbers, predicate: |item| {
        return item > 3
    }) {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "find-none")
        }
    }

    let parts = List.partition<Int>(list: read numbers, predicate: |item| {
        return is_even(value: item)
    })
    Output.write(message: read String.from_int(value: List.len<Int>(list: read parts[0])))
    Output.write(message: read String.from_int(value: List.len<Int>(list: read parts[1])))

    let flattened = List.flat_map<Int, Int>(list: read numbers, mapper: |item| {
        let values: List<Int> = [item, item + 10]
        return values
    })
    Output.write(message: read String.from_int(value: List.len<Int>(list: read flattened)))
    Output.write(message: read String.from_int(value: flattened[1]))

    let grouped = List.group_by<Int, String>(list: read numbers, key: |item| {
        if is_even(value: item) {
            return String.copy(value: read "even")
        }
        return String.copy(value: read "odd")
    })
    match Map.get(map: read grouped, key: read "even") {
        Some(items) => {
            Output.write(message: read String.from_int(value: List.len(list: read items)))
        }
        None => {
            Output.write(message: read "even-missing")
        }
    }

    match List.try_fold<Int, Int, String>(list: read [1, 2], initial: read 0, folder: |state, item| {
        return Ok(state + item)
    }) {
        Ok(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }
    match List.try_fold<Int, Int, String>(list: read numbers, initial: read 0, folder: |state, item| {
        if item > 3 {
            return Err(String.copy(value: read "too-large"))
        }
        return Ok(state + item)
    }) {
        Ok(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let mut left = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut left, key: read "a", value: read 1)
    Map.insert<String, Int>(map: mut left, key: read "b", value: read 2)

    let mapped = Map.map_values<String, Int, Int>(map: read left, mapper: |value| {
        return value + 10
    })
    match Map.get<String, Int>(map: read mapped, key: read "a") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "mapped-missing")
        }
    }

    let filtered = Map.filter<String, Int>(map: read mapped, predicate: |key, value| {
        return key == "b" && value > 10
    })
    Output.write(message: read String.from_int(value: Map.len<String, Int>(map: read filtered)))

    let mut single = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut single, key: read "only", value: read 8)
    Map.for_each<String, Int>(map: read single, callback: |key, value| {
        Output.write(message: read key)
        Output.write(message: read String.from_int(value: value))
        return Unit
    })

    let folded = Map.fold<String, Int, Int>(map: read left, initial: read 0, folder: |state, key, value| {
        return state + value
    })
    Output.write(message: read String.from_int(value: folded))

    match Map.try_fold<String, Int, Int, String>(map: read left, initial: read 0, folder: |state, key, value| {
        if key == "b" {
            return Err(String.copy(value: read "stop-b"))
        }
        return Ok(state + value)
    }) {
        Ok(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        Err(error) => {
            Output.write(message: read error)
        }
    }

    let mut right = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut right, key: read "b", value: read 20)
    Map.insert<String, Int>(map: mut right, key: read "c", value: read 30)
    let merged = Map.merge<String, Int>(left: read left, right: read right, resolver: |left_value, right_value| {
        return left_value + right_value
    })
    match Map.get<String, Int>(map: read merged, key: read "b") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "merge-b-missing")
        }
    }
    match Map.get<String, Int>(map: read merged, key: read "c") {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "merge-c-missing")
        }
    }
    return Unit
}
"#;

    let output = reg_vm_eval_source_main("reg-vm-live-closure-intrinsics.rss", source)
        .expect("eval should succeed");

    let expected = "true\n\
true\n\
2\n\
4\n\
2\n\
3\n\
10\n\
11\n\
2\n\
3\n\
too-large\n\
11\n\
1\n\
only\n\
8\n\
3\n\
stop-b\n\
22\n\
30\n";
    assert_eq!(output.stdout, expected);
}

#[test]
fn parity_native_host_bindings_match_lowered_backend() {
    fn host_open(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::Native {
            type_name: "HostHandle".to_string(),
            id: 7,
        })
    }

    fn host_describe(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::String(format!("{type_name}:{id}")))
    }

    fn host_echo(args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        let [NativeValue::String(message)] = args.as_slice() else {
            return Err(ProviderError::internal(format!(
                "unexpected args: {args:?}"
            )));
        };
        Ok(NativeValue::String(format!("host:{message}")))
    }

    let interface = r#"
opaque struct HostHandle
pub fn Host.open() -> HostHandle
pub fn Host.describe(handle: read HostHandle) -> String
pub fn Host.echo(message: read String) -> String
"#;
    let source = r#"
fn main(args: read List<String>) -> Unit {
    let handle = Host.open()
    Output.write(message: read Host.describe(handle: read handle))
    Output.write(message: read Host.echo(message: read "native"))
    return Unit
}
"#;

    let eval = reg_vm_eval_source_main_with_interfaces_and_external_bindings(
        "parity-native-host.rss",
        source,
        &[("host-bindings.rssi", interface)],
        [
            ("Host.open", ExternalFunction::from_fn(host_open)),
            ("Host.describe", ExternalFunction::from_fn(host_describe)),
            ("Host.echo", ExternalFunction::from_fn(host_echo)),
        ],
    )
    .expect("interpreter should run native host binding");

    let package = "rsscript_parity_native_host";
    let native_dir = common::unique_temp_dir("rsscript-parity-native-crate");
    fs::create_dir_all(native_dir.join("src")).expect("native crate src dir should create");
    fs::write(
        native_dir.join("Cargo.toml"),
        r#"[package]
name = "rsscript_test_native"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("native Cargo.toml should write");
    fs::write(
        native_dir.join("src/lib.rs"),
        r#"#[derive(Clone, Debug)]
pub struct HostHandle {
    id: i64,
}

pub fn open() -> HostHandle {
    HostHandle { id: 7 }
}

pub fn describe(handle: &HostHandle) -> String {
    format!("HostHandle:{}", handle.id)
}

pub fn echo(message: &String) -> String {
    format!("host:{message}")
}
"#,
    )
    .expect("native lib should write");

    let runtime_path = common::runtime_path();
    let lowered = lower_sources_to_rust_package_with_options(
        &[("parity-native-host.rss".to_string(), source.to_string())],
        package,
        &runtime_path,
        &[("host-bindings.rssi".to_string(), interface.to_string())],
        &[NativeRustDependency {
            crate_name: "rsscript_test_native".to_string(),
            path: native_dir.to_string_lossy().to_string(),
            cargo_features: Vec::new(),
            default_features: true,
            bindings: BTreeMap::from([
                (
                    "Host.echo".to_string(),
                    "rsscript_test_native::echo".to_string(),
                ),
                (
                    "Host.open".to_string(),
                    "rsscript_test_native::open".to_string(),
                ),
                (
                    "Host.describe".to_string(),
                    "rsscript_test_native::describe".to_string(),
                ),
            ]),
        }],
    )
    .expect("source should lower with native binding");
    let package_dir = common::unique_temp_dir(package);
    write_generated_rust_package(&package_dir, &lowered).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);
    let _ = fs::remove_dir_all(&native_dir);

    assert!(
        output.status.success(),
        "generated Rust failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(eval.stdout, stdout);
    assert_eq!(eval.stderr, stderr);
}

// Differential parity harness: run the same source through the interpreter and
// through the lowered-Rust backend, then assert their observable output agrees.
// This is the mechanism (not docs) that keeps the interpreter from diverging
// from the authoritative backend — one fixture per supported construct.
// parity: function:native function:sync
// parity: hir_stmt:Assign hir_stmt:Break hir_stmt:Continue hir_stmt:Expr hir_stmt:For
// parity: hir_stmt:If hir_stmt:Let hir_stmt:Loop hir_stmt:Return hir_stmt:With
// parity: hir_expr:ArrayLiteral hir_expr:Binary hir_expr:Call hir_expr:Effect hir_expr:Field
// parity: hir_expr:Closure hir_expr:Ident hir_expr:Index hir_expr:Manage hir_expr:MapLiteral
// parity: hir_expr:Number hir_expr:ObjectLiteral hir_expr:String hir_expr:Try
// parity: value:Bool value:Bytes value:Float value:Int value:Json value:List value:Managed
// parity: value:Char value:Closure value:Map value:Native value:String value:Struct value:Unit value:Variant
// parity: runtime:Arguments.all runtime:Arguments.count runtime:Arguments.get runtime:Arguments.get_or_default
// parity: runtime:Assert.equal runtime:Assert.equal_bool runtime:Assert.equal_int
// parity: runtime:Base64.decode runtime:Base64.decode_string runtime:Base64.encode runtime:Base64.encode_bytes
// parity: runtime:Char.compare runtime:Char.from_code runtime:Char.is_alpha
// parity: runtime:Char.is_alphanumeric runtime:Char.is_digit runtime:Char.is_whitespace
// parity: runtime:Char.is_lower runtime:Char.is_upper
// parity: runtime:Char.to_code runtime:Char.to_lower runtime:Char.to_string runtime:Char.to_upper
// parity: runtime:DecodeError.message
// parity: runtime:Deque.clear runtime:Deque.is_empty runtime:Deque.len runtime:Deque.new
// parity: runtime:Deque.pop_back runtime:Deque.pop_front runtime:Deque.push_back
// parity: runtime:Deque.push_front runtime:Deque.to_list
// parity: runtime:Directory.create runtime:Directory.create_all runtime:Directory.create_dir_all
// parity: runtime:Directory.exists runtime:Directory.is_dir runtime:Directory.is_file
// parity: runtime:Directory.list_files runtime:Directory.list_paths runtime:Directory.metadata
// parity: runtime:Directory.copy_file runtime:Directory.rename runtime:Directory.remove_file
// parity: runtime:Directory.remove_dir_all runtime:Directory.read_string runtime:Directory.write_string
// parity: runtime:Duration.add runtime:Duration.as_ms runtime:Duration.as_seconds
// parity: runtime:Duration.ms runtime:Duration.seconds
// parity: runtime:Diff.unified runtime:Patch.apply_text
// parity: runtime:Bytes.concat runtime:Bytes.consume runtime:Bytes.from_string runtime:Bytes.is_empty
// parity: runtime:Bytes.from_buffer runtime:Bytes.len runtime:Bytes.slice runtime:Bytes.view
// parity: runtime:Bytes.from_uints runtime:Bytes.to_uints runtime:Bytes.to_string runtime:Gzip.decompress_bytes
// parity: runtime:BytesView.is_empty runtime:BytesView.len runtime:BytesView.slice
// parity: runtime:BytesView.starts_with runtime:BytesView.to_bytes
// parity: runtime:Int.bit_and runtime:Int.bit_not runtime:Int.bit_or runtime:Int.bit_xor
// parity: runtime:Int.shift_left runtime:Int.shift_right runtime:Int.to_string
// parity: runtime:Int.to_float
// parity: runtime:Buffer.clear runtime:Buffer.consume runtime:Buffer.is_empty runtime:Buffer.len
// parity: runtime:Buffer.new runtime:Buffer.view runtime:BufferView.is_empty
// parity: runtime:BufferView.len runtime:BufferView.slice runtime:BufferView.to_bytes
// parity: runtime:Channel.bounded runtime:Channel.message runtime:Channel.receiver runtime:Channel.sender
// parity: runtime:ChannelError.message
// parity: runtime:CancellationSource.cancel runtime:CancellationSource.new
// parity: runtime:CancellationSource.token runtime:CancellationToken.is_cancelled
// parity: runtime:Clone.clone
// parity: runtime:Clock.now runtime:Clock.system_unix_ms
// parity: runtime:Csv.open_read runtime:Csv.parse_row runtime:Csv.read_into runtime:Csv.rows
// parity: runtime:Date.add_days runtime:Date.add_ms runtime:Date.day runtime:Date.days_between
// parity: runtime:Date.days_in_month runtime:Date.format_iso runtime:Date.format_ymd
// parity: runtime:Date.hour runtime:Date.is_leap_year runtime:Date.minute
// parity: runtime:Date.month runtime:Date.parse_iso runtime:Date.parse_ymd runtime:Date.second
// parity: runtime:Date.start_of_day runtime:Date.weekday runtime:Date.year
// parity: runtime:Deadline.after runtime:Deadline.after_ms
// parity: runtime:Deadline.is_expired runtime:Deadline.remaining_ms
// parity: runtime:Json.array runtime:Json.array_bools runtime:Json.array_contains_prefix
// parity: runtime:Json.array_contains_string runtime:Json.array_contains_substring
// parity: runtime:Json.array_count_where runtime:Json.array_fold runtime:Json.array_get
// parity: runtime:Json.array_ints runtime:Json.array_len runtime:Json.array_strings
// parity: runtime:Json.as_bool runtime:Json.as_int runtime:Json.as_string runtime:Json.field
// parity: runtime:Json.at runtime:Json.at_bool runtime:Json.at_bool_or runtime:Json.at_int
// parity: runtime:Json.at_int_or runtime:Json.at_optional runtime:Json.at_optional_bool
// parity: runtime:Json.at_optional_int runtime:Json.at_optional_string runtime:Json.at_or
// parity: runtime:Json.at_string runtime:Json.at_string_or runtime:Json.at_to_string
// parity: runtime:Json.at_to_string_or
// parity: runtime:Json.bool_at runtime:Json.bool_at_or
// parity: runtime:Json.bool_field runtime:Json.clone
// parity: runtime:Json.field_bool runtime:Json.field_int runtime:Json.field_optional
// parity: runtime:Json.field_optional_bool runtime:Json.field_optional_int
// parity: runtime:Json.field_optional_string runtime:Json.field_string runtime:Json.is_array
// parity: runtime:Json.is_null runtime:Json.is_object runtime:Json.json_bool_at_or
// parity: runtime:Json.json_int_at_or runtime:Json.json_parse runtime:Json.json_string_at_or
// parity: runtime:Json.int_at runtime:Json.int_at_or runtime:Json.int_field
// parity: runtime:Json.kind runtime:Json.object runtime:Json.object_keys
// parity: runtime:Json.object_len runtime:Json.parse runtime:Json.parse_file
// parity: runtime:Json.quote_string runtime:Json.raw_field
// parity: runtime:Json.string_array runtime:Json.string_at runtime:Json.string_at_or
// parity: runtime:Json.strings runtime:Json.string_field runtime:Json.to_string
// parity: runtime:Json.to_string_at runtime:Json.to_string_at_or
// parity: runtime:Json.value runtime:Json.value_at runtime:Json.values runtime:JsonError.message
// parity: runtime:Instant.elapsed
// parity: runtime:Hash.sha256_bytes runtime:Hash.sha256_file runtime:Hash.sha256_string
// parity: runtime:Hash.sha3_224_bytes runtime:Hash.sha3_256_bytes runtime:Hash.shake128_bytes
// parity: runtime:Hmac.sha256_bytes runtime:Hmac.sha256_string
// parity: runtime:Hex.decode runtime:Hex.encode runtime:Hex.encode_string
// parity: runtime:Http.get runtime:Http.get_async runtime:Http.get_retry_async
// parity: runtime:Http.get_timeout_async runtime:Http.post_form runtime:Http.post_form_async
// parity: runtime:Http.post_json runtime:Http.post_json_async runtime:Http.post_json_bearer_retry_async
// parity: runtime:Http.post_json_retry_async runtime:Http.post_json_timeout_async
// parity: runtime:Http.send_async runtime:HttpError.message
// parity: runtime:HttpRequest.json runtime:HttpRequest.with_header
// parity: runtime:HttpRequest.with_retry runtime:HttpRequest.with_timeout
// parity: runtime:HttpResponse.bytes runtime:HttpResponse.is_success runtime:HttpResponse.lines
// parity: runtime:HttpResponse.status runtime:HttpResponse.text
// parity: runtime:List.all runtime:List.any runtime:List.append runtime:List.clear
// parity: runtime:List.contains runtime:List.contains_value runtime:List.count_where
// parity: runtime:List.consume runtime:List.dedup runtime:List.enumerate runtime:List.filter runtime:List.find runtime:List.first
// parity: runtime:List.flat_map runtime:List.flatten runtime:List.fold runtime:List.get runtime:List.group_by
// parity: runtime:List.is_empty runtime:List.join
// parity: runtime:List.last runtime:List.len runtime:List.map runtime:List.max runtime:List.min
// parity: runtime:List.new runtime:List.partition
// parity: runtime:List.pipeline runtime:List.pop runtime:List.push runtime:List.reverse runtime:List.remove_at runtime:List.set
// parity: runtime:List.skip runtime:List.slice runtime:List.sort runtime:List.sort_by
// parity: runtime:List.sort_with runtime:List.sum runtime:List.take
// parity: runtime:List.to_json_strings runtime:List.to_json_values runtime:List.try_fold runtime:List.zip
// parity: runtime:Output.error runtime:Output.error_json runtime:Output.trace runtime:Output.write
// parity: runtime:Output.write_json
// parity: runtime:Env.current_dir runtime:Env.get runtime:Env.get_or_default
// parity: runtime:Env.home_dir runtime:Env.run_workspace_root runtime:Env.set
// parity: runtime:Env.set_current_dir runtime:Env.temp_dir
// parity: runtime:File.append_bytes runtime:File.append_string runtime:File.bytes_stream runtime:File.exists
// parity: runtime:File.open runtime:File.open_read runtime:File.open_write
// parity: runtime:File.read_all runtime:File.read_all_async
// parity: runtime:File.read_all_string runtime:File.read_all_string_async runtime:File.read_into
// parity: runtime:File.read_bytes runtime:File.read_string runtime:File.remove
// parity: runtime:File.write runtime:File.write_async runtime:File.write_atomic runtime:File.write_bytes
// parity: runtime:File.write_bytes_view runtime:File.write_buffer runtime:File.write_buffer_view
// parity: runtime:File.write_string runtime:File.write_string_async runtime:File.write_string_to_path
// parity: runtime:FileError.message
// parity: runtime:FalliblePipeline.collect runtime:FalliblePipeline.each
// parity: runtime:FalliblePipeline.filter runtime:FalliblePipeline.map
// parity: runtime:FalliblePipeline.try_map
// parity: runtime:PersistentMap.clear runtime:PersistentMap.contains_key runtime:PersistentMap.get
// parity: runtime:PersistentMap.insert runtime:PersistentMap.is_empty runtime:PersistentMap.len
// parity: runtime:PersistentMap.new runtime:PersistentMap.remove
// parity: runtime:Regex.captures runtime:Regex.compile runtime:Regex.find
// parity: runtime:Regex.is_match runtime:Regex.replace_all runtime:Regex.split
// parity: runtime:RegexError.message
// parity: runtime:Receiver.close runtime:Receiver.into_stream runtime:Receiver.recv
// parity: runtime:Receiver.recv_cancellable
// parity: runtime:Path.extension runtime:Path.file_name runtime:Path.from_string
// parity: runtime:Path.exists runtime:Path.is_absolute runtime:Path.is_dir runtime:Path.is_file
// parity: runtime:Path.join runtime:Path.list_files runtime:Path.list_paths
// parity: runtime:Path.normalize runtime:Path.parent runtime:Path.read_string
// parity: runtime:Path.resolve_relative runtime:Path.safe_relative
// parity: runtime:Path.starts_with runtime:Path.to_string runtime:Path.with_extension
// parity: runtime:Path.write_string
// parity: runtime:Pipeline.collect runtime:Pipeline.each runtime:Pipeline.filter
// parity: runtime:Pipeline.map runtime:Pipeline.try_map
// parity: runtime:String.safe_relative runtime:String.to_path runtime:Workspace.resolve
// parity: runtime:Process.run runtime:Process.run_async runtime:Process.run_many_stdout
// parity: runtime:Process.run_many_stdout_async runtime:Process.run_many_stdout_timeout
// parity: runtime:Process.run_many_stdout_timeout_async runtime:Process.run_request
// parity: runtime:Process.run_request_async runtime:Process.run_request_cancellable_async
// parity: runtime:Process.run_stdout runtime:Process.run_stdout_async runtime:Process.run_stdout_timeout
// parity: runtime:Process.run_stdout_timeout_async runtime:Process.run_timeout runtime:Process.run_timeout_async
// parity: runtime:Process.stream
// parity: runtime:Random.bool runtime:Random.bytes runtime:Random.float runtime:Random.int runtime:Random.string
// parity: runtime:Map.clear runtime:Map.contains_key runtime:Map.filter runtime:Map.fold runtime:Map.for_each
// parity: runtime:Map.get runtime:Map.get_or_default runtime:Map.insert runtime:Map.insert_old
// parity: runtime:Map.is_empty runtime:Map.keys runtime:Map.len runtime:Map.map_values
// parity: runtime:Map.merge runtime:Map.new runtime:Map.remove runtime:Map.try_fold runtime:Map.values
// parity: runtime:Float.is_finite runtime:Float.is_infinite runtime:Float.is_nan
// parity: runtime:Float.to_string runtime:Math.abs runtime:Math.abs_float runtime:Math.ceil
// parity: runtime:Math.clamp runtime:Math.clamp_float runtime:Math.floor runtime:Math.max
// parity: runtime:Math.max_float runtime:Math.min runtime:Math.min_float runtime:Math.pow
// parity: runtime:Math.pow_float runtime:Math.round runtime:Math.sqrt
// parity: runtime:Math.cos runtime:Math.exp runtime:Math.exp2 runtime:Math.log
// parity: runtime:Math.log2 runtime:Math.sin runtime:Math.tanh runtime:Math.trunc_float
// parity: runtime:Math.saturating_add runtime:Math.saturating_mul runtime:Math.saturating_sub
// parity: runtime:Math.wrapping_add runtime:Math.wrapping_mul runtime:Math.wrapping_sub
// parity: runtime:Option.and_then runtime:Option.filter runtime:Option.is_none
// parity: runtime:Option.is_some runtime:Option.map runtime:Option.ok_or
// parity: runtime:Option.or runtime:Option.unwrap_or runtime:Option.unwrap_or_else
// parity: runtime:Result.and_then runtime:Result.map runtime:Result.map_error
// parity: runtime:Ord.compare runtime:OS.close
// parity: runtime:Result.err runtime:Result.err_message runtime:Result.is_err
// parity: runtime:Result.is_ok runtime:Result.ok runtime:Result.unwrap_or runtime:Result.unwrap_or_else
// parity: runtime:Row.field_string runtime:RowBuffer.new
// parity: runtime:Set.clear runtime:Set.contains runtime:Set.difference runtime:Set.for_each
// parity: runtime:Set.insert runtime:Set.intersection runtime:Set.is_empty runtime:Set.is_subset
// parity: runtime:Set.len runtime:Set.new runtime:Set.remove runtime:Set.to_list runtime:Set.union
// parity: runtime:Sender.close runtime:Sender.send runtime:Sender.send_cancellable
// parity: runtime:SortedSet.clear runtime:SortedSet.contains runtime:SortedSet.insert
// parity: runtime:SortedSet.is_empty runtime:SortedSet.len runtime:SortedSet.new
// parity: runtime:SortedSet.remove runtime:SortedSet.to_list
// parity: runtime:SortedMap.clear runtime:SortedMap.contains_key runtime:SortedMap.get
// parity: runtime:SortedMap.insert runtime:SortedMap.is_empty runtime:SortedMap.keys
// parity: runtime:SortedMap.len runtime:SortedMap.new runtime:SortedMap.remove
// parity: runtime:SortedMap.values
// parity: runtime:String.after runtime:String.before runtime:String.char_at runtime:String.concat
// parity: runtime:String.chars
// parity: runtime:String.clone runtime:String.contains runtime:String.count runtime:String.copy runtime:String.ends_with
// parity: runtime:String.env runtime:String.env_or
// parity: runtime:String.format runtime:String.from_bool runtime:String.from_float runtime:String.from_int
// parity: runtime:String.index_of runtime:String.is_empty runtime:String.join runtime:String.len
// parity: runtime:String.lines runtime:String.pad_left runtime:String.pad_right
// parity: runtime:String.parse_float runtime:String.parse_int
// parity: runtime:String.repeat runtime:String.replace runtime:String.replace_first runtime:String.reverse
// parity: runtime:String.slice runtime:String.split
// parity: runtime:String.starts_with runtime:String.strip_prefix runtime:String.to_bytes
// parity: runtime:String.to_lowercase runtime:String.to_uppercase runtime:String.to_url runtime:String.trim
// parity: runtime:String.trim_end runtime:String.trim_start
// parity: runtime:String.view runtime:StringView.after runtime:StringView.before
// parity: runtime:StringView.contains runtime:StringView.is_empty runtime:StringView.len
// parity: runtime:StringView.slice runtime:StringView.starts_with runtime:StringView.to_string
// parity: runtime:StringBuilder.finish runtime:StringBuilder.new runtime:StringBuilder.push
// parity: runtime:Stream.collect_list runtime:Stream.from_list runtime:Stream.next
// parity: runtime:TempDir.keep runtime:TempDir.new runtime:TempDir.new_in runtime:TempDir.path
// parity: runtime:Timer.sleep runtime:Timer.sleep_cancellable runtime:Timer.sleep_until
// parity: runtime:Tcp.connect runtime:TcpError.message
// parity: runtime:TcpStream.read runtime:TcpStream.shutdown
// parity: runtime:TcpStream.write runtime:TcpStream.write_all
// parity: runtime:Toml.parse_file
// parity: runtime:Url.decode_component runtime:Url.encode_component runtime:Url.from_string runtime:Url.to_string
// parity: runtime:Uuid.new_v4
// parity: runtime:WebSocket.close runtime:WebSocket.connect runtime:WebSocket.recv_bytes
// parity: runtime:WebSocket.recv_text runtime:WebSocket.send_bytes runtime:WebSocket.send_text
// parity: runtime:WebSocketError.message
// parity: runtime:Yaml.parse runtime:Yaml.parse_file
