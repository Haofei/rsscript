use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use rsscript::{
    check_generated_rust_package, parse_runtime_diagnostics, write_generated_rust_package,
};

use super::{
    cleanup_temp_dir, default_runtime_path, generated_target_dir_from_env,
    lower_cli_input_to_rust_package, print_diagnostics, print_usage, required_flag_value,
    run_cache_dir,
};

#[derive(Debug)]
struct RunOptions<'a> {
    json: bool,
    release: bool,
    dry_run: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
    program_args: Vec<&'a str>,
}

fn parse_run_args(args: &[String]) -> Result<RunOptions<'_>, String> {
    let mut json = false;
    let mut release = false;
    let mut dry_run = false;
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
        } else if arg == "--release" {
            release = true;
        } else if arg == "--dry-run" {
            dry_run = true;
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

    Ok(RunOptions {
        json,
        release,
        dry_run,
        path,
        out_dir,
        program_args,
    })
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
    let runtime_path = match default_runtime_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
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
    let mut cargo = Command::new("cargo");
    for arg in cargo_run_args(&package_dir, options.release, &options.program_args) {
        cargo.arg(arg);
    }
    if let Some(target_dir) = generated_target_dir_from_env() {
        cargo.env("CARGO_TARGET_DIR", target_dir);
    }
    if let Ok(current_dir) = std::env::current_dir() {
        cargo.env("RSS_RUN_WORKSPACE_ROOT", current_dir);
    }
    let output = match cargo.output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("failed to run cargo: {error}");
            if cleanup_package_dir {
                cleanup_temp_dir(&package_dir);
            }
            return ExitCode::from(2);
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if output.status.success() {
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        return ExitCode::SUCCESS;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = parse_runtime_diagnostics(&stderr);
    if !diagnostics.is_empty() {
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        print_diagnostics(options.json, &diagnostics);
        return ExitCode::from(1);
    }
    match check_generated_rust_package(&package_dir) {
        Ok(result) if !result.diagnostics.is_empty() => {
            if cleanup_package_dir {
                cleanup_temp_dir(&package_dir);
            }
            print_diagnostics(options.json, &result.diagnostics);
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
    if cleanup_package_dir {
        cleanup_temp_dir(&package_dir);
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
        assert!(options.release);
        assert!(options.dry_run);
        assert_eq!(options.path, Some("packages/rayon/tests/sort-speed"));
        assert_eq!(options.program_args, vec!["input"]);
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
}
