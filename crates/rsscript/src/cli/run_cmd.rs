use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rsscript::{
    EvalError, EvalOutput, NativeValue, VmLimits, check_generated_rust_package,
    eval_package_main_with_args_and_native_bindings,
    eval_package_main_with_args_and_native_bindings_and_limits, format_diagnostics_human,
    format_diagnostics_json, load_package_native_bindings, parse_runtime_diagnostics,
    reg_vm_eval_source_main_with_args, reg_vm_eval_source_main_with_limits,
    write_generated_rust_package,
};

use super::{
    cleanup_temp_dir, cli_input_package_name, default_runtime_path, generated_target_dir_from_env,
    is_package_directory, lower_cli_input_to_rust_package, print_diagnostics, print_usage,
    read_cached_fingerprint, read_cli_source, required_flag_value, run_cache_dir,
    run_input_fingerprint, write_cached_fingerprint,
};
use crate::cli::process::run_bounded;

const CLI_VM_WALL_TIME: Duration = Duration::from_secs(60);
const CLI_AOT_WALL_TIME: Duration = Duration::from_secs(10 * 60);
const CLI_AOT_OUTPUT_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
struct RunOptions<'a> {
    json: bool,
    vm: bool,
    release: bool,
    dry_run: bool,
    trusted_unlimited: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
    program_args: Vec<&'a str>,
}

fn parse_run_args(args: &[String]) -> Result<RunOptions<'_>, String> {
    let mut json = false;
    let mut vm = false;
    let mut release = false;
    let mut dry_run = false;
    let mut trusted_unlimited = false;
    let mut path = None;
    let mut out_dir = None;
    let mut program_args = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--" {
            program_args.extend(args[index + 1..].iter().map(String::as_str));
            break;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--vm" {
            vm = true;
        } else if arg == "--release" {
            release = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--trusted-unlimited" {
            trusted_unlimited = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = Some(required_flag_value(args, index, "--out-dir")?);
        } else if arg.starts_with("--") && path.is_none() {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            program_args.push(arg.as_str());
        }
        index += 1;
    }

    let options = RunOptions {
        json,
        vm,
        release,
        dry_run,
        trusted_unlimited,
        path,
        out_dir,
        program_args,
    };
    validate_run_options(&options)?;
    Ok(options)
}

fn validate_run_options(options: &RunOptions<'_>) -> Result<(), String> {
    if options.vm && options.release {
        return Err("`rss run --vm` cannot be combined with `--release`.".to_string());
    }
    if options.vm && options.dry_run {
        return Err("`rss run --vm` cannot be combined with `--dry-run`.".to_string());
    }
    if options.vm && options.out_dir.is_some() {
        return Err("`rss run --vm` cannot be combined with `--out-dir`.".to_string());
    }
    if options.trusted_unlimited && !options.vm {
        return Err("`--trusted-unlimited` is only valid with `rss run --vm`.".to_string());
    }
    Ok(())
}
pub(crate) fn run_generated_rust(args: &[String]) -> ExitCode {
    let options = match parse_run_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(path) = options.path else {
        print_usage();
        return ExitCode::from(2);
    };
    if options.vm {
        return run_via_vm(
            path,
            &options.program_args,
            options.json,
            options.trusted_unlimited,
        );
    }
    let runtime_path = match default_runtime_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    // Fast path: for a default-cache run (no `--out-dir`, not a dry run), reuse a
    // previously lowered + compiled package when the source, runtime, and release
    // flag are byte-for-byte unchanged. This skips re-lowering entirely and lets
    // cargo's own up-to-date check make the rebuild a near no-op (often a direct
    // run of the cached binary). Correctness: the fingerprint covers every input
    // that affects generated output, so any change forces the full path below.
    if !options.dry_run
        && options.out_dir.is_none()
        && let Some(package_name) = cli_input_package_name(path)
    {
        let cache_dir = run_cache_dir(path, &package_name);
        let cached_package_present =
            cache_dir.join("Cargo.toml").is_file() && cache_dir.join("src/main.rs").is_file();
        if cached_package_present
            && let Some(fingerprint) = run_input_fingerprint(path, &runtime_path, options.release)
            && read_cached_fingerprint(&cache_dir).as_deref() == Some(fingerprint.as_str())
        {
            return run_cached_package(
                &cache_dir,
                options.release,
                &options.program_args,
                options.json,
            );
        }
    }

    let package = match lower_cli_input_to_rust_package(path, &runtime_path, options.json) {
        Ok(package) => package,
        Err(exit_code) => return exit_code,
    };
    if package.main_rs.is_none() {
        eprintln!(
            "rss run requires a zero-argument `fn main() -> Unit` or `fn main() -> Result<Unit, E>`."
        );
        return ExitCode::from(1);
    }

    if options.dry_run {
        if let Some(out_dir) = options.out_dir {
            let package_dir = PathBuf::from(out_dir);
            if let Err(error) = write_generated_rust_package(&package_dir, &package) {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
            print_run_dry_run(
                &package,
                Some(&package_dir),
                options.release,
                &options.program_args,
            );
        } else {
            print_run_dry_run(&package, None, options.release, &options.program_args);
        }
        return ExitCode::SUCCESS;
    }

    let is_default_cache = options.out_dir.is_none();
    let package_dir = options
        .out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| run_cache_dir(path, &package.package_name));
    let cleanup_package_dir = false;
    if let Err(error) = write_generated_rust_package(&package_dir, &package) {
        eprintln!("{error}");
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        return ExitCode::from(2);
    }
    // Record the input fingerprint so the next run of unchanged source hits the
    // fast path above. Only for the default cache dir; a user-chosen `--out-dir`
    // is left untouched. Written after the package files so a partial write never
    // leaves a fingerprint claiming a stale package is current.
    if is_default_cache
        && let Some(fingerprint) = run_input_fingerprint(path, &runtime_path, options.release)
    {
        write_cached_fingerprint(&package_dir, &fingerprint);
    }
    build_and_run_package(
        &package_dir,
        options.release,
        &options.program_args,
        options.json,
    )
}

