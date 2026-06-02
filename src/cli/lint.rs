use std::fs;
use std::process::ExitCode;

use rsscript::{
    analyze_source_with_interfaces, analyze_source_with_interfaces_without_core,
    analyze_source_without_core, format_diagnostics_human, format_diagnostics_json, lint_source,
    standard_package_interfaces,
};

use super::check::parse_check_args;
use super::{print_usage, read_interface_sources};

pub(crate) fn run_lint(args: &[String]) -> ExitCode {
    let options = match parse_check_args(args) {
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

    let interfaces = match read_interface_sources(&options.interfaces) {
        Ok(interfaces) => interfaces,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let interface_refs = interfaces
        .iter()
        .map(|interface| (interface.path.as_str(), interface.contents.as_str()))
        .collect::<Vec<_>>();
    let mut diagnostics = if options.use_core {
        let mut combined = standard_package_interfaces().to_vec();
        combined.extend(interface_refs);
        analyze_source_with_interfaces(path, &source, &combined)
    } else if interface_refs.is_empty() {
        analyze_source_without_core(path, &source)
    } else {
        analyze_source_with_interfaces_without_core(path, &source, &interface_refs)
    };
    diagnostics.extend(lint_source(path, &source));

    if options.json {
        println!("{}", format_diagnostics_json(&diagnostics));
    } else if diagnostics.is_empty() {
        println!("{path}: lint ok");
    } else {
        print!("{}", format_diagnostics_human(&diagnostics));
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
