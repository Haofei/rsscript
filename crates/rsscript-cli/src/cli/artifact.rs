use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript_diagnostics::{Diagnostic, format_diagnostics_human};
use rsscript_sdk::{
    analysis::SemanticDiffV2,
    artifact::{
        ARTIFACT_BUNDLE_MAGIC, ArtifactBundle, ArtifactVerifier, BYTECODE_MAGIC, BuiltArtifact,
        BytecodeArtifact, BytecodeVerifier, VerifiedArtifact,
    },
    compile::{CompileError, Compiler},
    project::ProjectCompiler,
};
use serde_json::json;

use super::inputs::{InterfacePrelude, SourceInput};
use super::{is_package_directory, required_flag_value};

/// Why a build input could not be turned into an Artifact.
///
/// Compilation diagnostics are kept as diagnostics so `rss build` can render
/// them exactly as `rss check` does instead of printing a count.
pub(crate) enum InputError {
    Diagnostics(Vec<Diagnostic>),
    Message(String),
}

impl InputError {
    fn report(&self) {
        match self {
            Self::Diagnostics(diagnostics) => eprint!("{}", format_diagnostics_human(diagnostics)),
            Self::Message(message) => eprintln!("{message}"),
        }
    }
}

impl From<CompileError> for InputError {
    fn from(error: CompileError) -> Self {
        match error {
            CompileError::Diagnostics(diagnostics) => Self::Diagnostics(diagnostics),
            other => Self::Message(other.to_string()),
        }
    }
}

impl From<String> for InputError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl std::fmt::Display for InputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diagnostics(diagnostics) => {
                formatter.write_str(format_diagnostics_human(diagnostics).trim_end())
            }
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

pub(crate) fn run_build(args: &[String]) -> ExitCode {
    let options = match parse_build_args(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(error),
    };
    let BuildOptions {
        input,
        output,
        analysis_output,
        interfaces,
    } = options;
    let build = match build_input(input, &interfaces) {
        Ok(build) => build,
        Err(error) => {
            error.report();
            return ExitCode::from(1);
        }
    };
    let output = output.map_or_else(|| default_artifact_path(input), PathBuf::from);
    let verified = match verify_then_write(build.into_bundle(), &output) {
        Ok(verified) => verified,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(error.exit_code());
        }
    };
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
        let analysis =
            serde_json::to_string_pretty(verified.bundle().analysis_envelope().payload())
                .expect("Artifact Bundle analysis must serialize");
        if let Err(error) = fs::write(&analysis_output, format!("{analysis}\n")) {
            eprintln!("cannot write {}: {error}", analysis_output.display());
            return ExitCode::from(2);
        }
        println!("analysis: {}", analysis_output.display());
    }
    ExitCode::SUCCESS
}

/// Why a built Artifact did not become a file on disk.
///
/// The two halves are kept apart because they mean different things to a
/// caller: `Rejected` is the Artifact verifier refusing the bytes, `Write` is
/// the filesystem refusing the write. `rss build` reports them with the exit
/// codes it has always used (1 and 2).
#[derive(Debug)]
pub(crate) enum ArtifactWriteError {
    Rejected(String),
    Write(String),
}

impl ArtifactWriteError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Rejected(_) => 1,
            Self::Write(_) => 2,
        }
    }
}

impl std::fmt::Display for ArtifactWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(message) | Self::Write(message) => formatter.write_str(message),
        }
    }
}

