use std::fs;
use std::process::ExitCode;

use rsscript::{EvalError, eval_source_main, format_diagnostics_human};

use super::print_usage;

pub(crate) fn run_eval(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        print_usage();
        return ExitCode::from(2);
    };
    if args.len() > 1 {
        eprintln!("unexpected extra argument `{}`.", args[1]);
        return ExitCode::from(2);
    }
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            return ExitCode::from(2);
        }
    };
    match eval_source_main(path, &source) {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
            println!("{}", output.value);
            ExitCode::SUCCESS
        }
        Err(EvalError::Diagnostics(diagnostics)) => {
            print!("{}", format_diagnostics_human(&diagnostics));
            ExitCode::from(1)
        }
        Err(EvalError::Runtime(error)) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
