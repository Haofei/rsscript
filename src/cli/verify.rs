use std::path::PathBuf;
use std::process::ExitCode;

use rsscript::{check_generated_rust_package, write_generated_rust_package};

use super::{
    cleanup_temp_dir, default_runtime_path, lower_cli_input_to_rust_package, print_diagnostics,
    print_usage, required_flag_value, verify_temp_dir,
};

#[derive(Debug)]
struct VerifyOptions<'a> {
    json: bool,
    path: Option<&'a str>,
    out_dir: Option<&'a str>,
}

fn parse_verify_args(args: &[String]) -> Result<VerifyOptions<'_>, String> {
    let mut json = false;
    let mut path = None;
    let mut out_dir = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--out-dir" {
            index += 1;
            out_dir = Some(required_flag_value(args, index, "--out-dir")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
        index += 1;
    }

    Ok(VerifyOptions {
        json,
        path,
        out_dir,
    })
}
pub(crate) fn run_verify_rust(args: &[String]) -> ExitCode {
    let options = match parse_verify_args(args) {
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
    let package_dir = options
        .out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| verify_temp_dir(&package.package_name));
    let cleanup_package_dir = options.out_dir.is_none();
    if let Err(error) = write_generated_rust_package(&package_dir, &package) {
        eprintln!("{error}");
        if cleanup_package_dir {
            cleanup_temp_dir(&package_dir);
        }
        return ExitCode::from(2);
    }
    let result = match check_generated_rust_package(&package_dir) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            if cleanup_package_dir {
                cleanup_temp_dir(&package_dir);
            }
            return ExitCode::from(2);
        }
    };
    if cleanup_package_dir {
        cleanup_temp_dir(&package_dir);
    }

    if result.diagnostics.is_empty() {
        if result.success {
            if !options.json {
                println!("{path}: rust backend ok");
                if options.out_dir.is_some() {
                    println!("generated Rust package kept at {}", package_dir.display());
                }
            } else {
                println!("[]");
            }
            return ExitCode::SUCCESS;
        }
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr.trim());
        }
        eprintln!("rust backend check failed without mappable diagnostics");
        return ExitCode::from(1);
    }

    print_diagnostics(options.json, &result.diagnostics);
    if result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_verify_args_rejects_extra_paths() {
        let values = args(&["one.rss", "two.rss"]);
        let error = super::parse_verify_args(&values).expect_err("extra path should fail");

        assert_eq!(error, "unexpected extra path `two.rss`.");
    }
}
