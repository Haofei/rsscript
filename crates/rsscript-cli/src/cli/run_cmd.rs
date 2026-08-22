use std::process::ExitCode;

use rsscript_runner_protocol::RunnerProfileV1;

use super::{print_usage, required_flag_value};

/// Options for the supported reference-VM execution paths. Generated Rust/AOT
/// execution deliberately lives in the experiments workspace: the product CLI
/// must not select an experimental backend or compile it into its dependency
/// closure.
#[derive(Debug)]
struct RunOptions<'a> {
    json: bool,
    trusted_in_process: bool,
    native: bool,
    profile: RunnerProfileV1,
    path: Option<&'a str>,
    program_args: Vec<&'a str>,
}

fn parse_run_args(args: &[String]) -> Result<RunOptions<'_>, String> {
    let mut json = false;
    let mut trusted_in_process = false;
    let mut native = false;
    let mut profile = RunnerProfileV1::default();
    let mut path = None;
    let mut program_args = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--" {
            program_args.extend(args[index + 1..].iter().map(String::as_str));
            break;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--trusted-in-process" {
            trusted_in_process = true;
        } else if arg == "--native" {
            native = true;
        } else if arg == "--profile" {
            index += 1;
            let name = required_flag_value(args, index, "--profile")?;
            profile = RunnerProfileV1::parse_name(name)
                .ok_or_else(|| format!("unknown runner profile `{name}`."))?;
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
        trusted_in_process,
        native,
        profile,
        path,
        program_args,
    };
    validate_run_options(&options)?;
    Ok(options)
}

fn validate_run_options(options: &RunOptions<'_>) -> Result<(), String> {
    if options.trusted_in_process && options.profile != RunnerProfileV1::default() {
        return Err(
            "`--profile` selects an isolated runner profile and cannot be combined with `--trusted-in-process`."
                .to_string(),
        );
    }
    if options.native && !options.trusted_in_process {
        return Err("`--native` requires explicit `--trusted-in-process`.".to_string());
    }
    #[cfg(not(feature = "native-jit"))]
    if options.native {
        return Err("this `rss` binary was built without native-JIT support.".to_string());
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
    if options.trusted_in_process {
        return super::runner::run_trusted_in_process(
            path,
            &options.program_args,
            options.json,
            options.native,
        );
    }
    super::runner::run_isolated(path, &options.program_args, options.json, options.profile)
}

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_run_args_uses_isolated_runner_by_default() {
        let values = args(&["--json", "demo.rss", "--", "input"]);
        let options = super::parse_run_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(!options.trusted_in_process);
        assert!(!options.native);
        assert_eq!(
            options.profile,
            rsscript_runner_protocol::RunnerProfileV1::NoProviders
        );
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["input"]);
    }

    #[test]
    fn parse_run_args_selects_only_closed_profiles() {
        let values = args(&["--profile", "log-only", "demo.rss"]);
        let options = super::parse_run_args(&values).expect("known profile should parse");
        assert_eq!(
            options.profile,
            rsscript_runner_protocol::RunnerProfileV1::LogOnly
        );

        let error = super::parse_run_args(&args(&["--profile", "arbitrary-library", "demo.rss"]))
            .expect_err("unknown profile must fail closed");
        assert!(error.contains("unknown runner profile"));
    }

    #[test]
    fn trusted_in_process_cannot_widen_an_isolated_profile() {
        let error = super::parse_run_args(&args(&[
            "--trusted-in-process",
            "--profile",
            "log-only",
            "demo.rss",
        ]))
        .expect_err("mixed execution modes must be rejected");
        assert!(error.contains("cannot be combined"));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_execution_requires_explicit_trusted_in_process() {
        let error = super::parse_run_args(&args(&["--native", "demo.rss"]))
            .expect_err("native execution without trust declaration must fail");
        assert!(error.contains("requires explicit"));

        let values = args(&["--trusted-in-process", "--native", "demo.rss"]);
        let options =
            super::parse_run_args(&values).expect("trusted native execution should parse");
        assert!(options.trusted_in_process);
        assert!(options.native);
    }

    #[test]
    fn aot_flags_are_not_part_of_the_product_cli() {
        for flag in ["--aot", "--release", "--dry-run", "--out-dir"] {
            let error = super::parse_run_args(&args(&[flag, "demo.rss"]))
                .expect_err("experimental AOT option must not parse");
            assert!(error.contains("unknown argument"), "{flag}: {error}");
        }
    }
}
