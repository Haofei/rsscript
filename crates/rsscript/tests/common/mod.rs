#![allow(dead_code)]

pub mod differential;

use base64::Engine;
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript::{
    EvalError, EvalOutput, RegVmExecutable, Severity, analyze_source_with_interfaces_result,
    lower_source_to_rust_package_with_interfaces, reg_vm_compile_validated,
    write_generated_rust_package,
};

pub const FULL_BACKEND_PARITY_ENV: &str = "RSSCRIPT_FULL_BACKEND_PARITY";

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Host contracts used explicitly by cross-backend fixtures. They are test
/// dependencies, not part of the language's default interface surface.
pub const TEST_HOST_INTERFACES: &[(&str, &str)] = &[
    (
        "async/cancellation.rssi",
        include_str!("../../../../packages/async/interface/cancellation.rssi"),
    ),
    (
        "async/channel.rssi",
        include_str!("../../../../packages/async/interface/channel.rssi"),
    ),
    (
        "async/stream.rssi",
        include_str!("../../../../packages/async/interface/stream.rssi"),
    ),
    (
        "async/task.rssi",
        include_str!("../../../../packages/async/interface/task.rssi"),
    ),
    (
        "host/async-csv.rssi",
        include_str!("../../../../packages/async/interface/csv.rssi"),
    ),
    (
        "host/deadline.rssi",
        include_str!("../../../../packages/async/interface/deadline.rssi"),
    ),
    (
        "host/async-file.rssi",
        include_str!("../../../../packages/async/interface/file.rssi"),
    ),
    (
        "host/async-http.rssi",
        include_str!("../../../../packages/async/interface/http.rssi"),
    ),
    (
        "host/async-process.rssi",
        include_str!("../../../../packages/async/interface/process.rssi"),
    ),
    (
        "host/tcp.rssi",
        include_str!("../../../../packages/async/interface/tcp.rssi"),
    ),
    (
        "host/timer.rssi",
        include_str!("../../../../packages/async/interface/timer.rssi"),
    ),
    (
        "host/websocket.rssi",
        include_str!("../../../../packages/async/interface/websocket.rssi"),
    ),
    (
        "host/args.rssi",
        include_str!("../../../../stdlib/os/os.rssi"),
    ),
    (
        "host/clock.rssi",
        include_str!("../../../../stdlib/clock/clock.rssi"),
    ),
    (
        "host/env.rssi",
        include_str!("../../../../stdlib/env/env.rssi"),
    ),
    (
        "host/directory.rssi",
        include_str!("../../../../stdlib/fs/directory.rssi"),
    ),
    (
        "host/file.rssi",
        include_str!("../../../../stdlib/fs/file.rssi"),
    ),
    (
        "host/http.rssi",
        include_str!("../../../../stdlib/http/client.rssi"),
    ),
    (
        "host/log.rssi",
        include_str!("../../../../stdlib/log/log.rssi"),
    ),
    (
        "host/path.rssi",
        include_str!("../../../../stdlib/path/path.rssi"),
    ),
    (
        "host/process.rssi",
        include_str!("../../../../stdlib/process/process.rssi"),
    ),
    (
        "host/random.rssi",
        include_str!("../../../../stdlib/random/random.rssi"),
    ),
    (
        "host/tempdir.rssi",
        include_str!("../../../../stdlib/tempdir/tempdir.rssi"),
    ),
    (
        "host/workspace.rssi",
        include_str!("../../../../stdlib/workspace/workspace.rssi"),
    ),
];

pub fn full_backend_parity_enabled() -> bool {
    std::env::var(FULL_BACKEND_PARITY_ENV)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
}

pub fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn workspace_root() -> PathBuf {
    crate_root().join("../..")
}

