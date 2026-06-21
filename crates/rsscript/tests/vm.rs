#![allow(unused_imports, dead_code)]
pub(crate) use rsscript::{
    NativeInterpreterFn, NativeValue, lower_source_to_rust_package, reg_vm_compile_source,
    reg_vm_eval_source_main_with_args, reg_vm_eval_source_main_with_args_and_native_bindings,
    write_generated_rust_package,
};
pub(crate) use sha1::{Digest, Sha1};
pub(crate) use std::fs;
pub(crate) use std::path::Path;
pub(crate) use std::process::Command;
mod common;

fn assert_reg_vm_matches_compiled_backend<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
        .expect("reg vm should run");
    if !common::full_backend_parity_enabled() {
        return;
    }
    let runtime_path = common::runtime_path();
    let cache_dir = common::workspace_root().join("target/rsscript-vm-compiled-output-cache");
    fs::create_dir_all(&cache_dir).expect("compiled parity cache dir should create");
    let cache_key = compiled_output_cache_key(file, source, &args);
    let stdout_path = cache_dir.join(format!("{cache_key}.stdout"));
    let stderr_path = cache_dir.join(format!("{cache_key}.stderr"));
    let (stdout, stderr) = if stdout_path.exists() && stderr_path.exists() {
        (
            fs::read_to_string(&stdout_path).expect("cached stdout should read"),
            fs::read_to_string(&stderr_path).expect("cached stderr should read"),
        )
    } else {
        let (stdout, stderr) = run_compiled_backend(file, source, &args, &runtime_path);
        fs::write(&stdout_path, &stdout).expect("cached stdout should write");
        fs::write(&stderr_path, &stderr).expect("cached stderr should write");
        (stdout, stderr)
    };

    assert_eq!(reg.stdout, stdout);
    assert_eq!(reg.stderr, stderr);
}

fn assert_reg_vm_output<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
    expected_display_value: &str,
    expected_stdout: &str,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
        .expect("reg vm should run");

    assert_eq!(reg.value, expected_display_value);
    assert_eq!(reg.display_value, expected_display_value);
    assert_eq!(reg.stdout, expected_stdout);
    assert_eq!(reg.stderr, "");
}

#[derive(Clone, Copy)]
enum CompiledReturnHarness {
    HttpRequest,
    Image,
    JsonValue,
    ResultUnitString,
}

impl CompiledReturnHarness {
    fn cache_tag(self) -> &'static str {
        match self {
            Self::HttpRequest => "return-http-request",
            Self::Image => "return-image",
            Self::JsonValue => "return-json-value",
            Self::ResultUnitString => "return-result-unit-string",
        }
    }

    fn main_rs(self, crate_name: &str) -> String {
        match self {
            Self::HttpRequest => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    let value = {crate_name}::main();\n",
                    "    println!(\"__RSSCRIPT_RETURN__{{}}\", rsscript_runtime::http_request_debug_summary(&value));\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
            Self::Image => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    let value = {crate_name}::main();\n",
                    "    println!(\"__RSSCRIPT_RETURN__{{}}\", rsscript_runtime::image_debug_summary(&value));\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
            Self::JsonValue => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    let value = {crate_name}::main();\n",
                    "    println!(\"__RSSCRIPT_RETURN__{{}}\", rsscript_runtime::json_to_string(&value));\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
            Self::ResultUnitString => format!(
                concat!(
                    "fn main() {{\n",
                    "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
                    "    match {crate_name}::main() {{\n",
                    "        Ok(()) => println!(\"__RSSCRIPT_RETURN__Ok {{{{ value: Unit }}}}\"),\n",
                    "        Err(error) => println!(\"__RSSCRIPT_RETURN__Err {{{{ value: {{}} }}}}\", error),\n",
                    "    }}\n",
                    "}}\n"
                ),
                crate_name = crate_name
            ),
        }
    }
}

fn assert_reg_vm_matches_compiled_backend_return<'a>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
    harness: CompiledReturnHarness,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args(file, source, args.iter().copied())
        .expect("reg vm should run");
    if !common::full_backend_parity_enabled() {
        return;
    }
    let runtime_path = common::runtime_path();
    let cache_dir = common::workspace_root().join("target/rsscript-vm-compiled-output-cache");
    fs::create_dir_all(&cache_dir).expect("compiled parity cache dir should create");
    let cache_key = compiled_output_cache_key_with_tag(file, source, &args, harness.cache_tag());
    let stdout_path = cache_dir.join(format!("{cache_key}.stdout"));
    let stderr_path = cache_dir.join(format!("{cache_key}.stderr"));
    let (stdout, stderr) = if stdout_path.exists() && stderr_path.exists() {
        (
            fs::read_to_string(&stdout_path).expect("cached stdout should read"),
            fs::read_to_string(&stderr_path).expect("cached stderr should read"),
        )
    } else {
        let (stdout, stderr) =
            run_compiled_backend_with_return_harness(file, source, &args, &runtime_path, harness);
        fs::write(&stdout_path, &stdout).expect("cached stdout should write");
        fs::write(&stderr_path, &stderr).expect("cached stderr should write");
        (stdout, stderr)
    };

    let (compiled_stdout, compiled_return) =
        split_compiled_return_stdout(&stdout).expect("compiled return marker should exist");
    assert_eq!(reg.stdout, compiled_stdout);
    assert_eq!(reg.stderr, stderr);
    assert_eq!(reg.display_value, compiled_return);
}

