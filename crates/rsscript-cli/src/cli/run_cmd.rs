#[cfg(feature = "aot-rust")]
use std::ffi::OsString;
#[cfg(feature = "aot-rust")]
use std::path::{Path, PathBuf};
#[cfg(feature = "aot-rust")]
use std::process::Command;
use std::process::ExitCode;
#[cfg(feature = "aot-rust")]
use std::time::Duration;

#[cfg(feature = "aot-rust")]
use rsscript_compiler::{parse_runtime_diagnostics, write_generated_rust_package};

#[cfg(feature = "aot-rust")]
use super::{
    cleanup_temp_dir, cli_input_package_name, default_runtime_path, generated_target_dir_from_env,
    lower_cli_input_to_rust_package, print_diagnostics, read_cached_fingerprint, run_cache_dir,
    run_input_fingerprint, write_cached_fingerprint,
};
use super::{print_usage, required_flag_value};
#[cfg(feature = "aot-rust")]
use crate::cli::process::{BoundedProcessKind, run_bounded, run_bounded_with_limits};

#[cfg(feature = "aot-rust")]
const CLI_AOT_WALL_TIME: Duration = Duration::from_secs(10 * 60);
#[cfg(feature = "aot-rust")]
const CLI_AOT_OUTPUT_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
struct RunOptions<'a> {
    json: bool,
    aot: bool,
    release: bool,
    dry_run: bool,
    trusted_in_process: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
    program_args: Vec<&'a str>,
}

fn parse_run_args(args: &[String]) -> Result<RunOptions<'_>, String> {
    let mut json = false;
    let mut aot = false;
    let mut release = false;
    let mut dry_run = false;
    let mut trusted_in_process = false;
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
        } else if arg == "--aot" {
            aot = true;
        } else if arg == "--release" {
            release = true;
        } else if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--trusted-in-process" {
            trusted_in_process = true;
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
        aot,
        release,
        dry_run,
        trusted_in_process,
        path,
        out_dir,
        program_args,
    };
    validate_run_options(&options)?;
    Ok(options)
}

fn validate_run_options(options: &RunOptions<'_>) -> Result<(), String> {
    if !options.aot && options.release {
        return Err("`rss run --release` requires the experimental `--aot` backend.".to_string());
    }
    if !options.aot && options.dry_run {
        return Err("`rss run --dry-run` requires the experimental `--aot` backend.".to_string());
    }
    if !options.aot && options.out_dir.is_some() {
        return Err("`rss run --out-dir` requires the experimental `--aot` backend.".to_string());
    }
    if options.aot && options.trusted_in_process {
        return Err("`--trusted-in-process` cannot be combined with `--aot`.".to_string());
    }
    Ok(())
}
pub(crate) fn run_input(args: &[String]) -> ExitCode {
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
    if options.aot {
        #[cfg(not(feature = "aot-rust"))]
        {
            eprintln!(
                "`rss run --aot` requires this experimental CLI build to enable the `aot-rust` feature."
            );
            return ExitCode::from(2);
        }
        #[cfg(feature = "aot-rust")]
        return run_aot(path, &options);
    }
    if options.trusted_in_process {
        return super::runner::run_trusted_in_process(path, &options.program_args, options.json);
    }
    super::runner::run_isolated(path, &options.program_args, options.json)
}

