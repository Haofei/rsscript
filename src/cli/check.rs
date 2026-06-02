use std::fs;
use std::process::ExitCode;

use rsscript::{
    analyze_source_with_interfaces, analyze_source_with_interfaces_without_core,
    analyze_source_without_core, explain_diagnostic_code, format_diagnostic_explanation,
    format_diagnostics_human, format_diagnostics_json, standard_package_interfaces,
};

use super::package::run_package_check;
use super::{is_package_directory, print_usage, read_interface_sources, required_flag_value};

fn parse_explain_args(args: &[String]) -> Option<&str> {
    let [flag, code] = args else {
        return None;
    };
    (flag == "--explain").then_some(code.as_str())
}
#[derive(Debug)]
pub(crate) struct CheckOptions<'a> {
    pub(crate) json: bool,
    pub(crate) use_core: bool,
    pub(crate) path: Option<&'a str>,
    pub(crate) interfaces: Vec<&'a str>,
}

pub(crate) fn parse_check_args(args: &[String]) -> Result<CheckOptions<'_>, String> {
    let mut json = false;
    let mut use_core = true;
    let mut path = None;
    let mut interfaces = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--core" {
            use_core = true;
        } else if arg == "--no-core" {
            use_core = false;
        } else if arg == "--interface" {
            index += 1;
            let interface = required_flag_value(args, index, "--interface")?;
            interfaces.push(interface);
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
        index += 1;
    }

    Ok(CheckOptions {
        json,
        use_core,
        path,
        interfaces,
    })
}

fn package_check_option_error(options: &CheckOptions<'_>) -> Option<String> {
    if !options.use_core {
        return Some(
            "`rss check --no-core` is only valid for single-file checks; package checks use package interfaces and dependencies.".to_string(),
        );
    }
    if !options.interfaces.is_empty() {
        return Some(
            "`rss check --interface` is only valid for single-file checks; package checks read interfaces from rsspkg.toml.".to_string(),
        );
    }
    None
}
pub(crate) fn run_check(args: &[String]) -> ExitCode {
    if let Some(code) = parse_explain_args(args) {
        let Some(explanation) = explain_diagnostic_code(code) else {
            eprintln!("unknown diagnostic code: {code}");
            return ExitCode::from(2);
        };
        print!("{}", format_diagnostic_explanation(explanation));
        return ExitCode::SUCCESS;
    }

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

    if is_package_directory(path) {
        if let Some(error) = package_check_option_error(&options) {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
        return run_package_check(options.json, path);
    }

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
    let diagnostics = if options.use_core {
        let mut combined = standard_package_interfaces().to_vec();
        combined.extend(interface_refs);
        analyze_source_with_interfaces(path, &source, &combined)
    } else if interface_refs.is_empty() {
        analyze_source_without_core(path, &source)
    } else {
        analyze_source_with_interfaces_without_core(path, &source, &interface_refs)
    };
    if options.json {
        println!("{}", format_diagnostics_json(&diagnostics));
    } else if diagnostics.is_empty() {
        println!("{path}: ok");
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

#[cfg(test)]
mod tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_check_args_rejects_missing_interface_value() {
        let values = args(&["--interface", "--json", "demo.rss"]);
        let error = super::parse_check_args(&values).expect_err("missing interface should fail");

        assert_eq!(error, "missing value for `--interface`.");
    }

    #[test]
    fn package_check_options_reject_single_file_flags() {
        let values = args(&["--no-core", "package"]);
        let options = super::parse_check_args(&values).expect("arguments should parse");
        let error = super::package_check_option_error(&options)
            .expect("package check should reject no-core");

        assert!(error.contains("--no-core"));

        let values = args(&["--interface", "api.rssi", "package"]);
        let options = super::parse_check_args(&values).expect("arguments should parse");
        let error = super::package_check_option_error(&options)
            .expect("package check should reject explicit interface");

        assert!(error.contains("--interface"));
    }
}