/// Locate the language spec under `docs/` by its stable, version-independent
/// name shape `RSScript_v<version>_Spec.md`, so tests never hard-code the spec
/// version. Panics if zero or more than one match is found.
pub fn language_spec_path() -> PathBuf {
    find_versioned_doc("RSScript_v", "_Spec.md")
}

/// Locate the current package reference.
pub fn package_reference_path() -> PathBuf {
    workspace_root().join("docs/package.md")
}

/// Find exactly one file in `docs/` whose name starts with `prefix` and ends
/// with `suffix` (the span between them is the version and is not constrained).
fn find_versioned_doc(prefix: &str, suffix: &str) -> PathBuf {
    let docs = workspace_root().join("docs").join("spec");
    let mut matches: Vec<PathBuf> = fs::read_dir(&docs)
        .unwrap_or_else(|error| panic!("docs/ should be readable: {error}"))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .collect();
    matches.sort();
    match matches.as_slice() {
        [path] => path.clone(),
        [] => panic!("no docs/ file matched `{prefix}*{suffix}`"),
        many => {
            panic!("expected exactly one docs/ file matching `{prefix}*{suffix}`, found {many:?}")
        }
    }
}

pub fn runtime_path() -> String {
    workspace_root()
        .join("crates/runtime")
        .display()
        .to_string()
}

pub fn generated_target_dir() -> PathBuf {
    // A SINGLE shared target dir across every generated-package build. Each test
    // writes its package into its own temp dir (so sources never collide), but the
    // build output is shared: the heavy `rsscript-runtime` dependency tree
    // (tokio, reqwest, flate2, sha3, …) is then compiled *once* and reused by
    // every test instead of being rebuilt from scratch per nextest process.
    // Cargo's build lock serializes concurrent builds into this dir safely, and
    // after the first warm-up each test only recompiles its own tiny crate.
    workspace_root().join("target/rsscript-generated-test")
}

// ---------------------------------------------------------------------------
// Differential execution engine shared by the corpus framework and unit tests.
// `run_vm_source` evaluates a program on the register VM; `run_compiled_source`
// lowers it to Rust, builds, and runs it (cached). Asserting the two agree is
// the backbone of the framework.
// ---------------------------------------------------------------------------

/// Evaluate a single-file program's `main` on the register VM.
pub fn run_vm_source(file: &str, source: &str, args: &[&str]) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args(args.iter().copied())
}

pub fn compile_vm_source(file: &str, source: &str) -> Result<RegVmExecutable, EvalError> {
    compile_vm_source_with_interfaces(file, source, &[])
}

pub fn compile_vm_source_with_interfaces(
    file: &str,
    source: &str,
    extra_interfaces: &[(&str, &str)],
) -> Result<RegVmExecutable, EvalError> {
    let mut interfaces = TEST_HOST_INTERFACES.to_vec();
    interfaces.extend_from_slice(extra_interfaces);
    let validated = analyze_source_with_interfaces_result(file, source, &interfaces)
        .into_validated()
        .map_err(EvalError::Diagnostics)?;
    reg_vm_compile_validated(&validated)
}

pub fn reg_vm_eval_source_main(file: &str, source: &str) -> Result<EvalOutput, EvalError> {
    run_vm_source(file, source, &[])
}

pub fn reg_vm_eval_source_main_with_args(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args(args)
}

pub fn reg_vm_eval_source_main_with_limits(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    limits: rsscript::VmLimits,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_limits(args, limits)
}

pub fn reg_vm_eval_source_main_with_args_and_external_bindings(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    external_bindings: impl IntoIterator<Item = (impl Into<String>, rsscript::ExternalFunction)>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?
        .eval_main_with_args_and_external_bindings(args, external_bindings)
}