/// Experimental generated-Rust execution path. It is deliberately excluded
/// from the ordinary CLI `execution` feature so the reference VM/runner path
/// remains the only default execution product.
#[cfg(feature = "aot-rust")]
fn run_aot(path: &str, options: &RunOptions<'_>) -> ExitCode {
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

/// Runs the fast-path cache hit: the generated package in `cache_dir` is already
/// up to date for the current source, so cargo's incremental check makes this a
/// near no-op build (or a direct run of the cached binary).
#[cfg(feature = "aot-rust")]
fn run_cached_package(
    cache_dir: &Path,
    release: bool,
    program_args: &[&str],
    json: bool,
) -> ExitCode {
    build_and_run_package(cache_dir, release, program_args, json)
}

/// Build in a reduced environment, then execute the emitted binary as a
/// separately bounded child. Build scripts never inherit the program's ambient
/// environment.
#[cfg(feature = "aot-rust")]
fn build_and_run_package(
    package_dir: &Path,
    release: bool,
    program_args: &[&str],
    json: bool,
) -> ExitCode {
    let mut cargo = Command::new("cargo");
    for arg in cargo_build_args(package_dir, release) {
        cargo.arg(arg);
    }
    configure_reduced_build_environment(&mut cargo);
    if let Some(target_dir) = generated_target_dir_from_env() {
        cargo.env("CARGO_TARGET_DIR", target_dir);
    }
    let build_output = match run_bounded_with_limits(
        &mut cargo,
        "generated Rust build",
        CLI_AOT_WALL_TIME,
        CLI_AOT_OUTPUT_MAX_BYTES,
        BoundedProcessKind::CompilerWorker,
    ) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("failed to build generated Rust: {error}");
            return ExitCode::from(2);
        }
    };
    let build_stderr = String::from_utf8_lossy(&build_output.stderr);
    if !build_output.status.success() {
        return finish_failed_aot_build(&build_stderr, build_output.status);
    }
    let executable = match cargo_artifact_executable(&build_output.stdout) {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("failed to locate generated Rust executable: {error}");
            return ExitCode::from(2);
        }
    };

    let mut program = Command::new(executable);
    program.args(program_args);
    if let Ok(current_dir) = std::env::current_dir() {
        program.env("RSS_RUN_WORKSPACE_ROOT", current_dir);
    }
    let output = match run_bounded(
        &mut program,
        "generated Rust program",
        CLI_AOT_WALL_TIME,
        CLI_AOT_OUTPUT_MAX_BYTES,
    ) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("failed to run generated Rust: {error}");
            return ExitCode::from(2);
        }
    };
    print!("{}", String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        eprint!("{stderr}");
        return ExitCode::SUCCESS;
    }
    let diagnostics = parse_runtime_diagnostics(&stderr);
    if !diagnostics.is_empty() {
        print_diagnostics(json, &diagnostics);
        return ExitCode::from(1);
    }
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }
    exit_code(output.status)
}

#[cfg(feature = "aot-rust")]
fn finish_failed_aot_build(stderr: &str, status: std::process::ExitStatus) -> ExitCode {
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }
    exit_code(status)
}

#[cfg(feature = "aot-rust")]
fn configure_reduced_build_environment(command: &mut Command) {
    const ALLOWED: &[&str] = &[
        "PATH",
        "HOME",
        "USERPROFILE",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTC",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SYSTEMROOT",
        "WINDIR",
    ];
    let preserved = ALLOWED
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect::<Vec<_>>();
    command.env_clear();
    command.envs(preserved);
    command
        .env("CARGO_TERM_COLOR", "never")
        .env("TERM", "dumb")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("SOURCE_DATE_EPOCH", "0")
        .env_remove("CARGO_TARGET_DIR");
}

#[cfg(feature = "aot-rust")]
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(portable_exit_code(code));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ExitCode::from(portable_exit_code(128_i32.saturating_add(signal)));
        }
    }
    ExitCode::from(1)
}

#[cfg(feature = "aot-rust")]
fn portable_exit_code(code: i32) -> u8 {
    u8::try_from(code).unwrap_or(1)
}

#[cfg(feature = "aot-rust")]
fn cargo_build_args(package_dir: &Path, release: bool) -> Vec<String> {
    let mut args = vec![
        "build".to_string(),
        "--quiet".to_string(),
        "--offline".to_string(),
        "--message-format=json-render-diagnostics".to_string(),
    ];
    if release {
        args.push("--release".to_string());
    }
    args.push("--manifest-path".to_string());
    args.push(package_dir.join("Cargo.toml").display().to_string());
    args
}

#[cfg(feature = "aot-rust")]
fn cargo_artifact_executable(stdout: &[u8]) -> Result<PathBuf, String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| {
            message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|message| message["executable"].as_str().map(PathBuf::from))
        .next_back()
        .ok_or_else(|| "Cargo emitted no executable compiler artifact".to_string())
}

