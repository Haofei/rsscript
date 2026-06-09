use std::fs;
use std::process::ExitCode;

use rsscript::{
    EvalError, NativeValue, format_diagnostics_human, format_diagnostics_json,
    reg_vm_eval_source_main_with_args,
};

use super::print_usage;

#[derive(Debug)]
struct EvalOptions<'a> {
    json: bool,
    path: Option<&'a str>,
    program_args: Vec<&'a str>,
}

fn parse_eval_args(args: &[String]) -> Result<EvalOptions<'_>, String> {
    let mut json = false;
    let mut path = None;
    let mut program_args = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--" {
            program_args.extend(args[index + 1..].iter().map(String::as_str));
            break;
        } else if arg == "--json" {
            json = true;
        } else if arg.starts_with("--") && path.is_none() {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            program_args.push(arg.as_str());
        }
        index += 1;
    }

    Ok(EvalOptions {
        json,
        path,
        program_args,
    })
}

pub(crate) fn run_eval(args: &[String]) -> ExitCode {
    let options = match parse_eval_args(args) {
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
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            return ExitCode::from(2);
        }
    };
    match reg_vm_eval_source_main_with_args(path, &source, options.program_args.iter().copied()) {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
            // A `main` returning `Err` is a failed run (ledger SH-005): report it
            // to stderr and exit non-zero, matching the AOT backend (a runnable
            // `main` is `Unit` or `Result<Unit, E>`, so an `Err` variant here is
            // unambiguously the failure case).
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
            if options.json {
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

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_eval_args_accepts_json_path_and_program_args() {
        let values = args(&["--json", "demo.rss", "--", "a", "b"]);
        let options = super::parse_eval_args(&values).expect("eval args should parse");

        assert!(options.json);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["a", "b"]);
    }

    #[test]
    fn parse_eval_args_treats_extra_values_as_program_args() {
        let values = args(&["demo.rss", "a", "b"]);
        let options = super::parse_eval_args(&values).expect("args should parse");

        assert!(!options.json);
        assert_eq!(options.path, Some("demo.rss"));
        assert_eq!(options.program_args, vec!["a", "b"]);
    }

    #[test]
    fn parse_eval_args_rejects_unknown_option_before_path() {
        assert_eq!(
            super::parse_eval_args(&args(&["--release", "demo.rss"]))
                .expect_err("unknown option should fail"),
            "unknown argument `--release`."
        );
    }
}
