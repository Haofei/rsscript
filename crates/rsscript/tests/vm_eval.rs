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

use rsscript::{
    EvalError, NativeInterpreterFn, NativeRustDependency, NativeValue, eval_package_main_with_args,
    eval_source_main, eval_source_main_with_args, eval_source_main_with_args_and_native_bindings,
    lower_source_to_rust_package, lower_sources_to_rust_package_with_options,
    write_generated_rust_package,
};

#[test]
fn eval_runs_pure_arithmetic_main() {
    let source = r#"
fn main() -> Int {
    let x = 2
    let y = 3
    return x + y * 4
}
"#;

    let output = eval_source_main("eval-arithmetic.rss", source).expect("eval should succeed");

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

fn main() -> Int {
    let mut total = add(a: 1, b: 2)
    total = total + 4
    return total
}
"#;

    let output = eval_source_main("eval-function.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "7");
}

/// Build a program that folds a large `List<Float>` with `folder_body` and
/// returns the sum formatted as a string. The list values are deterministic so
/// the fast and slow folders must agree bit-for-bit.
fn float_fold_program(folder_body: &str) -> String {
    // Build the list once, then fold it many times so the fold (not the one-time
    // list construction) dominates the measured time.
    format!(
        r#"features: local

fn main() -> Float {{
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
    let fast0 = eval_source_main("float-fold-fast.rss", &fast_src).expect("fast eval");
    let slow0 = eval_source_main("float-fold-slow.rss", &slow_src).expect("slow eval");
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
        let r = eval_source_main("float-fold-fast.rss", &fast_src).expect("fast eval");
        fast_ns = fast_ns.min(t.elapsed().as_nanos());
        assert_eq!(r.value, fast0.value);

        let t = Instant::now();
        let r = eval_source_main("float-fold-slow.rss", &slow_src).expect("slow eval");
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
"#,
    )
    .expect("manifest should write");
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
fn main() -> Unit {
    let args = Args.all()
    let joined = List.join<String>(list: read args, separator: read "|")
    Log.write(message: read decorate(value: read joined))
    return Unit
}
"#,
    )
    .expect("main source should write");

    let output = eval_package_main_with_args(&package_dir, ["alpha", "beta"])
        .expect("package eval should run");

    assert_eq!(output.stdout, "alpha|beta\n");
    assert_eq!(output.value, "Unit");
    let _ = fs::remove_dir_all(package_dir);
}

#[test]
fn eval_runs_nested_pattern_match() {
    let source = r#"
fn main() -> String {
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

    let output = eval_source_main("eval-nested-match.rss", source).expect("eval should succeed");

    assert_eq!(output.value, "rss");
}

#[test]
fn eval_runs_random_int_runtime_intrinsic() {
    // `Random.int` is a register-VM built-in intrinsic: evaluation succeeds and
    // returns a value within the requested range.
    let source = r#"
fn main() -> Int {
    return Random.int(min: 0, max: 10)
}
"#;

    let output =
        eval_source_main("eval-random-int.rss", source).expect("Random.int should evaluate");

    let Some(NativeValue::Int(value)) = output.native_value else {
        panic!("expected an Int result, got {output:?}");
    };
    assert!(
        (0..=10).contains(&value),
        "Random.int returned {value}, outside [0, 10]"
    );
}

#[test]
fn parity_async_await_runs_synchronously() {
    let source = r#"
features: async, native

async fn add_after_sleep(value: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(value + 1)
}

async fn main() -> Result<Unit, TimerError> {
    let value = await add_after_sleep(value: 4)?
    Log.write(message: read String.from_int(value: value))

    let deadline = Deadline.after_ms(ms: 1)
    await Timer.sleep_until(deadline: read deadline)?

    let source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    await Timer.sleep_cancellable(ms: 1, token: read token)?
    Log.write(message: read "async-done")
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-async-await.rss",
        "rsscript_parity_async_await",
        source,
    );
}

#[test]
fn parity_select_runs_first_ready_arm() {
    let source = r#"
features: async, native

async fn after(value: Int, ms: Int) -> Result<Int, TimerError> {
    await Timer.sleep(ms: ms)?
    return Ok(value)
}

fn main() -> Result<Unit, TimerError> {
    select {
        value = await after(value: 7, ms: 1)? => {
            Log.write(message: read String.from_int(value: value))
        }
        other = await after(value: 9, ms: 100)? => {
            Log.write(message: read String.from_int(value: other))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend("parity-select.rss", "rsscript_parity_select", source);
}

#[test]
fn parity_task_group_async_let_runs_spawn_handles() {
    let source = r#"
features: async

async fn fetch_user() -> Result<String, String> {
    return Ok("user")
}

async fn fetch_profile() -> Result<String, String> {
    return Ok("profile")
}

fn main() -> Result<Unit, String> {
    task_group {
        async let user = fetch_user()
        async let profile = fetch_profile()

        let u = await user?
        let p = await profile?
        Log.write(message: read u)
        Log.write(message: read p)
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
fn parity_async_file_intrinsics() {
    // Write to a unique path under the OS temp dir, not a relative path: the
    // in-process VM runs with the repo root as CWD, so a relative file would
    // litter the working tree on every run. Cleaned up afterwards.
    let file = common::unique_temp_dir("rsscript-parity-async-file").with_extension("txt");
    let template = r#"
features: async, native, local

async fn main() -> Result<Unit, FileError> {
    let path = Path.from_string(value: read "ASYNC_FILE_PATH")
    await File.write_string_async(path: read path, text: read "hello async")?
    let text = await File.read_all_string_async(path: read path)?
    Log.write(message: read text)

    await File.write_async(path: read path, data: read Bytes.from_string(value: read "bytes"))?
    let bytes = await File.read_all_async(path: read path)?
    Log.write(message: read String.from_int(value: Bytes.len(value: read bytes)))
    return Ok(Unit)
}
"#;
    let source = template.replace("ASYNC_FILE_PATH", &file.to_string_lossy());
    common::assert_vm_eval_matches_backend(
        "parity-async-file.rss",
        "rsscript_parity_async_file",
        &source,
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn parity_async_process_intrinsics() {
    let source = r#"
features: async, native, local

async fn main() -> Result<Unit, String> {
    let output = await Process.run_async(command: read "printf", args: read ["ok"])?
    Log.write(message: read String.from_int(value: output.status))

    let stdout = await Process.run_stdout_async(command: read "printf", args: read ["stdout"])?
    Log.write(message: read stdout)

    let timeout_output = await Process.run_timeout_async(command: read "printf", args: read ["timeout"], timeout_ms: 1000)?
    Log.write(message: read String.from_int(value: timeout_output.status))

    let timeout_stdout = await Process.run_stdout_timeout_async(command: read "printf", args: read ["timeout-stdout"], timeout_ms: 1000)?
    Log.write(message: read timeout_stdout)

    let many = await Process.run_many_stdout_async(command: read "printf", args: read [], appended_args: read ["a", "b"], jobs: 2)?
    Log.write(message: read List.join<String>(list: read many, separator: read "|"))

    let many_timeout = await Process.run_many_stdout_timeout_async(command: read "printf", args: read [], appended_args: read ["c", "d"], jobs: 2, timeout_ms: 1000)?
    Log.write(message: read List.join<String>(list: read many_timeout, separator: read "|"))

    let request = ProcessRequest(
        command: "cat",
        args: List<String>.new(),
        cwd: None,
        stdin: Some("request-stdin"),
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 0,
    )
    let request_output = await Process.run_request_async(request: read request)?
    Log.write(message: read request_output.stdout)

    let cancellable_request = ProcessRequest(
        command: "printf",
        args: ["cancellable"],
        cwd: None,
        stdin: None,
        env: List<ProcessEnv>.new(),
        timeout_ms: 1000,
        merge_stderr: false,
        output_cap_bytes: 0,
    )
    let source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    let cancellable_output = await Process.run_request_cancellable_async(request: read cancellable_request, token: read token)?
    Log.write(message: read cancellable_output.stdout)
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-async-process.rss",
        "rsscript_parity_async_process",
        source,
    );
}

#[test]
fn eval_matches_lowered_rust_for_pure_core_example() {
    let source_path = "examples/scripts/core/interpreter_pure_parity.rss";
    let source = fs::read_to_string(common::workspace_root().join(source_path))
        .expect("parity fixture should be readable");
    let eval = eval_source_main(source_path, &source).expect("eval should succeed");
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
fn main() -> Unit {
    let len = String.len(value: read "é")
    Log.write(message: read String.from_int(value: len))
}
"#;
    let eval = eval_source_main("eval-string-len-utf8.rss", source).expect("eval should succeed");
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
fn eval_matches_backend_for_declared_host_boundary() {
    let source_path = "examples/scripts/core/interpreter_host_boundary.rss";
    let source = fs::read_to_string(common::workspace_root().join(source_path))
        .expect("host boundary fixture should be readable");
    let cwd = std::env::current_dir().expect("current dir should be readable");
    let eval =
        eval_source_main(source_path, &source).expect("eval should run host boundary fixture");
    std::env::set_current_dir(&cwd).expect("current dir should be restored after eval");
    assert_eq!(eval.stdout, "host-ok\n");
    assert_eq!(eval.stderr, "");

    let runtime_path = common::runtime_path();
    let package = lower_source_to_rust_package(
        source_path,
        &source,
        "rsscript_eval_host_boundary",
        &runtime_path,
    )
    .expect("host boundary fixture should lower");
    let package_dir = common::unique_temp_dir("rsscript-eval-host-boundary");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
        .output()
        .expect("generated Rust package should run");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);

    assert!(
        output.status.success(),
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "host-ok\n");
    assert_eq!(stderr, "");
}

#[test]
fn eval_dispatches_native_host_bindings() {
    fn host_echo(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::String(message)] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("host:{message}")))
    }

    fn host_tag(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Int(value)] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("tag:{value}")))
    }

    let source = r#"
features: native

native fn Host.echo(message: read String) -> String
    effects(native)

native fn Host.tag(value: Int) -> String
    effects(native)

fn main() -> Unit {
    Log.write(message: read Host.echo(message: read "hello"))
    Log.write(message: read Host.tag(value: 7))
    return Unit
}
"#;

    let eval = eval_source_main_with_args_and_native_bindings(
        "eval-native-host.rss",
        source,
        std::iter::empty::<&str>(),
        [
            ("Host.echo", host_echo as NativeInterpreterFn),
            ("Host.tag", host_tag as NativeInterpreterFn),
        ],
    )
    .expect("native host binding eval should succeed");

    assert_eq!(eval.value, "Unit");
    assert_eq!(eval.stdout, "host:hello\ntag:7\n");
    assert_eq!(eval.stderr, "");
}

#[test]
fn eval_reports_unbound_native_declarations() {
    let source = r#"
features: native

native fn Host.echo(message: read String) -> String
    effects(native)

fn main() -> Unit {
    Log.write(message: read Host.echo(message: read "hello"))
    return Unit
}
"#;

    let error = eval_source_main("eval-unbound-native.rss", source)
        .expect_err("unbound native declaration should fail");

    assert!(
        matches!(error, EvalError::Runtime(ref message) if message.contains("Host.echo") && message.contains("no host binding")),
        "{error:?}"
    );
}

#[test]
fn eval_receiver_native_bindings_use_resolved_receiver_namespace() {
    fn alpha_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "Alpha".to_string(),
            id: 1,
        })
    }

    fn beta_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "Beta".to_string(),
            id: 2,
        })
    }

    fn alpha_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("alpha:{type_name}:{id}")))
    }

    fn beta_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("beta:{type_name}:{id}")))
    }

    let source = r#"
