use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript_compiler::{
    PackageAnalysis, analyze_package_dir, format_package_analysis_json, load_workspace_snapshot,
};
use rsscript_sdk::{
    BYTECODE_MAGIC, EvalError, RegVmExecutable, reg_vm_compile_package_input, reg_vm_compile_source,
};
use serde_json::json;

use super::{is_package_directory, package_execution_lowering_input, read_cli_source};

pub(crate) fn run_build(args: &[String]) -> ExitCode {
    let (input, output, analysis_output) = match parse_build_args(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(error),
    };
    if analysis_output.is_some() && !is_package_directory(input) {
        return usage_error("`--analysis-out` requires a package directory".to_string());
    }
    let build = match build_input(input) {
        Ok(build) => build,
        Err(error) => {
            eprintln!("{error:?}");
            return ExitCode::from(1);
        }
    };
    let bytes = match build.executable.to_bytecode() {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{error:?}");
            return ExitCode::from(1);
        }
    };
    let output = output.map_or_else(|| default_artifact_path(input), PathBuf::from);
    if let Some(parent) = output.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("cannot create {}: {error}", parent.display());
        return ExitCode::from(2);
    }
    if let Err(error) = fs::write(&output, bytes) {
        eprintln!("cannot write {}: {error}", output.display());
        return ExitCode::from(2);
    }
    println!("{}", output.display());
    if let Some(analysis) = build.analysis {
        let analysis_output = analysis_output
            .map(PathBuf::from)
            .unwrap_or_else(|| default_analysis_path(&output));
        if let Some(parent) = analysis_output.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            eprintln!("cannot create {}: {error}", parent.display());
            return ExitCode::from(2);
        }
        if let Err(error) = fs::write(&analysis_output, format_package_analysis_json(&analysis)) {
            eprintln!("cannot write {}: {error}", analysis_output.display());
            return ExitCode::from(2);
        }
        println!("analysis: {}", analysis_output.display());
    }
    ExitCode::SUCCESS
}

struct BuildProduct {
    executable: RegVmExecutable,
    analysis: Option<PackageAnalysis>,
}

fn build_input(input: &str) -> Result<BuildProduct, EvalError> {
    if !is_package_directory(input) {
        return compile_input(input).map(|executable| BuildProduct {
            executable,
            analysis: None,
        });
    }

    let snapshot = load_workspace_snapshot(Path::new(input)).map_err(EvalError::Runtime)?;
    if snapshot.analysis().summary.errors != 0 {
        return Err(EvalError::Diagnostics(
            snapshot.analysis().diagnostics.clone(),
        ));
    }
    let mut executable = reg_vm_compile_package_input(snapshot.lowering_input())?;
    executable.bind_snapshot_digest(snapshot.digest())?;
    let mut analysis = snapshot.analysis().clone();
    analysis.module_digest = Some(
        executable
            .bytecode_artifact()
            .header
            .executable_hash
            .clone(),
    );
    Ok(BuildProduct {
        executable,
        analysis: Some(analysis),
    })
}

pub(crate) fn run_inspect(args: &[String]) -> ExitCode {
    let (view, json_output, input) = match parse_inspect_args(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(error),
    };
    match view {
        "bytecode" | "imports" => inspect_bytecode(view, json_output, input),
        "analysis" | "resources" | "async" | "call-graph" => {
            inspect_analysis(view, json_output, input)
        }
        _ => usage_error(format!("unknown inspect view `{view}`")),
    }
}

fn inspect_bytecode(view: &str, json_output: bool, input: &str) -> ExitCode {
    let executable = match load_or_compile(input) {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let artifact = executable.bytecode_artifact();
    if view == "imports" {
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&artifact.imports).expect("imports serialize")
            );
        } else if artifact.imports.is_empty() {
            println!("no external imports");
        } else {
            for import in &artifact.imports {
                println!(
                    "{} {} abi={}",
                    import.symbol,
                    import.signature_hash.as_str(),
                    import.abi_version
                );
            }
        }
        return ExitCode::SUCCESS;
    }

    let summary = json!({
        "schema": artifact.header.schema,
        "language_version": artifact.header.language_version,
        "runtime_abi_version": artifact.header.runtime_abi_version,
        "source_content_hash": artifact.header.source_content_hash,
        "executable_hash": artifact.header.executable_hash,
        "checksum": artifact.checksum,
        "imports": artifact.imports.len(),
        "payload_bytes": artifact.payload.len(),
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    } else {
        println!("schema: {}", artifact.header.schema);
        println!("language: {}", artifact.header.language_version);
        println!("runtime ABI: {}", artifact.header.runtime_abi_version);
        println!("source: {}", artifact.header.source_content_hash);
        println!("executable: {}", artifact.header.executable_hash);
        println!("checksum: {}", artifact.checksum);
        println!("imports: {}", artifact.imports.len());
        println!("payload bytes: {}", artifact.payload.len());
    }
    ExitCode::SUCCESS
}

