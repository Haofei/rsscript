use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript_compiler::{PackageAnalysis, analyze_package_dir, format_package_analysis_json};
use rsscript_sdk::{
    analysis::SemanticDiffV1,
    artifact::{
        ARTIFACT_BUNDLE_MAGIC, ArtifactBundle, ArtifactVerifier, BYTECODE_MAGIC, BuiltArtifact,
        BytecodeArtifact, BytecodeVerifier,
    },
    compile::Compiler,
    project::ProjectCompiler,
};
use serde_json::json;

use super::{is_package_directory, read_cli_source};

pub(crate) fn run_build(args: &[String]) -> ExitCode {
    let (input, output, analysis_output) = match parse_build_args(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(error),
    };
    let build = match build_input(input) {
        Ok(build) => build,
        Err(error) => {
            eprintln!("{error:?}");
            return ExitCode::from(1);
        }
    };
    let bytes = match build.bundle_bytes() {
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
    if analysis_output.is_some() {
        let analysis_output = analysis_output
            .map(PathBuf::from)
            .unwrap_or_else(|| default_analysis_path(&output));
        if let Some(parent) = analysis_output.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            eprintln!("cannot create {}: {error}", parent.display());
            return ExitCode::from(2);
        }
        let analysis = serde_json::to_string_pretty(build.analysis())
            .expect("Artifact Bundle analysis must serialize");
        if let Err(error) = fs::write(&analysis_output, format!("{analysis}\n")) {
            eprintln!("cannot write {}: {error}", analysis_output.display());
            return ExitCode::from(2);
        }
        println!("analysis: {}", analysis_output.display());
    }
    ExitCode::SUCCESS
}

fn build_input(input: &str) -> Result<BuiltArtifact, String> {
    let compiler = Compiler;
    if is_package_directory(input) {
        ProjectCompiler::new()
            .compile_package(Path::new(input))
            .map_err(|error| error.to_string())
    } else {
        let source = read_cli_source(Path::new(input))?;
        compiler
            .compile(input, &source)
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn run_verify(args: &[String]) -> ExitCode {
    let [input] = args else {
        return usage_error("usage: rss verify <artifact.rssbundle>".to_string());
    };
    let bytes = match fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot read {input}: {error}");
            return ExitCode::from(2);
        }
    };
    match ArtifactVerifier.verify_bytes(&bytes) {
        Ok(verified) => {
            println!("verified: {}", verified.bundle().digest());
            println!("module: {}", verified.module_digest());
            println!("interfaces: {}", verified.external_imports().len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("verification failed: {error}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn run_diff(args: &[String]) -> ExitCode {
    let (format, old, new) = match parse_diff_args(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(error),
    };
    let old = match bundle_from_input(old) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("cannot build old input: {error}");
            return ExitCode::from(1);
        }
    };
    let new = match bundle_from_input(new) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("cannot build new input: {error}");
            return ExitCode::from(1);
        }
    };
    let diff = SemanticDiffV1::between(&old, &new);
    match format {
        DiffFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&diff).expect("semantic diff serializes")
        ),
        DiffFormat::Markdown => print!("{}", diff.to_markdown()),
    }
    ExitCode::SUCCESS
}

fn bundle_from_input(input: &str) -> Result<ArtifactBundle, String> {
    let path = Path::new(input);
    if path.is_file() {
        let bytes = fs::read(path).map_err(|error| format!("cannot read {input}: {error}"))?;
        if bytes.starts_with(ARTIFACT_BUNDLE_MAGIC) {
            return ArtifactBundle::from_bytes(&bytes).map_err(|error| error.to_string());
        }
    }
    build_input(input).map(BuiltArtifact::into_bundle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffFormat {
    Json,
    Markdown,
}

fn parse_diff_args(args: &[String]) -> Result<(DiffFormat, &str, &str), String> {
    let mut format = DiffFormat::Markdown;
    let mut explicit_format = None;
    let mut inputs = Vec::new();
    for argument in args {
        match argument.as_str() {
            "--json" => {
                if explicit_format.replace(DiffFormat::Json).is_some() {
                    return Err("select exactly one diff output format".to_string());
                }
                format = DiffFormat::Json;
            }
            "--markdown" => {
                if explicit_format.replace(DiffFormat::Markdown).is_some() {
                    return Err("select exactly one diff output format".to_string());
                }
                format = DiffFormat::Markdown;
            }
            value if value.starts_with("--") => return Err(format!("unknown argument `{value}`")),
            value => inputs.push(value),
        }
    }
    let [old, new] = inputs.as_slice() else {
        return Err("usage: rss diff [--json|--markdown] <old> <new>".to_string());
    };
    Ok((format, old, new))
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
    let artifact = match load_or_compile(input) {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
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
    if view == "analysis" && Path::new(input).is_file() {
        return inspect_bundle_analysis(input);
    }
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

fn inspect_bundle_analysis(input: &str) -> ExitCode {
    let bytes = match fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot read {input}: {error}");
            return ExitCode::from(2);
        }
    };
    let bundle = match ArtifactBundle::from_bytes(&bytes) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("cannot decode Artifact Bundle: {error}");
            return ExitCode::from(1);
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(bundle.analysis()).expect("bundle analysis serializes")
    );
    ExitCode::SUCCESS
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

fn load_or_compile(input: &str) -> Result<BytecodeArtifact, String> {
    let path = Path::new(input);
    if path.is_file() {
        let bytes = fs::read(path).map_err(|error| format!("cannot read {input}: {error}"))?;
        if bytes.starts_with(ARTIFACT_BUNDLE_MAGIC) {
            let bundle = ArtifactBundle::from_bytes(&bytes).map_err(|error| error.to_string())?;
            return ArtifactVerifier
                .verify_bundle(bundle)
                .map(|verified| verified.bytecode_artifact().clone())
                .map_err(|error| error.to_string());
        }
        if bytes.starts_with(BYTECODE_MAGIC) {
            return BytecodeVerifier::default()
                .verify(&bytes)
                .map(|verified| verified.into_artifact())
                .map_err(|error| error.to_string());
        }
    }
    let built = build_input(input)?;
    ArtifactVerifier
        .verify(built)
        .map(|verified| verified.bytecode_artifact().clone())
        .map_err(|error| error.to_string())
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
    PathBuf::from("target").join(format!("{name}.rssbundle"))
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
            "demo.rssbundle",
            "--analysis-out",
            "demo.analysis.json",
        ]);
        assert_eq!(
            parse_build_args(&build).unwrap(),
            (
                "demo.rss",
                Some("demo.rssbundle"),
                Some("demo.analysis.json")
            )
        );
        let inspect = args(&["imports", "--json", "demo.rssbundle"]);
        assert_eq!(
            parse_inspect_args(&inspect).unwrap(),
            ("imports", true, "demo.rssbundle")
        );
        assert!(parse_build_args(&args(&["a.rss", "b.rss"])).is_err());
        assert_eq!(
            default_analysis_path(Path::new("target/demo.rssbundle")),
            PathBuf::from("target/demo.analysis.json")
        );
        assert_eq!(
            parse_diff_args(&args(&["--json", "old", "new"])).unwrap(),
            (DiffFormat::Json, "old", "new")
        );
        assert!(parse_diff_args(&args(&["old"])).is_err());
    }
}