features: native

opaque struct Alpha
opaque struct Beta

native fn Alpha.open() -> Alpha
    effects(native)

native fn Alpha.describe(self: read Alpha) -> String
    effects(native)

native fn Beta.open() -> Beta
    effects(native)

native fn Beta.describe(self: read Beta) -> String
    effects(native)

fn main() -> Unit {
    let alpha = Alpha.open()
    let beta = Beta.open()
    Log.write(message: read alpha.describe())
    Log.write(message: read beta.describe())
    return Unit
}
"#;

    let output = eval_source_main_with_args_and_native_bindings(
        "receiver-native-bindings.rss",
        source,
        std::iter::empty::<&str>(),
        [
            ("Alpha.open", alpha_open as NativeInterpreterFn),
            ("Alpha.describe", alpha_describe as NativeInterpreterFn),
            ("Beta.open", beta_open as NativeInterpreterFn),
            ("Beta.describe", beta_describe as NativeInterpreterFn),
        ],
    )
    .expect("receiver native host binding eval should succeed");

    assert_eq!(output.stdout, "alpha:Alpha:1\nbeta:Beta:2\n");
}

#[test]
fn parity_native_host_bindings_match_lowered_backend() {
    fn host_open(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::Native {
            type_name: "HostHandle".to_string(),
            id: 7,
        })
    }

    fn host_describe(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::Native { type_name, id }] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("{type_name}:{id}")))
    }

    fn host_echo(args: Vec<NativeValue>) -> Result<NativeValue, String> {
        let [NativeValue::String(message)] = args.as_slice() else {
            return Err(format!("unexpected args: {args:?}"));
        };
        Ok(NativeValue::String(format!("host:{message}")))
    }

    let source = r#"
