use std::fs;
use std::process::ExitCode;

use rsscript::{analyze_source, format_diagnostics_human, format_source};

use super::{parse_path_args, print_usage};

pub(crate) fn run_fmt(args: &[String]) -> ExitCode {
    let (_, path) = match parse_path_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(path) = path else {
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

    let diagnostics = analyze_source(path, &source);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        print!("{}", format_diagnostics_human(&diagnostics));
        return ExitCode::from(1);
    }

    print!("{}", format_source(path, &source));
    ExitCode::SUCCESS
}