fn inspect_analysis(view: &str, json_output: bool, input: &str) -> ExitCode {
    if !is_package_directory(input) {
        return usage_error(format!("`rss inspect {view}` requires a package directory"));
    }
    let analysis = match analyze_package_dir(Path::new(input)) {
        Ok(analysis) => analysis,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    if view == "analysis" {
        println!("{}", format_package_analysis_json(&analysis));
    } else if json_output {
        let value = match view {
            "resources" => resource_json(&analysis),
            "async" => json!({
                "async_apis": analysis.summary.async_apis,
                "await_sites": analysis.await_sites,
            }),
            "call-graph" => json!({ "external_calls": analysis.external_imports }),
            _ => unreachable!(),
        };
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        match view {
            "resources" => {
                println!("resource APIs: {}", analysis.summary.resource_apis);
                for export in resource_exports(&analysis) {
                    println!("{}: {}", export.name, export.semantic_facts.join(", "));
                }
            }
            "async" => {
                println!("async APIs: {}", analysis.summary.async_apis);
                for site in &analysis.await_sites {
                    println!(
                        "{} awaits {} (live: {})",
                        site.function,
                        site.callee.as_deref().unwrap_or("<expression>"),
                        site.live_across_await.join(", ")
                    );
                }
            }
            "call-graph" => {
                for call in &analysis.external_imports {
                    println!(
                        "{} -> {} via {}",
                        call.function,
                        call.symbol,
                        call.call_chain.join(" -> ")
                    );
                }
            }
            _ => unreachable!(),
        }
    }
    if analysis.summary.errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn resource_exports(
    analysis: &PackageAnalysis,
) -> impl Iterator<Item = &rsscript_compiler::PackageAnalysisExport> {
    analysis.exports.iter().filter(|export| {
        export
            .semantic_facts
            .iter()
            .any(|fact| fact.starts_with("resource "))
    })
}

fn resource_json(analysis: &PackageAnalysis) -> serde_json::Value {
    json!({
        "resource_apis": analysis.summary.resource_apis,
        "exports": resource_exports(analysis).collect::<Vec<_>>(),
    })
}

fn load_or_compile(input: &str) -> Result<RegVmExecutable, String> {
    let path = Path::new(input);
    if path.is_file() {
        let bytes = fs::read(path).map_err(|error| format!("cannot read {input}: {error}"))?;
        if bytes.starts_with(BYTECODE_MAGIC) {
            return RegVmExecutable::from_bytecode(&bytes).map_err(|error| format!("{error:?}"));
        }
    }
    compile_input(input).map_err(|error| format!("{error:?}"))
}

fn compile_input(input: &str) -> Result<RegVmExecutable, EvalError> {
    if is_package_directory(input) {
        let package =
            package_execution_lowering_input(Path::new(input)).map_err(EvalError::Runtime)?;
        reg_vm_compile_package_input(&package)
    } else {
        let source = read_cli_source(Path::new(input)).map_err(EvalError::Runtime)?;
        reg_vm_compile_source(input, &source)
    }
}

fn parse_build_args(args: &[String]) -> Result<(&str, Option<&str>, Option<&str>), String> {
    let mut input = None;
    let mut output = None;
    let mut analysis_output = None;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        match argument.as_str() {
            "--out" => {
                index += 1;
                output = Some(
                    args.get(index)
                        .ok_or_else(|| "missing value for `--out`".to_string())?
                        .as_str(),
                );
            }
            "--analysis-out" => {
                index += 1;
                analysis_output = Some(
                    args.get(index)
                        .ok_or_else(|| "missing value for `--analysis-out`".to_string())?
                        .as_str(),
                );
            }
            value if value.starts_with("--") => return Err(format!("unknown argument `{value}`")),
            value if input.is_none() => input = Some(value),
            value => return Err(format!("unexpected extra input `{value}`")),
        }
        index += 1;
    }
    Ok((
        input.ok_or_else(|| "missing build input".to_string())?,
        output,
        analysis_output,
    ))
}

fn parse_inspect_args(args: &[String]) -> Result<(&str, bool, &str), String> {
    let view = args
        .first()
        .ok_or_else(|| "missing inspect view".to_string())?;
    let mut json_output = false;
    let mut input = None;
    for argument in &args[1..] {
        if argument == "--json" {
            json_output = true;
        } else if argument.starts_with("--") {
            return Err(format!("unknown argument `{argument}`"));
        } else if input.is_none() {
            input = Some(argument.as_str());
        } else {
            return Err(format!("unexpected extra input `{argument}`"));
        }
    }
    Ok((
        view,
        json_output,
        input.ok_or_else(|| "missing inspect input".to_string())?,
    ))
}

fn default_artifact_path(input: &str) -> PathBuf {
    let path = Path::new(input);
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("package");
    PathBuf::from("target").join(format!("{name}.rssbc"))
}

fn default_analysis_path(artifact: &Path) -> PathBuf {
    artifact.with_extension("analysis.json")
}

fn usage_error(error: String) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn build_and_inspect_arguments_are_bounded() {
        let build = args(&[
            "demo.rss",
            "--out",
            "demo.rssbc",
            "--analysis-out",
            "demo.analysis.json",
        ]);
        assert_eq!(
            parse_build_args(&build).unwrap(),
            ("demo.rss", Some("demo.rssbc"), Some("demo.analysis.json"))
        );
        let inspect = args(&["imports", "--json", "demo.rssbc"]);
        assert_eq!(
            parse_inspect_args(&inspect).unwrap(),
            ("imports", true, "demo.rssbc")
        );
        assert!(parse_build_args(&args(&["a.rss", "b.rss"])).is_err());
        assert_eq!(
            default_analysis_path(Path::new("target/demo.rssbc")),
            PathBuf::from("target/demo.analysis.json")
        );
    }
}