features: native

opaque struct HostHandle

native fn Host.open() -> HostHandle
    effects(native)

native fn Host.describe(handle: read HostHandle) -> String
    effects(native)

native fn Host.echo(message: read String) -> String
    effects(native)

fn main() -> Unit {
    let handle = Host.open()
    Log.write(message: read Host.describe(handle: read handle))
    Log.write(message: read Host.echo(message: read "native"))
    return Unit
}
"#;

    let eval = eval_source_main_with_args_and_native_bindings(
        "parity-native-host.rss",
        source,
        std::iter::empty::<&str>(),
        [
            ("Host.open", host_open as NativeInterpreterFn),
            ("Host.describe", host_describe as NativeInterpreterFn),
            ("Host.echo", host_echo as NativeInterpreterFn),
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
        &[],
        &[NativeRustDependency {
            crate_name: "rsscript_test_native".to_string(),
            path: native_dir.to_string_lossy().to_string(),
            cargo_features: Vec::new(),
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
// parity: runtime:Args.all runtime:Args.count runtime:Args.get runtime:Args.get_or_default
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
// parity: runtime:Db.close runtime:DbConnection.open runtime:DbConnection.query
// parity: runtime:DbConnection.try_open
// parity: runtime:Directory.create runtime:Directory.create_all runtime:Directory.create_dir_all
// parity: runtime:Directory.exists runtime:Directory.is_dir runtime:Directory.is_file
// parity: runtime:Directory.list_files runtime:Directory.list_paths runtime:Directory.metadata
// parity: runtime:Directory.copy_file runtime:Directory.rename runtime:Directory.remove_file
// parity: runtime:Directory.remove_dir_all runtime:Directory.read_string runtime:Directory.write_string
// parity: runtime:Duration.add runtime:Duration.as_ms runtime:Duration.as_seconds
// parity: runtime:Duration.ms runtime:Duration.seconds
// parity: runtime:Environment.bind_function runtime:Environment.child
// parity: runtime:Environment.has_function runtime:Environment.has_parent runtime:Environment.root
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
// parity: runtime:Cache.get runtime:Cache.insert runtime:Cache.lookup runtime:Cache.new
// parity: runtime:Channel.bounded runtime:Channel.message runtime:Channel.receiver runtime:Channel.sender
// parity: runtime:ChannelError.message
// parity: runtime:CancellationSource.cancel runtime:CancellationSource.new
// parity: runtime:CancellationSource.token runtime:CancellationToken.is_cancelled
// parity: runtime:Clone.clone
// parity: runtime:Clock.now runtime:Clock.system_unix_ms
// parity: runtime:Config.load runtime:Config.name runtime:Config.new runtime:Config.rule_count
// parity: runtime:ConfigStore.name runtime:ConfigStore.new runtime:ConfigStore.replace
// parity: runtime:Counter.add runtime:Counter.new runtime:Counter.value
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
// parity: runtime:Image.inspect runtime:Image.load runtime:Image.normalize runtime:Image.resize
// parity: runtime:Image.save runtime:Image.sharpen
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
// parity: runtime:Log.error runtime:Log.error_json runtime:Log.trace runtime:Log.write
// parity: runtime:Log.write_json
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
// parity: runtime:FunctionObject.has_closure runtime:FunctionObject.new
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
// parity: runtime:Option.and_then runtime:Option.filter runtime:Option.is_none
// parity: runtime:Option.is_some runtime:Option.map runtime:Option.ok_or
// parity: runtime:Option.or runtime:Option.unwrap_or runtime:Option.unwrap_or_else
// parity: runtime:Result.and_then runtime:Result.map runtime:Result.map_error
// parity: runtime:Ord.compare runtime:OS.close
// parity: runtime:Request.new runtime:Request.path
// parity: runtime:Response.body runtime:Response.ok runtime:Response.status
// parity: runtime:ResourcePool.discard runtime:ResourcePool.stats
// parity: runtime:Result.err runtime:Result.err_message runtime:Result.is_err
// parity: runtime:Result.is_ok runtime:Result.ok runtime:Result.unwrap_or runtime:Result.unwrap_or_else
// parity: runtime:Row.field_string runtime:RowBuffer.new
// parity: runtime:RuleLoader.load_rules
// parity: runtime:PoolError.message
// parity: runtime:PoolStats.available runtime:PoolStats.capacity
// parity: runtime:PoolStats.created runtime:PoolStats.in_use
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
// parity: runtime:GlobalConfig.new runtime:GlobalConfig.replace runtime:GlobalConfig.rule_count
// parity: runtime:TempDir.keep runtime:TempDir.new runtime:TempDir.new_in runtime:TempDir.path
// parity: runtime:Tensor.from_f32_slice runtime:Tensor.to_f32_slice runtime:Tensor.shape
// parity: runtime:Tensor.rank runtime:Tensor.matmul runtime:TensorError.message
// parity: runtime:Tensor.add runtime:Tensor.sub runtime:Tensor.mul runtime:Tensor.div
// parity: runtime:Tensor.neg runtime:Tensor.exp runtime:Tensor.log runtime:Tensor.sqrt runtime:Tensor.relu
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
