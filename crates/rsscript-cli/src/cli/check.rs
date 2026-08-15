use std::fs;
use std::process::ExitCode;

use rsscript_diagnostics::{
    explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_human,
    format_diagnostics_json_with_source,
};
use rsscript_semantics::{
    CompilationSession, analyze_source_with_interfaces,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    standard_package_interfaces,
};
use rsscript_syntax::lint_source;

use super::{is_package_directory, print_usage, read_interface_sources, required_flag_value};
#[cfg(feature = "execution")]
use rsscript_sdk::{compile::CompileError, project::ProjectCompiler};

/// Parse `--explain <CODE>` (optionally with `--json`), in any order.
fn parse_explain_args(args: &[String]) -> Option<(&str, bool)> {
    let mut saw_explain = false;
    let mut json = false;
    let mut code = None;
    for arg in args {
        match arg.as_str() {
            "--explain" => saw_explain = true,
            "--json" => json = true,
            other if !other.starts_with("--") => code = Some(other),
            _ => return None,
        }
    }
    saw_explain.then(|| code.map(|code| (code, json))).flatten()
}
#[derive(Debug)]
pub(crate) struct CheckOptions<'a> {
    pub(crate) json: bool,
    pub(crate) use_core: bool,
    pub(crate) lint: bool,
    pub(crate) path: Option<&'a str>,
    pub(crate) interfaces: Vec<&'a str>,
}

pub(crate) fn parse_check_args(args: &[String]) -> Result<CheckOptions<'_>, String> {
    let mut json = false;
    let mut use_core = true;
    let mut lint = false;
    let mut path = None;
    let mut interfaces = Vec::new();
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        if arg == "--json" {
            json = true;
        } else if arg == "--lint" {
            lint = true;
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
        lint,
        path,
        interfaces,
    })
}

fn package_check_option_error(options: &CheckOptions<'_>) -> Option<String> {
    if options.lint {
        return Some(
            "`rss check --lint` is only valid for single-file checks; package checks compile one immutable project snapshot."
                .to_string(),
        );
    }
    if !options.use_core {
        return Some(
            "`rss check --no-core` is only valid for single-file checks; package checks use package interfaces and dependencies.".to_string(),
        );
    }
    if !options.interfaces.is_empty() {
        return Some(
            "`rss check --interface` is only valid for single-file checks; package checks capture declared interfaces from the project manifest.".to_string(),
        );
    }
    None
}