/// Verify a built Artifact Bundle and, only if it verifies, write it out.
///
/// `rss build` promises verified bytecode, so the Artifact verifier runs here
/// rather than only in `rss run`/`rss inspect`. It runs before any filesystem
/// effect: a Bundle the verifier rejects leaves no file behind, not even a
/// truncated one or an empty parent directory, because nothing has been
/// created yet. That ordering is the whole point of this function, which is
/// why it is one function and is tested directly with a Bundle the verifier
/// refuses — a *source program* the checker accepts and the verifier rejects
/// would be a compiler bug, so this guarantee cannot be tested through one.
pub(crate) fn verify_then_write(
    bundle: ArtifactBundle,
    output: &Path,
) -> Result<VerifiedArtifact, ArtifactWriteError> {
    let verified = ArtifactVerifier
        .verify_bundle(bundle)
        .map_err(|error| ArtifactWriteError::Rejected(format!("verification failed: {error}")))?;
    let bytes = verified
        .bundle()
        .to_bytes()
        .map_err(|error| ArtifactWriteError::Rejected(format!("{error:?}")))?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ArtifactWriteError::Write(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    fs::write(output, bytes).map_err(|error| {
        ArtifactWriteError::Write(format!("cannot write {}: {error}", output.display()))
    })?;
    Ok(verified)
}

/// Compile one build input the same way `rss check` checks it.
///
/// A package directory carries its own declared interfaces, so `--interface`
/// is refused there exactly as it is for `rss check <package-directory>`. A
/// single file is compiled from the shared [`SourceInput`] assembly, which is
/// what keeps `build`, `run` and `inspect` on the same interfaces as `check`.
pub(crate) fn build_input(input: &str, interfaces: &[&str]) -> Result<BuiltArtifact, InputError> {
    if is_package_directory(input) {
        if !interfaces.is_empty() {
            return Err(InputError::Message(PACKAGE_INTERFACE_ERROR.to_string()));
        }
        return ProjectCompiler::new()
            .compile_package(Path::new(input))
            .map_err(InputError::from);
    }
    let source = SourceInput::read(input, interfaces, InterfacePrelude::StandardPackages)?;
    Compiler
        .compile_snapshot(&source.snapshot())
        .map_err(InputError::from)
}

pub(crate) const PREBUILT_INTERFACE_ERROR: &str = "`--interface` is only valid for source inputs; a prebuilt Artifact already records its resolved interfaces.";

pub(crate) const PACKAGE_INTERFACE_ERROR: &str = "`--interface` is only valid for single-file inputs; a package captures its declared interfaces from the project manifest.";

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
    let diff = SemanticDiffV2::between(&old, &new);
    match format {
        DiffFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&diff).expect("semantic diff serializes")
        ),
        DiffFormat::Markdown => print!("{}", diff.to_markdown()),
    }
    ExitCode::SUCCESS
}

fn bundle_from_input(input: &str) -> Result<ArtifactBundle, InputError> {
    let path = Path::new(input);
    if path.is_file() {
        let bytes = fs::read(path)
            .map_err(|error| InputError::Message(format!("cannot read {input}: {error}")))?;
        if bytes.starts_with(ARTIFACT_BUNDLE_MAGIC) {
            return ArtifactBundle::from_bytes(&bytes)
                .map_err(|error| InputError::Message(error.to_string()));
        }
    }
    build_input(input, &[]).map(BuiltArtifact::into_bundle)
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
    let InspectOptions {
        view,
        json_output,
        input,
        interfaces,
    } = match parse_inspect_args(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(error),
    };
    match view {
        "bytecode" | "imports" => inspect_bytecode(view, json_output, input, &interfaces),
        "analysis" | "resources" | "async" | "call-graph" => {
            inspect_analysis(view, json_output, input, &interfaces)
        }
        _ => usage_error(format!("unknown inspect view `{view}`")),
    }
}