pub fn reg_vm_eval_source_main_with_interfaces_and_external_bindings(
    file: &str,
    source: &str,
    extra_interfaces: &[(&str, &str)],
    external_bindings: impl IntoIterator<Item = (impl Into<String>, rsscript::ExternalFunction)>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source_with_interfaces(file, source, extra_interfaces)?
        .eval_main_with_args_and_external_bindings(std::iter::empty::<String>(), external_bindings)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args_native(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_force_all_safepoints(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args_native_force_all_safepoints(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_precise(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args_native_precise(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_osr(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args_native_osr(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_osr_report(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<(EvalOutput, rsscript::NativeStats, Vec<String>), EvalError> {
    compile_vm_source(file, source)?.eval_main_with_args_native_osr_report(args)
}

pub fn lower_test_source_to_rust_package(
    file: &str,
    source: &str,
    package_name: &str,
    runtime_path: &str,
) -> Result<rsscript::GeneratedRustPackage, Vec<rsscript::Diagnostic>> {
    let interfaces = TEST_HOST_INTERFACES
        .iter()
        .filter(|(path, _)| {
            !matches!(
                *path,
                "async/cancellation.rssi"
                    | "async/channel.rssi"
                    | "async/stream.rssi"
                    | "async/task.rssi"
                    | "host/async-csv.rssi"
            )
        })
        .map(|(path, contents)| ((*path).to_string(), (*contents).to_string()))
        .collect::<Vec<_>>();
    lower_source_to_rust_package_with_interfaces(
        file,
        source,
        package_name,
        runtime_path,
        &interfaces,
    )
}

/// Error-severity diagnostic codes (sorted, unique) the checker reports for a
/// single-file program.
pub fn error_codes(file: &str, source: &str) -> Vec<String> {
    let mut codes = analyze_source_with_interfaces_result(file, source, TEST_HOST_INTERFACES)
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code.clone())
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

/// Lower a single-file program to Rust, build, and run it, returning
/// `(stdout, stderr)`. Results are cached on disk keyed by the source + args, so
/// repeated runs across the suite don't rebuild. Panics if the program fails to
/// lower or run.
pub fn run_compiled_source(file: &str, source: &str, args: &[&str]) -> (String, String) {
    try_run_compiled_source(file, source, args)
        .unwrap_or_else(|error| panic!("compiled backend for `{file}` failed: {error}"))
}

/// Like [`run_compiled_source`] but returns `Err` (instead of panicking) when the
/// generated program fails at runtime — for failure-path differential tests.
/// Successful runs are cached; failures recompile (they are rare).
pub fn try_run_compiled_source(
    file: &str,
    source: &str,
    args: &[&str],
) -> Result<(String, String), String> {
    let cache_dir = workspace_root().join("target/rsscript-corpus-compiled-cache");
    fs::create_dir_all(&cache_dir).expect("compiled cache dir should create");
    let key = compiled_cache_key(file, source, args);
    let stdout_path = cache_dir.join(format!("{key}.stdout"));
    let stderr_path = cache_dir.join(format!("{key}.stderr"));
    if let (Ok(stdout), Ok(stderr)) = (
        fs::read_to_string(&stdout_path),
        fs::read_to_string(&stderr_path),
    ) {
        return Ok((stdout, stderr));
    }
    let (stdout, stderr) = compile_and_run(file, source, args, &key)?;
    let _ = fs::write(&stdout_path, &stdout);
    let _ = fs::write(&stderr_path, &stderr);
    Ok((stdout, stderr))
}

fn compiled_cache_key(file: &str, source: &str, args: &[&str]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(b"rsscript-corpus-compiled-cache-v4\0");
    hasher.update(env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT").as_bytes());
    hasher.update([0]);
    hasher.update(file.as_bytes());
    hasher.update([0]);
    hasher.update(source.as_bytes());
    for arg in args {
        hasher.update([0]);
        hasher.update(arg.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn compile_and_run(
    file: &str,
    source: &str,
    args: &[&str],
    cache_key: &str,
) -> Result<(String, String), String> {
    let runtime_path = runtime_path();
    let package_name = format!(
        "rsscript_{}_{}",
        file.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
            .trim_matches('_'),
        &cache_key[..12],
    );
    let interfaces = TEST_HOST_INTERFACES
        .iter()
        .filter(|(path, _)| {
            !matches!(
                *path,
                "async/cancellation.rssi"
                    | "async/channel.rssi"
                    | "async/stream.rssi"
                    | "async/task.rssi"
                    | "host/async-csv.rssi"
            )
        })
        .map(|(path, contents)| ((*path).to_string(), (*contents).to_string()))
        .collect::<Vec<_>>();
    let package = lower_source_to_rust_package_with_interfaces(
        file,
        source,
        &package_name,
        &runtime_path,
        &interfaces,
    )
    .expect("source should lower to a Rust package");
    let package_dir = unique_temp_dir("rsscript-corpus-compiled");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", generated_target_dir())
        .env("RUSTFLAGS", "-Awarnings")
        // Build the generated crate offline. Every dependency (`rsscript-runtime`
        // and its transitive deps such as `imbl`/`archery`) is already vendored
        // in the shared cargo registry by the workspace build, so no download is
        // ever needed. Without this, `cargo` may contact crates.io to refresh the
        // index and a transient network error (e.g. an HTTP/2 framing failure on
        // a crate download) makes `cargo run` exit non-zero — which the
        // differential harness would otherwise misreport as a `rust-compiled`
        // backend divergence. Offline removes the network as a flake source; a
        // genuinely missing dependency now surfaces as a deterministic error.
        .env("CARGO_NET_OFFLINE", "true");
    if !args.is_empty() {
        command.arg("--").args(args);
    }
    let output = command.output().expect("generated Rust package should run");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);
    if output.status.success() {
        Ok((stdout, stderr))
    } else {
        // A runtime failure of the generated program (e.g. a panic on an
        // out-of-bounds access). Reported as `Err` so failure-path differential
        // tests can assert every backend fails.
        Err(format!(
            "compiled backend for `{file}` exited with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        ))
    }
}

pub fn fixture_paths(directory: &str) -> Vec<PathBuf> {
    let directory_path = fixture_root(directory);
    let mut paths: Vec<PathBuf> = fs::read_dir(&directory_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory_path.display()))
        .map(|entry| entry.expect("fixture entry should be readable").path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
        })
        .collect();
    paths.sort();
    paths
}

pub fn recursive_fixture_paths(directory: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_fixture_paths(&fixture_root(directory), &mut paths);
    paths.sort();
    paths
}

pub fn recursive_paths_with_extension(directory: &str, extension: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_paths_with_extension(&fixture_root(directory), extension, &mut paths);
    paths.sort();
    paths
}

fn fixture_root(directory: &str) -> PathBuf {
    let path = PathBuf::from(directory);
    if path.exists() {
        path
    } else {
        workspace_root().join(directory)
    }
}

fn collect_paths_with_extension(directory: &Path, extension: &str, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory:?}: {error}"))
    {
        let path = entry.expect("fixture entry should be readable").path();
        if path.is_dir() {
            collect_paths_with_extension(&path, extension, paths);
        } else if path
            .extension()
            .and_then(|path_extension| path_extension.to_str())
            == Some(extension)
        {
            paths.push(path);
        }
    }
}

fn collect_fixture_paths(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory:?}: {error}"))
    {
        let path = entry.expect("fixture entry should be readable").path();
        if path.is_dir() {
            collect_fixture_paths(&path, paths);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
        {
            paths.push(path);
        }
    }
}

pub fn read_fixture(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

pub fn toml_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::var_os("RSSCRIPT_TEMP_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("{prefix}-{}-{nanos}-{counter}", std::process::id()))
}

pub fn write_package_fixture(
    directory: &Path,
    version: &str,
    extra_manifest: &str,
    interface_source: &str,
) {
    write_named_package_fixture(
        directory,
        "rss-json",
        version,
        extra_manifest,
        interface_source,
    );
}

pub fn write_named_package_fixture(
    directory: &Path,
    name: &str,
    version: &str,
    extra_manifest: &str,
    interface_source: &str,
) {
    fs::create_dir_all(directory.join("interface")).expect("interface dir should be created");
    fs::write(
        directory.join("rsspkg.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "{version}"
edition = "2026"

[interfaces]
paths = ["interface"]

{extra_manifest}
"#
        ),
    )
    .expect("package manifest should be written");
    fs::write(directory.join("interface/lib.rssi"), interface_source)
        .expect("interface should be written");
}

pub fn expected_codes(source: &str) -> Vec<String> {
    let first_line = source.lines().next().unwrap_or_default();
    let Some(codes) = first_line.strip_prefix("// expect:") else {
        panic!("fail fixture must start with `// expect:`");
    };
    codes.split_whitespace().map(str::to_string).collect()
}

pub fn fail_fixture_expected_code_set() -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for path in fixture_paths("tests/fixtures/fail") {
        let source = read_fixture(&path);
        for code in expected_codes(&source) {
            codes.insert(code);
        }
    }
    codes
}

pub fn source_map_summary(entries: &[rsscript::RustSourceMapEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{} {}:{}:{} -> {}:{}:{}\n",
                entry.kind,
                entry.source.file,
                entry.source.line,
                entry.source.column,
                entry.generated.file,
                entry.generated.line,
                entry.generated.column
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VM/backend parity helpers + network test servers, shared by the split
// vm_eval_*.rs files. Generated-Rust parity is opt-in for broad sweeps because
// each unique program becomes a Cargo build.
// ---------------------------------------------------------------------------

pub fn assert_vm_eval_matches_backend(name: &str, package: &str, source: &str) {
    assert_vm_eval_matches_backend_with_args(name, package, source, &[]);
}

pub fn assert_vm_eval_matches_compiled_backend(name: &str, package: &str, source: &str) {
    assert_vm_eval_matches_backend_internal(name, package, source, &[], &[], false, true);
}

pub fn assert_vm_eval_matches_compiled_backend_with_distinct_args(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
) {
    assert_vm_eval_matches_backend_internal(
        name,
        package,
        source,
        interpreter_args,
        backend_args,
        false,
        true,
    );
}

pub fn run_with_large_stack(test: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(test)
        .expect("large-stack test thread should spawn")
        .join()
        .expect("large-stack test thread should pass");
}

pub fn spawn_tcp_echo_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
    let port = listener
        .local_addr()
        .expect("test listener should have an address")
        .port()
        .to_string();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("client should connect");
        let mut buffer = [0; 4];
        socket
            .read_exact(&mut buffer)
            .expect("server should read ping");
        assert_eq!(&buffer, b"ping");
        socket.write_all(b"pong").expect("server should write pong");
    });
    (port, handle)
}

pub fn spawn_http_response_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
    let port = listener
        .local_addr()
        .expect("test listener should have an address")
        .port()
        .to_string();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("client should connect");
        let mut request = [0; 1024];
        let read = socket
            .read(&mut request)
            .expect("server should read request");
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("GET /health HTTP/1.1"), "{request}");
        socket
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 11\r\n\r\nalpha\nbeta\n")
            .expect("server should write response");
    });
    (port, handle)
}

pub struct TestWebSocketFrame {
    pub opcode: u8,
    pub payload: Vec<u8>,
}

pub fn spawn_websocket_echo_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
    let port = listener
        .local_addr()
        .expect("test listener should have an address")
        .port()
        .to_string();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("client should connect");
        websocket_accept_handshake(&mut socket);

        let text = websocket_read_test_frame(&mut socket);
        assert_eq!(text.opcode, 0x1);
        assert_eq!(
            String::from_utf8(text.payload).expect("text should be UTF-8"),
            "ping"
        );
        websocket_write_test_frame(&mut socket, 0x1, b"pong");

        let bytes = websocket_read_test_frame(&mut socket);
        assert_eq!(bytes.opcode, 0x2);
        assert_eq!(bytes.payload, b"bin");
        websocket_write_test_frame(&mut socket, 0x2, &[1, 2, 3, 4]);

        let close = websocket_read_test_frame(&mut socket);
        assert_eq!(close.opcode, 0x8);
        websocket_write_test_frame(&mut socket, 0x8, &[]);
    });
    (port, handle)
}

pub fn websocket_accept_handshake(socket: &mut std::net::TcpStream) {
    let mut request = Vec::new();
    loop {
        let mut byte = [0; 1];
        socket
            .read_exact(&mut byte)
            .expect("server should read websocket handshake");
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let request = String::from_utf8_lossy(&request);
    let key = request
        .lines()
        .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
        .expect("websocket key should be present")
        .trim();
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    socket
        .write_all(response.as_bytes())
        .expect("server should write websocket handshake");
}

pub fn websocket_read_test_frame(socket: &mut std::net::TcpStream) -> TestWebSocketFrame {
    let mut header = [0; 2];
    socket
        .read_exact(&mut header)
        .expect("server should read websocket frame header");
    let opcode = header[0] & 0x0F;
    let masked = header[1] & 0x80 != 0;
    let mut len = u64::from(header[1] & 0x7F);
    if len == 126 {
        let mut bytes = [0; 2];
        socket
            .read_exact(&mut bytes)
            .expect("server should read websocket frame length");
        len = u64::from(u16::from_be_bytes(bytes));
    } else if len == 127 {
        let mut bytes = [0; 8];
        socket
            .read_exact(&mut bytes)
            .expect("server should read websocket frame length");
        len = u64::from_be_bytes(bytes);
    }
    let mut mask = [0; 4];
    if masked {
        socket
            .read_exact(&mut mask)
            .expect("server should read websocket frame mask");
    }
    let mut payload = vec![0; len as usize];
    socket
        .read_exact(&mut payload)
        .expect("server should read websocket frame payload");
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % mask.len()];
        }
    }
    TestWebSocketFrame { opcode, payload }
}

pub fn websocket_write_test_frame(socket: &mut std::net::TcpStream, opcode: u8, payload: &[u8]) {
    let mut frame = Vec::new();
    frame.push(0x80 | (opcode & 0x0F));
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if u16::try_from(payload.len()).is_ok() {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    socket
        .write_all(&frame)
        .expect("server should write websocket frame");
}

pub fn assert_vm_eval_matches_backend_with_args(
    name: &str,
    package: &str,
    source: &str,
    args: &[&str],
) {
    assert_vm_eval_matches_backend_with_distinct_args(name, package, source, args, args);
}

pub fn assert_vm_eval_matches_backend_with_distinct_args(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
) {
    assert_vm_eval_matches_backend_internal(
        name,
        package,
        source,
        interpreter_args,
        backend_args,
        false,
        false,
    );
}

pub fn assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
) {
    assert_vm_eval_matches_backend_internal(
        name,
        package,
        source,
        interpreter_args,
        backend_args,
        true,
        false,
    );
}

pub fn assert_vm_eval_matches_backend_internal(
    name: &str,
    package: &str,
    source: &str,
    interpreter_args: &[&str],
    backend_args: &[&str],
    allow_unused_mut_warning: bool,
    force_compiled_backend: bool,
) {
    let eval = reg_vm_eval_source_main_with_args(name, source, interpreter_args.iter().copied())
        .unwrap_or_else(|error| panic!("interpreter eval failed for {name}: {error:?}"));

    // Tier-0 JIT must match the interpreter exactly. Skip programs that do
    // (non-idempotent) network I/O — running them an extra time would exhaust the
    // test server's connections, and their ops aren't JIT-eligible anyway. Pure
    // programs are verified three-way here; the rest by tests/backend_differential.rs.
    let does_network_io = source.contains("Tcp")
        || source.contains("WebSocket")
        || source.contains("HttpResponse")
        || source.contains("HttpRequest");
    if !does_network_io {
        let eval_jit = compile_vm_source(name, source)
            .and_then(|executable| {
                executable.eval_main_with_args_jit(interpreter_args.iter().copied())
            })
            .unwrap_or_else(|error| panic!("jit eval failed for {name}: {error:?}"));
        assert_eq!(
            eval_jit.stdout, eval.stdout,
            "JIT vs interpreter stdout divergence for {name}"
        );
        assert_eq!(
            eval_jit.display_value, eval.display_value,
            "JIT vs interpreter value divergence for {name}"
        );

        // Native (Cranelift) tier, when enabled: the integer/control core runs as
        // machine code; everything else falls back. Must also match exactly.
        #[cfg(feature = "native-jit")]
        {
            let eval_native =
                reg_vm_eval_source_main_native(name, source, interpreter_args.iter().copied())
                    .unwrap_or_else(|error| panic!("native jit eval failed for {name}: {error:?}"));
            assert_eq!(
                eval_native.stdout, eval.stdout,
                "native JIT vs interpreter stdout divergence for {name}"
            );
            assert_eq!(
                eval_native.display_value, eval.display_value,
                "native JIT vs interpreter value divergence for {name}"
            );
        }
    }

    if !does_network_io && !force_compiled_backend && !full_backend_parity_enabled() {
        return;
    }

    // Cache the compiled backend's (stdout, stderr) keyed by source + args, so
    // reruns skip the (dominant) generated-crate rebuild. Same approach as vm.rs.
    let cache_dir = workspace_root().join("target/rsscript-corpus-compiled-cache");
    let _ = fs::create_dir_all(&cache_dir);
    let key = compiled_cache_key(name, source, backend_args);
    let stdout_path = cache_dir.join(format!("{key}.stdout"));
    let stderr_path = cache_dir.join(format!("{key}.stderr"));
    let (stdout, stderr) = if let (Ok(out), Ok(err)) = (
        fs::read_to_string(&stdout_path),
        fs::read_to_string(&stderr_path),
    ) {
        (out, err)
    } else {
        let runtime_path = runtime_path();
        let lowered = lower_test_source_to_rust_package(name, source, package, &runtime_path)
            .expect("parity fixture should lower");
        let package_dir = unique_temp_dir(package);
        write_generated_rust_package(&package_dir, &lowered)
            .expect("generated package should write");
        let output = Command::new("cargo")
            .arg("run")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(package_dir.join("Cargo.toml"))
            .arg("--")
            .args(backend_args)
            .env("CARGO_TARGET_DIR", generated_target_dir())
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .expect("generated Rust package should run");
        let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
        let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
        let _ = fs::remove_dir_all(&package_dir);
        assert!(
            output.status.success(),
            "backend run failed for {name}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let _ = fs::write(&stdout_path, &stdout);
        let _ = fs::write(&stderr_path, &stderr);
        (stdout, stderr)
    };

    assert_eq!(stdout, eval.stdout, "stdout divergence for {name}");
    if allow_unused_mut_warning && eval.stderr.is_empty() && stderr.contains("unused_mut") {
        assert!(
            stderr.lines().all(|line| line.is_empty()
                || line.contains("variable does not need to be mutable")
                || line.contains("-->")
                || line.contains("let mut ")
                || line.contains("----")
                || line.contains("|")
                || line.contains("help: remove this `mut`")
                || line.contains("#[warn(unused_mut)]")
                || line.contains("on by default")),
            "backend stderr contained non-unused_mut warning for {name}:\n{stderr}"
        );
    } else {
        assert_eq!(stderr, eval.stderr, "stderr divergence for {name}");
    }
}