#[cfg(feature = "execution")]
fn run_package_check(json: bool, path: &str) -> ExitCode {
    match ProjectCompiler::new().compile_package(std::path::Path::new(path)) {
        Ok(build) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&build.analysis_envelope().payload())
                        .expect("versioned source analysis serializes")
                );
            } else {
                println!("{path}: ok");
            }
            ExitCode::SUCCESS
        }
        Err(CompileError::Diagnostics(diagnostics)) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&diagnostics).expect("compiler diagnostics serialize")
                );
            } else {
                print!("{}", format_diagnostics_human(&diagnostics));
            }
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
pub(crate) fn run_check(args: &[String]) -> ExitCode {
    if let Some((code, json)) = parse_explain_args(args) {
        let Some(explanation) = explain_diagnostic_code(code) else {
            eprintln!("unknown diagnostic code: {code}");
            return ExitCode::from(2);
        };
        if json {
            // Machine-readable explanation for agents. (Per-diagnostic `fixes`
            // with applicability are already in `rss check --json` output.)
            println!(
                "{}",
                serde_json::to_string(&explanation).expect("diagnostic explanation serializes")
            );
        } else {
            print!("{}", format_diagnostic_explanation(explanation));
        }
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
        #[cfg(feature = "execution")]
        return run_package_check(options.json, path);
        #[cfg(not(feature = "execution"))]
        {
            eprintln!(
                "package checks require the `execution` feature; single-file checks are frontend-only"
            );
            return ExitCode::from(2);
        }
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
    let mut diagnostics = if options.use_core {
        let mut combined = standard_package_interfaces().to_vec();
        combined.extend(interface_refs);
        analyze_source_with_session(path, &source, &combined)
    } else {
        analyze_source_without_core_with_session(path, &source, &interface_refs)
    };
    if options.lint {
        diagnostics.extend(lint_source(path, &source));
    }
    if options.json {
        println!(
            "{}",
            format_diagnostics_json_with_source(&source, &diagnostics)
        );
    } else if diagnostics.is_empty() {
        println!("{}: {}", path, if options.lint { "lint ok" } else { "ok" });
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

/// Route the normal single-file check through the semantic-owned session
/// query. CLI file I/O stays at this composition boundary, while parse,
/// resolve, type, HIR, and diagnostic facts share one immutable input snapshot
/// below it.
fn analyze_source_with_session(
    path: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<rsscript_diagnostics::Diagnostic> {
    // Session files have stable path identities. Preserve the legacy analyzer's
    // duplicate-interface diagnostics rather than silently replacing one input
    // buffer when a caller supplied the same logical interface path twice.
    let unique_paths = interfaces
        .iter()
        .map(|(interface_path, _)| *interface_path)
        .collect::<std::collections::BTreeSet<_>>();
    if unique_paths.len() != interfaces.len() {
        return analyze_source_with_interfaces(path, source, interfaces);
    }
    let mut session = CompilationSession::default();
    session
        .set_file(path, source)
        .expect("CLI source path must be a valid session path");
    for (interface_path, interface_source) in interfaces {
        session
            .set_interface(*interface_path, *interface_source)
            .expect("CLI interface path must be a valid session path");
    }
    session.workspace_analysis().diagnostics().to_vec()
}

/// The no-core mode is still a normal immutable source/interface workspace:
/// it differs only in which interface snapshot the CLI supplies. Keep its
/// duplicate-input fallback separate so historical diagnostics are preserved
/// without letting an ordinary no-core check bypass the session query.
fn analyze_source_without_core_with_session(
    path: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<rsscript_diagnostics::Diagnostic> {
    let unique_paths = interfaces
        .iter()
        .map(|(interface_path, _)| *interface_path)
        .collect::<std::collections::BTreeSet<_>>();
    if unique_paths.len() != interfaces.len() {
        return if interfaces.is_empty() {
            analyze_source_without_core(path, source)
        } else {
            analyze_source_with_interfaces_without_core(path, source, interfaces)
        };
    }
    let mut session = CompilationSession::without_core();
    session
        .set_file(path, source)
        .expect("CLI source path must be a valid session path");
    for (interface_path, interface_source) in interfaces {
        session
            .set_interface(*interface_path, *interface_source)
            .expect("CLI interface path must be a valid session path");
    }
    session.workspace_analysis().diagnostics().to_vec()
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

        let values = args(&["--lint", "package"]);
        let options = super::parse_check_args(&values).expect("arguments should parse");
        let error =
            super::package_check_option_error(&options).expect("package check should reject lint");

        assert!(error.contains("--lint"));
    }

    #[test]
    fn default_core_check_uses_the_session_owned_workspace_analysis() {
        let diagnostics = super::analyze_source_with_session(
            "main.rss",
            "fn main() -> Int { return Host.value() }",
            &[("host.rssi", "module Host\npub fn value() -> Int\n")],
        );
        assert!(
            diagnostics.is_empty(),
            "session-owned analysis should retain explicit interface visibility: {diagnostics:#?}"
        );
    }

    #[test]
    fn duplicate_interface_paths_preserve_the_legacy_analysis_behavior() {
        let source = "fn main() -> Int { return Host.value() }";
        let interfaces = [
            ("host.rssi", "module Host\npub fn value() -> Int\n"),
            ("host.rssi", "module Host\npub fn value() -> String\n"),
        ];
        assert_eq!(
            super::analyze_source_with_session("main.rss", source, &interfaces),
            rsscript_semantics::analyze_source_with_interfaces("main.rss", source, &interfaces)
        );
    }

    #[test]
    fn no_core_check_uses_the_session_owned_workspace_analysis() {
        let diagnostics = super::analyze_source_without_core_with_session(
            "main.rss",
            "fn main() -> Int { return Host.value() }",
            &[("host.rssi", "module Host\npub fn value() -> Int\n")],
        );
        assert!(
            diagnostics.is_empty(),
            "session-owned no-core analysis should retain explicit interfaces: {diagnostics:#?}"
        );
    }

    #[test]
    fn no_core_duplicate_interfaces_preserve_legacy_analysis_behavior() {
        let source = "fn main() -> Int { return Host.value() }";
        let interfaces = [
            ("host.rssi", "module Host\npub fn value() -> Int\n"),
            ("host.rssi", "module Host\npub fn value() -> String\n"),
        ];
        assert_eq!(
            super::analyze_source_without_core_with_session("main.rss", source, &interfaces),
            rsscript_semantics::analyze_source_with_interfaces_without_core(
                "main.rss",
                source,
                &interfaces,
            ),
        );
    }
}