/// Execute through the register VM instead of the Rust-lowering AOT backend.
/// This is the fast edit-run path folded into `rss run` so VM execution remains
/// available without growing the top-level command set.
fn run_via_vm(path: &str, program_args: &[&str], json: bool, trusted_unlimited: bool) -> ExitCode {
    let limits = if trusted_unlimited {
        VmLimits::default()
    } else {
        cli_vm_limits()
    };
    let result = if is_package_directory(path) {
        run_package_via_vm(path, program_args, limits)
    } else {
        let source = match read_cli_source(Path::new(path)) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        };
        run_source_via_vm(path, &source, program_args, limits)
    };
    finish_vm_run(result, json)
}

fn cli_vm_limits() -> VmLimits {
    let cancel = Arc::new(AtomicBool::new(false));
    let watchdog = Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(CLI_VM_WALL_TIME);
        watchdog.store(true, Ordering::Release);
    });
    VmLimits {
        cancel: Some(cancel),
        ..VmLimits::safe_default()
    }
}

fn run_source_via_vm(
    path: &str,
    source: &str,
    program_args: &[&str],
    limits: VmLimits,
) -> Result<EvalOutput, EvalError> {
    if limits.step_budget.is_none() {
        reg_vm_eval_source_main_with_args(path, source, program_args.iter().copied())
    } else {
        reg_vm_eval_source_main_with_limits(path, source, program_args.iter().copied(), limits)
    }
}

fn run_package_via_vm(
    path: &str,
    program_args: &[&str],
    limits: VmLimits,
) -> Result<EvalOutput, EvalError> {
    let package_dir = Path::new(path);
    let bindings = load_package_native_bindings(package_dir).map_err(EvalError::Runtime)?;
    if limits.step_budget.is_none() {
        eval_package_main_with_args_and_native_bindings(
            package_dir,
            program_args.iter().copied(),
            bindings,
        )
    } else {
        eval_package_main_with_args_and_native_bindings_and_limits(
            package_dir,
            program_args.iter().copied(),
            bindings,
            limits,
        )
    }
}