#[cfg(feature = "aot-rust")]
fn print_run_dry_run(
    package: &rsscript_compiler::GeneratedRustPackage,
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
    let cargo_args = cargo_build_args(&command_dir, release);
    println!("## cargo build invocation");
    println!("cargo {}", cargo_args.join(" "));
    println!("## generated program invocation");
    println!("<cargo-artifact> {}", program_args.join(" "));
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

    #[cfg(feature = "aot-rust")]
    #[test]
    fn portable_exit_code_rejects_values_that_exit_code_cannot_represent() {
        assert_eq!(super::portable_exit_code(0), 0);
        assert_eq!(super::portable_exit_code(255), 255);
        assert_eq!(super::portable_exit_code(-1), 1);
        assert_eq!(super::portable_exit_code(256), 1);
    }

    #[test]
    fn parse_run_args_accepts_release_before_path() {
        let values = args(&[
            "--json",
            "--aot",
            "--release",
            "--dry-run",
            "packages/native-abi-fixture",
            "--",
            "input",
        ]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(options.aot);
        assert!(options.release);
        assert!(options.dry_run);
        assert_eq!(options.path, Some("packages/native-abi-fixture"));
        assert_eq!(options.program_args, vec!["input"]);
    }

    #[test]
    fn parse_run_args_accepts_default_vm_path() {
        let values = args(&["--json", "demo.rss", "--", "input"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(!options.aot);
        assert!(!options.release);
        assert!(!options.dry_run);
        assert!(!options.trusted_in_process);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["input"]);
    }

    #[test]
    fn parse_run_args_treats_aot_after_separator_as_program_arg() {
        let values = args(&["demo.rss", "--", "--aot"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(!options.aot);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["--aot"]);
    }

    #[test]
    fn parse_run_args_treats_release_after_separator_as_program_arg() {
        let values = args(&["packages/native-abi-fixture", "--", "--release"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(!options.release);
        assert!(!options.dry_run);
        assert_eq!(options.path, Some("packages/native-abi-fixture"));
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

    #[cfg(feature = "aot-rust")]
    #[test]
    fn cargo_build_args_include_manifest_release_and_json_messages() {
        let package_dir = std::path::Path::new("/tmp/rss-generated");
        let args = super::cargo_build_args(package_dir, true);

        assert_eq!(
            args,
            vec![
                "build",
                "--quiet",
                "--offline",
                "--message-format=json-render-diagnostics",
                "--release",
                "--manifest-path",
                "/tmp/rss-generated/Cargo.toml",
            ]
        );
    }

    #[cfg(feature = "aot-rust")]
    #[test]
    fn cargo_artifact_parser_selects_binary_executable() {
        let messages = br#"{"reason":"compiler-artifact","target":{"kind":["lib"]},"executable":null}
{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"/tmp/rss-generated/target/debug/demo"}
"#;

        assert_eq!(
            super::cargo_artifact_executable(messages).expect("binary artifact"),
            std::path::PathBuf::from("/tmp/rss-generated/target/debug/demo")
        );
    }

    #[test]
    fn parse_run_args_rejects_missing_out_dir_value() {
        let values = args(&["demo.rss", "--out-dir"]);
        let error = super::parse_run_args(&values).expect_err("missing out-dir should fail");

        assert_eq!(error, "missing value for `--out-dir`.");
    }

    #[test]
    fn parse_run_args_requires_aot_for_release() {
        let values = args(&["--release", "demo.rss"]);
        let error = super::parse_run_args(&values).expect_err("release requires aot");

        assert_eq!(
            error,
            "`rss run --release` requires the experimental `--aot` backend."
        );
    }

    #[test]
    fn parse_run_args_requires_aot_for_dry_run() {
        let values = args(&["--dry-run", "demo.rss"]);
        let error = super::parse_run_args(&values).expect_err("dry-run requires aot");

        assert_eq!(
            error,
            "`rss run --dry-run` requires the experimental `--aot` backend."
        );
    }

    #[test]
    fn parse_run_args_requires_aot_for_out_dir() {
        let values = args(&["demo.rss", "--out-dir", "generated"]);
        let error = super::parse_run_args(&values).expect_err("out-dir requires aot");

        assert_eq!(
            error,
            "`rss run --out-dir` requires the experimental `--aot` backend."
        );
    }

    #[test]
    fn removed_vm_alias_is_rejected() {
        let values = args(&["--vm", "demo.rss"]);
        assert!(super::parse_run_args(&values).is_err());
    }

    #[test]
    fn trusted_in_process_is_explicit_and_not_an_aot_alias() {
        let values = args(&["--trusted-in-process", "demo.rss"]);
        let options = super::parse_run_args(&values).expect("trusted option");
        assert!(options.trusted_in_process);
        let invalid = args(&["--aot", "--trusted-in-process", "demo.rss"]);
        assert!(super::parse_run_args(&invalid).is_err());
    }
}
