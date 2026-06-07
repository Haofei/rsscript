#![allow(dead_code)]

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript::{
    EvalError, EvalOutput, Severity, analyze_source, lower_source_to_rust_package,
    reg_vm_eval_source_main_with_args, write_generated_rust_package,
};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Differential execution engine shared by the corpus framework and unit tests.
// `run_vm_source` evaluates a program on the register VM; `run_compiled_source`
// lowers it to Rust, builds, and runs it (cached). Asserting the two agree is
// the backbone of the framework.
// ---------------------------------------------------------------------------

/// Evaluate a single-file program's `main` on the register VM.
pub fn run_vm_source(file: &str, source: &str, args: &[&str]) -> Result<EvalOutput, EvalError> {
    reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
}

/// Error-severity diagnostic codes (sorted, unique) the checker reports for a
/// single-file program.
pub fn error_codes(file: &str, source: &str) -> Vec<String> {
    let mut codes = analyze_source(file, source)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code)
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
    let cache_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/rsscript-corpus-compiled-cache");
    fs::create_dir_all(&cache_dir).expect("compiled cache dir should create");
    let key = compiled_cache_key(file, source, args);
    let stdout_path = cache_dir.join(format!("{key}.stdout"));
    let stderr_path = cache_dir.join(format!("{key}.stderr"));
    if let (Ok(stdout), Ok(stderr)) = (
        fs::read_to_string(&stdout_path),
        fs::read_to_string(&stderr_path),
    ) {
        return (stdout, stderr);
    }
    let (stdout, stderr) = compile_and_run(file, source, args);
    let _ = fs::write(&stdout_path, &stdout);
    let _ = fs::write(&stderr_path, &stderr);
    (stdout, stderr)
}

fn compiled_cache_key(file: &str, source: &str, args: &[&str]) -> String {
    let mut hasher = DefaultHasher::new();
    file.hash(&mut hasher);
    source.hash(&mut hasher);
    args.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn compile_and_run(file: &str, source: &str, args: &[&str]) -> (String, String) {
    let runtime_path = format!("{}/runtime", env!("CARGO_MANIFEST_DIR"));
    let package_name = format!(
        "rsscript_{}",
        file.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
            .trim_matches('_')
    );
    let package = lower_source_to_rust_package(file, source, &package_name, &runtime_path)
        .expect("source should lower to a Rust package");
    let package_dir = unique_temp_dir("rsscript-corpus-compiled");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            format!(
                "{}/target/rsscript-generated-test",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .env("RUSTFLAGS", "-Awarnings");
    if !args.is_empty() {
        command.arg("--").args(args);
    }
    let output = command.output().expect("generated Rust package should run");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = fs::remove_dir_all(&package_dir);
    assert!(
        output.status.success(),
        "compiled backend for `{file}` failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

pub fn fixture_paths(directory: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {directory}: {error}"))
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
    collect_fixture_paths(Path::new(directory), &mut paths);
    paths.sort();
    paths
}

pub fn recursive_paths_with_extension(directory: &str, extension: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_paths_with_extension(Path::new(directory), extension, &mut paths);
    paths.sort();
    paths
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