fn split_compiled_return_stdout(stdout: &str) -> Option<(String, String)> {
    const MARKER: &str = "__RSSCRIPT_RETURN__";
    let marker_start = stdout.rfind(MARKER)?;
    let return_start = marker_start + MARKER.len();
    let return_end = stdout[return_start..]
        .find('\n')
        .map(|offset| return_start + offset)
        .unwrap_or(stdout.len());
    Some((
        stdout[..marker_start].to_string(),
        stdout[return_start..return_end].to_string(),
    ))
}

fn run_compiled_backend(
    file: &str,
    source: &str,
    args: &[&str],
    runtime_path: &str,
) -> (String, String) {
    let package_name = format!(
        "rsscript_{}",
        file.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
            .trim_matches('_')
    );
    let package = lower_source_to_rust_package(file, source, &package_name, runtime_path)
        .expect("source should lower to Rust package");
    let package_dir = common::unique_temp_dir("rsscript-reg-vm-compiled-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
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
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

fn run_compiled_backend_with_return_harness(
    file: &str,
    source: &str,
    args: &[&str],
    runtime_path: &str,
    harness: CompiledReturnHarness,
) -> (String, String) {
    let package_name = format!(
        "rsscript_{}",
        file.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
            .trim_matches('_')
    );
    let mut package = lower_source_to_rust_package(file, source, &package_name, runtime_path)
        .expect("source should lower to Rust package");
    let crate_name = package.package_name.replace('-', "_");
    if !package.lib_rs.contains("pub fn main(") {
        package.lib_rs = package.lib_rs.replacen("fn main(", "pub fn main(", 1);
    }
    package.main_rs = Some(harness.main_rs(&crate_name));
    let package_dir = common::unique_temp_dir("rsscript-reg-vm-compiled-parity");
    write_generated_rust_package(&package_dir, &package).expect("generated package should write");

    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(package_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", common::generated_target_dir())
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
        "generated Rust package failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

fn compiled_output_cache_key(file: &str, source: &str, args: &[&str]) -> String {
    compiled_output_cache_key_with_tag(file, source, args, "stdout-stderr")
}

fn compiled_output_cache_key_with_tag(
    file: &str,
    source: &str,
    args: &[&str],
    tag: &str,
) -> String {
    let crate_root = common::crate_root();
    let workspace_root = common::workspace_root();
    let mut hasher = Sha1::new();
    hasher.update(b"rsscript-vm-compiled-output-v2");
    hasher.update(tag.as_bytes());
    hasher.update(file.as_bytes());
    hasher.update(source.as_bytes());
    for arg in args {
        hasher.update(b"\0arg\0");
        hasher.update(arg.as_bytes());
    }
    hash_path_if_exists(&mut hasher, &workspace_root.join("Cargo.toml"));
    hash_path_if_exists(&mut hasher, &workspace_root.join("Cargo.lock"));
    hash_path_if_exists(&mut hasher, &crate_root.join("Cargo.toml"));
    hash_path_if_exists(&mut hasher, &crate_root.join("build.rs"));
    hash_path_if_exists(&mut hasher, &crate_root.join("src/lib.rs"));
    hash_path_if_exists(&mut hasher, &crate_root.join("src/core_index.rs"));
    hash_path_if_exists(&mut hasher, &crate_root.join("src/runtime_abi.rs"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/analyzer"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/checks"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/package"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/rust_lower"));
    hash_tree_if_exists(&mut hasher, &crate_root.join("src/syntax"));
    hash_tree_if_exists(&mut hasher, &workspace_root.join("crates/runtime"));
    format!("{:x}", hasher.finalize())
}

fn hash_tree_if_exists(hasher: &mut Sha1, path: &Path) {
    if !path.exists() {
        return;
    }
    if path.is_file() {
        hash_path_if_exists(hasher, path);
        return;
    }
    let mut entries = fs::read_dir(path)
        .expect("cache fingerprint directory should read")
        .map(|entry| entry.expect("cache fingerprint entry should read").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        hash_tree_if_exists(hasher, &entry);
    }
}

fn hash_path_if_exists(hasher: &mut Sha1, path: &Path) {
    if !path.exists() || !path.is_file() {
        return;
    }
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(fs::read(path).expect("cache fingerprint file should read"));
}

fn assert_reg_vm_with_native_output<'a, const N: usize>(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = &'a str>,
    native_bindings: [(&'static str, NativeInterpreterFn); N],
    expected_stdout: &str,
) {
    let args = args.into_iter().collect::<Vec<_>>();
    let reg = reg_vm_eval_source_main_with_args_and_native_bindings(
        file,
        source,
        args.iter().copied(),
        native_bindings,
    )
    .expect("reg vm should run");

    assert_eq!(reg.value, "Unit");
    assert_eq!(reg.display_value, "Unit");
    assert_eq!(reg.native_value, Some(NativeValue::Unit));
    assert_eq!(reg.stdout, expected_stdout);
    assert_eq!(reg.stderr, "");
}

#[path = "vm/closures.rs"]
mod closures;
#[path = "vm/collections.rs"]
mod collections;
#[path = "vm/control_flow.rs"]
mod control_flow;
#[path = "vm/misc.rs"]
mod misc;
#[path = "vm/native_managed.rs"]
mod native_managed;
#[path = "vm/strings.rs"]
mod strings;
#[path = "vm/types.rs"]
mod types;