fn finish_vm_run(result: Result<EvalOutput, EvalError>, json: bool) -> ExitCode {
    match result {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
            if let Some(NativeValue::Variant { name, .. }) = &output.native_value
                && name == "Err"
            {
                eprintln!("RSScript main returned an error: {}", output.value);
                return ExitCode::from(1);
            }
            println!("{}", output.value);
            ExitCode::SUCCESS
        }
        Err(EvalError::Diagnostics(diagnostics)) => {
            if json {
                println!("{}", format_diagnostics_json(&diagnostics));
            } else {
                print!("{}", format_diagnostics_human(&diagnostics));
            }
            ExitCode::from(1)
        }
        Err(EvalError::Runtime(error)) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

/// Runs the fast-path cache hit: the generated package in `cache_dir` is already
/// up to date for the current source, so cargo's incremental check makes this a
/// near no-op build (or a direct run of the cached binary).
fn run_cached_package(
    cache_dir: &Path,
    release: bool,
    program_args: &[&str],
    json: bool,
) -> ExitCode {
    build_and_run_package(cache_dir, release, program_args, json)
}

/// Invokes `cargo run` for the generated package in `package_dir` and translates
/// its result into an [`ExitCode`], remapping backend diagnostics for the
/// captured (non-streaming) path. Shared by the full lower+write path and the
/// cached fast path so both behave identically.
fn build_and_run_package(
    package_dir: &Path,
    release: bool,
    program_args: &[&str],
    json: bool,
) -> ExitCode {
    let mut cargo = Command::new("cargo");
    for arg in cargo_run_args(package_dir, release, program_args) {
        cargo.arg(arg);
    }
    if let Some(target_dir) = generated_target_dir_from_env() {
        cargo.env("CARGO_TARGET_DIR", target_dir);
    }
    if let Ok(current_dir) = std::env::current_dir() {
        cargo.env("RSS_RUN_WORKSPACE_ROOT", current_dir);
    }
    let output = match run_bounded(
        &mut cargo,
        "generated Rust build and execution",
        CLI_AOT_WALL_TIME,
        CLI_AOT_OUTPUT_MAX_BYTES,
    ) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("failed to run cargo: {error}");
            return ExitCode::from(2);
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if output.status.success() {
        return ExitCode::SUCCESS;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = parse_runtime_diagnostics(&stderr);
    if !diagnostics.is_empty() {
        print_diagnostics(json, &diagnostics);
        return ExitCode::from(1);
    }
    match check_generated_rust_package(package_dir) {
        Ok(result) if !result.diagnostics.is_empty() => {
            print_diagnostics(json, &result.diagnostics);
            return if result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
            {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            };
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("{error}");
        }
    }
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }
    output
        .status
        .code()
        .map(|code| ExitCode::from(code as u8))
        .unwrap_or_else(|| ExitCode::from(1))
}

fn cargo_run_args(package_dir: &Path, release: bool, program_args: &[&str]) -> Vec<String> {
    let mut args = vec!["run".to_string(), "--quiet".to_string()];
    if release {
        args.push("--release".to_string());
    }
    args.push("--manifest-path".to_string());
    args.push(package_dir.join("Cargo.toml").display().to_string());
    args.push("--".to_string());
    args.extend(program_args.iter().map(|arg| (*arg).to_string()));
    args
}

fn print_run_dry_run(
    package: &rsscript::GeneratedRustPackage,
    package_dir: Option<&Path>,
    release: bool,
    program_args: &[&str],
) {
    let manifest_path = package_dir
        .map(|dir| dir.join("Cargo.toml").display().to_string())
        .unwrap_or_else(|| "<dry-run>/Cargo.toml".to_string());
    let command_dir = package_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("<dry-run>"));
    let cargo_args = cargo_run_args(&command_dir, release, program_args);
    println!("## cargo invocation");
    println!("cargo {}", cargo_args.join(" "));
    println!();
    println!("## Cargo.toml ({manifest_path})");
    print!("{}", package.cargo_toml);
    if !package.cargo_toml.ends_with('\n') {
        println!();
    }
    if let Some(main_rs) = &package.main_rs {
        let main_path = package_dir
            .map(|dir| dir.join("src/main.rs").display().to_string())
            .unwrap_or_else(|| "<dry-run>/src/main.rs".to_string());
        println!();
        println!("## src/main.rs ({main_path})");
        print!("{main_rs}");
        if !main_rs.ends_with('\n') {
            println!();
        }
    }
    let lib_path = package_dir
        .map(|dir| dir.join("src/lib.rs").display().to_string())
        .unwrap_or_else(|| "<dry-run>/src/lib.rs".to_string());
    println!();
    println!("## src/lib.rs ({lib_path})");
    print!("{}", package.lib_rs);
    if !package.lib_rs.ends_with('\n') {
        println!();
    }
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_run_args_accepts_release_before_path() {
        let values = args(&[
            "--json",
            "--release",
            "--dry-run",
            "packages/rayon/tests/sort-speed",
            "--",
            "input",
        ]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(!options.vm);
        assert!(options.release);
        assert!(options.dry_run);
        assert_eq!(options.path, Some("packages/rayon/tests/sort-speed"));
        assert_eq!(options.program_args, vec!["input"]);
    }

    #[test]
    fn parse_run_args_accepts_vm_before_path() {
        let values = args(&["--json", "--vm", "demo.rss", "--", "input"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(options.vm);
        assert!(!options.release);
        assert!(!options.dry_run);
        assert!(!options.trusted_unlimited);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["input"]);
    }

    #[test]
    fn parse_run_args_treats_vm_after_separator_as_program_arg() {
        let values = args(&["demo.rss", "--", "--vm"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(!options.vm);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["--vm"]);
    }

    #[test]
    fn parse_run_args_treats_release_after_separator_as_program_arg() {
        let values = args(&["packages/rayon/tests/sort-speed", "--", "--release"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(!options.release);
        assert!(!options.dry_run);
        assert_eq!(options.path, Some("packages/rayon/tests/sort-speed"));
        assert_eq!(options.program_args, vec!["--release"]);
    }

    #[test]
    fn parse_run_args_treats_dry_run_after_separator_as_program_arg() {
        let values = args(&["demo.rss", "--", "--dry-run"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(!options.dry_run);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["--dry-run"]);
    }

    #[test]
    fn cargo_run_args_include_manifest_release_and_program_args() {
        let package_dir = std::path::Path::new("/tmp/rss-generated");
        let args = super::cargo_run_args(package_dir, true, &["one", "two"]);

        assert_eq!(
            args,
            vec![
                "run",
                "--quiet",
                "--release",
                "--manifest-path",
                "/tmp/rss-generated/Cargo.toml",
                "--",
                "one",
                "two"
            ]
        );
    }

    #[test]
    fn parse_run_args_rejects_missing_out_dir_value() {
        let values = args(&["demo.rss", "--out-dir"]);
        let error = super::parse_run_args(&values).expect_err("missing out-dir should fail");

        assert_eq!(error, "missing value for `--out-dir`.");
    }

    #[test]
    fn parse_run_args_rejects_vm_release_combo() {
        let values = args(&["--vm", "--release", "demo.rss"]);
        let error = super::parse_run_args(&values).expect_err("vm release combo should fail");

        assert_eq!(error, "`rss run --vm` cannot be combined with `--release`.");
    }

    #[test]
    fn parse_run_args_rejects_vm_dry_run_combo() {
        let values = args(&["--vm", "--dry-run", "demo.rss"]);
        let error = super::parse_run_args(&values).expect_err("vm dry-run combo should fail");

        assert_eq!(error, "`rss run --vm` cannot be combined with `--dry-run`.");
    }

    #[test]
    fn parse_run_args_rejects_vm_out_dir_combo() {
        let values = args(&["--vm", "demo.rss", "--out-dir", "generated"]);
        let error = super::parse_run_args(&values).expect_err("vm out-dir combo should fail");

        assert_eq!(error, "`rss run --vm` cannot be combined with `--out-dir`.");
    }

    #[test]
    fn parse_run_args_accepts_explicit_trusted_unlimited_vm_mode() {
        let values = args(&["--vm", "--trusted-unlimited", "demo.rss"]);
        let options = super::parse_run_args(&values).expect("trusted mode should parse");
        assert!(options.vm);
        assert!(options.trusted_unlimited);
    }

    #[test]
    fn parse_run_args_rejects_trusted_unlimited_aot_mode() {
        let values = args(&["--trusted-unlimited", "demo.rss"]);
        let error = super::parse_run_args(&values).expect_err("AOT mode stays bounded");
        assert_eq!(
            error,
            "`--trusted-unlimited` is only valid with `rss run --vm`."
        );
    }
}