fn inspect_bytecode(view: &str, json_output: bool, input: &str, interfaces: &[&str]) -> ExitCode {
    let artifact = match load_or_compile(input, interfaces) {
        Ok(artifact) => artifact,
        Err(error) => {
            error.report();
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

fn inspect_analysis(view: &str, json_output: bool, input: &str, interfaces: &[&str]) -> ExitCode {
    if view == "analysis" && Path::new(input).is_file() {
        return inspect_bundle_analysis(input);
    }
    let build = match build_input(input, interfaces) {
        Ok(build) => build,
        Err(error) => {
            error.report();
            return ExitCode::from(1);
        }
    };
    let envelope = build.analysis_envelope();
    let Some(analysis) = envelope.package_analysis() else {
        if view != "analysis" {
            return usage_error(format!(
                "`rss inspect {view}` requires package analysis evidence"
            ));
        }
        println!(
            "{}",
            if json_output {
                serde_json::to_string_pretty(envelope.payload())
            } else {
                serde_json::to_string(envelope.payload())
            }
            .expect("versioned analysis evidence serializes")
        );
        return ExitCode::SUCCESS;
    };
    if view == "analysis" {
        println!(
            "{}",
            if json_output {
                serde_json::to_string_pretty(analysis)
            } else {
                serde_json::to_string(analysis)
            }
            .expect("package analysis JSON serialization")
        );
    } else if json_output {
        let value = match view {
            "resources" => serde_json::json!({
                "resource_apis": analysis.summary.resource_apis,
                "lifetimes": analysis.resource_lifetimes,
                "transfers": analysis.resource_transfers,
            }),
            "async" => serde_json::json!({
                "async_apis": analysis.summary.async_apis,
                "await_sites": analysis.await_sites,
                "task_groups": analysis.task_groups,
            }),
            "call-graph" => serde_json::json!({
                "call_edges": analysis.call_edges,
                "external_calls": analysis.external_imports,
            }),
            _ => unreachable!(),
        };
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        match view {
            "resources" => {
                println!("resource APIs: {}", analysis.summary.resource_apis);
                for lifetime in &analysis.resource_lifetimes {
                    println!(
                        "{}: {} -> {}",
                        lifetime.function, lifetime.binding, lifetime.cleanup
                    );
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

fn load_or_compile(input: &str, interfaces: &[&str]) -> Result<BytecodeArtifact, InputError> {
    let path = Path::new(input);
    if path.is_file() {
        let bytes = fs::read(path)
            .map_err(|error| InputError::Message(format!("cannot read {input}: {error}")))?;
        if bytes.starts_with(ARTIFACT_BUNDLE_MAGIC) || bytes.starts_with(BYTECODE_MAGIC) {
            // A prebuilt Artifact already carries its resolved imports, so an
            // `--interface` here would silently do nothing.
            if !interfaces.is_empty() {
                return Err(InputError::Message(PREBUILT_INTERFACE_ERROR.to_string()));
            }
        }
        if bytes.starts_with(ARTIFACT_BUNDLE_MAGIC) {
            let bundle = ArtifactBundle::from_bytes(&bytes)
                .map_err(|error| InputError::Message(error.to_string()))?;
            return ArtifactVerifier
                .verify_bundle(bundle)
                .map(|verified| verified.bytecode_artifact().clone())
                .map_err(|error| InputError::Message(error.to_string()));
        }
        if bytes.starts_with(BYTECODE_MAGIC) {
            return BytecodeVerifier::default()
                .verify(&bytes)
                .map(|verified| verified.into_artifact())
                .map_err(|error| InputError::Message(error.to_string()));
        }
    }
    let built = build_input(input, interfaces)?;
    ArtifactVerifier
        .verify(built)
        .map(|verified| verified.bytecode_artifact().clone())
        .map_err(|error| InputError::Message(error.to_string()))
}

#[derive(Debug, PartialEq, Eq)]
struct BuildOptions<'a> {
    input: &'a str,
    output: Option<&'a str>,
    analysis_output: Option<&'a str>,
    interfaces: Vec<&'a str>,
}

fn parse_build_args(args: &[String]) -> Result<BuildOptions<'_>, String> {
    let mut input = None;
    let mut output = None;
    let mut analysis_output = None;
    let mut interfaces = Vec::new();
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
            "--interface" => {
                index += 1;
                interfaces.push(required_flag_value(args, index, "--interface")?);
            }
            value if value.starts_with("--") => return Err(format!("unknown argument `{value}`")),
            value if input.is_none() => input = Some(value),
            value => return Err(format!("unexpected extra input `{value}`")),
        }
        index += 1;
    }
    Ok(BuildOptions {
        input: input.ok_or_else(|| "missing build input".to_string())?,
        output,
        analysis_output,
        interfaces,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct InspectOptions<'a> {
    view: &'a str,
    json_output: bool,
    input: &'a str,
    interfaces: Vec<&'a str>,
}

fn parse_inspect_args(args: &[String]) -> Result<InspectOptions<'_>, String> {
    let view = args
        .first()
        .ok_or_else(|| "missing inspect view".to_string())?;
    let mut json_output = false;
    let mut input = None;
    let mut interfaces = Vec::new();
    let mut index = 1;
    while let Some(argument) = args.get(index) {
        if argument == "--json" {
            json_output = true;
        } else if argument == "--interface" {
            index += 1;
            interfaces.push(required_flag_value(args, index, "--interface")?);
        } else if argument.starts_with("--") {
            return Err(format!("unknown argument `{argument}`"));
        } else if input.is_none() {
            input = Some(argument.as_str());
        } else {
            return Err(format!("unexpected extra input `{argument}`"));
        }
        index += 1;
    }
    Ok(InspectOptions {
        view,
        json_output,
        input: input.ok_or_else(|| "missing inspect input".to_string())?,
        interfaces,
    })
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
            "--interface",
            "host.rssi",
        ]);
        assert_eq!(
            parse_build_args(&build).unwrap(),
            BuildOptions {
                input: "demo.rss",
                output: Some("demo.rssbundle"),
                analysis_output: Some("demo.analysis.json"),
                interfaces: vec!["host.rssi"],
            }
        );
        let inspect = args(&[
            "imports",
            "--json",
            "--interface",
            "host.rssi",
            "demo.rssbundle",
        ]);
        assert_eq!(
            parse_inspect_args(&inspect).unwrap(),
            InspectOptions {
                view: "imports",
                json_output: true,
                input: "demo.rssbundle",
                interfaces: vec!["host.rssi"],
            }
        );
        assert!(parse_build_args(&args(&["a.rss", "b.rss"])).is_err());
        assert!(parse_build_args(&args(&["--interface", "--out", "a.rss"])).is_err());
        assert!(parse_inspect_args(&args(&["imports", "--interface"])).is_err());
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

    /// `rss build` writes nothing when the Artifact verifier refuses the
    /// Bundle, and it refuses it before the output path exists at all.
    ///
    /// The rejected Bundle is built here rather than compiled from source on
    /// purpose. `rss check` and `rss build` run the same compiler, so a source
    /// program the checker accepts and the verifier rejects is a compiler bug
    /// by definition; a test that needed one would be a test that decays the
    /// moment the bug is fixed. What this gate actually protects is the
    /// ordering inside [`verify_then_write`], so the test hands it an Artifact
    /// whose executable payload no longer matches the digest its header
    /// commits to — exactly what a corrupted or tampered build would look
    /// like.
    #[test]
    fn a_bundle_the_verifier_rejects_is_never_written() {
        let built = Compiler
            .compile("main.rss", "fn main() -> Int {\n    return 1\n}\n")
            .expect("the fixture program compiles");
        let mut artifact =
            BytecodeArtifact::from_bytes(built.artifact_bytes()).expect("built bytecode decodes");
        let last = artifact.payload.len() - 1;
        artifact.payload[last] ^= 0xff;
        let bundle = ArtifactBundle::new(
            artifact.to_bytes().expect("corrupted bytecode re-encodes"),
            built.analysis_envelope().clone(),
        )
        .expect("a Bundle still forms around the corrupted executable");

        let temp = tempfile::tempdir().expect("temp dir");
        let directory = temp.path().join("out");
        let output = directory.join("main.rssbundle");
        let error =
            verify_then_write(bundle, &output).expect_err("the verifier must refuse this Bundle");
        assert!(
            matches!(error, ArtifactWriteError::Rejected(_)),
            "{error:?} must be a verification refusal, not a write failure"
        );
        assert_eq!(error.exit_code(), 1);
        assert!(
            error.to_string().starts_with("verification failed: ")
                && error.to_string().contains("executable hash mismatch"),
            "the refusal must carry the verifier's own message: {error}"
        );
        assert!(
            !output.exists(),
            "a refused build must not leave a bundle behind"
        );
        assert!(
            !directory.exists(),
            "a refused build must not create the output directory either"
        );
    }

    /// The write half of the same function: a Bundle the verifier accepts is
    /// written, and the output directory is created for it.
    #[test]
    fn a_verified_bundle_is_written_to_its_output_path() {
        let built = Compiler
            .compile("main.rss", "fn main() -> Int {\n    return 1\n}\n")
            .expect("the fixture program compiles");
        let temp = tempfile::tempdir().expect("temp dir");
        let output = temp.path().join("out").join("main.rssbundle");
        verify_then_write(built.into_bundle(), &output).expect("a verified Bundle is written");
        let written = fs::read(&output).expect("the bundle exists");
        assert!(written.starts_with(ARTIFACT_BUNDLE_MAGIC));
    }
}
