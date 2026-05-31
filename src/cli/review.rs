use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript::{
    Diagnostic, ReviewMap, analyze_source, analyze_sources_with_interfaces,
    format_diagnostics_human, format_diagnostics_json, format_package_review_reir_json,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    review_map_sources, review_package_dir, review_sources,
};

use super::{is_package_directory, print_diagnostics, print_usage};

pub(crate) fn run_review(args: &[String]) -> ExitCode {
    let command = match parse_review_args(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(2);
        }
    };
    match command {
        ReviewCommand::Diff {
            json,
            old_path,
            new_path,
        } => run_review_diff(json, old_path, new_path),
        ReviewCommand::Map { json, path } => run_review_map(json, path),
    }
}

pub(crate) fn run_review_pr(args: &[String]) -> ExitCode {
    let mut head: Option<String> = None;
    let mut base: Option<String> = None;
    let mut grants: Option<String> = None;
    let mut target: Option<String> = None;
    let mut format = "markdown".to_string();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--head" => {
                index += 1;
                head = args.get(index).cloned();
            }
            "--base" => {
                index += 1;
                base = args.get(index).cloned();
            }
            "--grants" => {
                index += 1;
                grants = args.get(index).cloned();
            }
            "--target" => {
                index += 1;
                target = args.get(index).cloned();
            }
            "--format" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    format = value.clone();
                }
            }
            other => {
                if head.is_none() && !other.starts_with('-') {
                    head = Some(other.to_string());
                } else {
                    eprintln!("unknown review-pr flag: {other}");
                    print_usage();
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(head_path) = head else {
        eprintln!("review-pr requires --head <package-directory>");
        print_usage();
        return ExitCode::from(2);
    };
    let Some(grants_path) = grants else {
        eprintln!("review-pr requires --grants <grants.reir.json>");
        print_usage();
        return ExitCode::from(2);
    };

    // Step 1: Run package review on head to get required capabilities
    let head_dir = Path::new(&head_path);
    let review = match review_package_dir(head_dir) {
        Ok(review) => review,
        Err(error) => {
            eprintln!("package review failed: {error}");
            return ExitCode::from(1);
        }
    };

    // Step 2: Convert to REIR bundle (required facts)
    let review_reir_json = format_package_review_reir_json(&review);
    let required_bundle: reir::Bundle = match serde_json::from_str(&review_reir_json) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("failed to parse review REIR: {error}");
            return ExitCode::from(1);
        }
    };

    // Step 3: Load granted capabilities
    let granted_bundle: reir::Bundle = match fs::read_to_string(&grants_path) {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(bundle) => bundle,
            Err(error) => {
                eprintln!("failed to parse grants file: {error}");
                return ExitCode::from(1);
            }
        },
        Err(error) => {
            eprintln!("failed to read grants file {grants_path}: {error}");
            return ExitCode::from(1);
        }
    };

    // Step 4: Reconcile
    let reconciliations = reir::reconcile_capabilities_for_target(
        &required_bundle.facts,
        &granted_bundle.facts,
        target.as_deref(),
    );

    // Step 5: If base provided, compute diff as well
    let _base_review = base
        .as_ref()
        .and_then(|base_path| review_package_dir(Path::new(base_path)).ok());

    // Step 6: Format output
    let has_missing = reconciliations
        .iter()
        .any(|r| r.kind == reir::ReconciliationKind::MissingCapability);

    match format.as_str() {
        "markdown" => {
            let comment = reir::format_pr_review_comment(
                &required_bundle.facts,
                &granted_bundle.facts,
                &reconciliations,
            );
            print!("{comment}");
        }
        "ci-json" => {
            let ci_output = reir::format_ci_gate_output(
                &required_bundle.facts,
                &granted_bundle.facts,
                &reconciliations,
            );
            println!("{}", reir::format_ci_gate_json(&ci_output));
        }
        "sarif" => {
            let sarif = format_sarif_from_reconciliations(&required_bundle.facts, &reconciliations);
            println!("{sarif}");
        }
        other => {
            eprintln!("unknown format: {other}; use markdown, ci-json, or sarif");
            return ExitCode::from(2);
        }
    }

    if has_missing {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Contract review: produce REIR bundle from .rssi interface contracts.
/// This allows existing Rust projects to declare their semantic boundaries
/// without rewriting to RSScript.
pub(crate) fn run_contract_review(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut reir = false;
    let mut paths: Vec<&str> = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--reir" => reir = true,
            _ if !arg.starts_with('-') => paths.push(arg.as_str()),
            other => {
                eprintln!("unknown contract flag: {other}");
                eprintln!("usage: rss contract [--json|--reir] <file.rssi ...>");
                return ExitCode::from(2);
            }
        }
    }

    if paths.is_empty() {
        eprintln!("usage: rss contract [--json|--reir] <file.rssi ...>");
        return ExitCode::from(2);
    }

    // Read all .rssi sources
    let mut sources: Vec<(String, String)> = Vec::new();
    for path in &paths {
        match fs::read_to_string(path) {
            Ok(source) => sources.push((path.to_string(), source)),
            Err(error) => {
                eprintln!("failed to read {path}: {error}");
                return ExitCode::from(1);
            }
        }
    }

    // Produce BBOM from the contracts
    let source_refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(f, s)| (f.as_str(), s.as_str()))
        .collect();
    let bom = rsscript::bbom::behavior_bom_sources(source_refs);

    if reir {
        // Convert BBOM to REIR bundle
        let bundle = rsscript::bbom::behavior_bom_to_reir_bundle(&bom, &paths);
        match serde_json::to_string_pretty(&bundle) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("failed to serialize REIR bundle: {error}");
                return ExitCode::from(1);
            }
        }
    } else if json {
        println!("{}", rsscript::bbom::format_bbom_json(&bom));
    } else {
        // Human-readable contract summary
        print_contract_summary(&bom, &paths);
    }

    ExitCode::SUCCESS
}

fn print_contract_summary(bom: &rsscript::bbom::BehaviorBom, paths: &[&str]) {
    println!("Contract Review: {} interface(s)", paths.len());
    println!("─────────────────────────────────────────────");
    for path in paths {
        println!("  {path}");
    }
    println!();
    println!("Declared behaviors:");
    println!("  Mutations:         {}", bom.mutations.len());
    println!("  Retentions:        {}", bom.retentions.len());
    println!("  Resources:         {}", bom.resources.len());
    println!("  Native boundaries: {}", bom.native_boundaries.len());
    println!("  Capabilities:      {}", bom.capabilities.len());
    println!();
    let summary = &bom.summary;
    println!("Summary:");
    println!("  Total functions:   {}", summary.total_functions);
    println!("  Review-required:   {}", summary.review_required_functions);
    println!("  Unknown risk:      {}", summary.unknown_functions);
    println!();
    if !bom.mutations.is_empty() {
        println!("Mutations:");
        for m in &bom.mutations {
            println!("  {} in {}", m.target, m.function);
        }
        println!();
    }
    if !bom.retentions.is_empty() {
        println!("Retentions:");
        for r in &bom.retentions {
            println!("  {} in {}", r.parameter, r.function);
        }
        println!();
    }
    if !bom.native_boundaries.is_empty() {
        println!("Native boundaries:");
        for n in &bom.native_boundaries {
            println!("  {}", n.function);
        }
        println!();
    }
    println!("Use --reir to produce a REIR bundle for CI integration.");
}
fn format_sarif_from_reconciliations(
    required_facts: &[reir::Fact],
    reconciliations: &[reir::Reconciliation],
) -> String {
    let results: Vec<serde_json::Value> = reconciliations
        .iter()
        .filter(|r| r.kind == reir::ReconciliationKind::MissingCapability)
        .enumerate()
        .map(|(i, r)| {
            let capability_label = r
                .capability
                .as_ref()
                .and_then(|c| c.action.as_deref())
                .unwrap_or("unknown");
            let resource_label = r
                .capability
                .as_ref()
                .and_then(|c| c.resource.as_deref())
                .unwrap_or("unknown");

            // Find evidence from the related required fact
            let (file, line) = r
                .required_fact
                .as_deref()
                .and_then(|id| required_facts.iter().find(|f| f.id == id))
                .and_then(|f| f.evidence.first())
                .map(|e| {
                    (
                        e.file.clone().unwrap_or_default(),
                        e.line.unwrap_or(1),
                    )
                })
                .unwrap_or_default();

            serde_json::json!({
                "ruleId": "reir/missing-capability",
                "ruleIndex": 0,
                "level": "error",
                "message": {
                    "text": format!("Required capability {capability_label} on {resource_label} is not granted by deployment")
                },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                        "region": { "startLine": line }
                    }
                }],
                "properties": {
                    "reir.capability": capability_label,
                    "reir.resource": resource_label,
                    "reir.index": i
                }
            })
        })
        .collect();

    let sarif = serde_json::json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "rsscript-reir",
                    "version": "0.1.0",
                    "informationUri": "https://github.com/Haofei/rsscript",
                    "rules": [{
                        "id": "reir/missing-capability",
                        "shortDescription": { "text": "Required deployment capability not granted" },
                        "helpUri": "https://github.com/Haofei/rsscript#reir"
                    }]
                }
            },
            "results": results
        }]
    });
    serde_json::to_string_pretty(&sarif).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}
#[derive(Debug)]
pub(crate) enum ReviewCommand<'a> {
    Diff {
        json: bool,
        old_path: &'a str,
        new_path: &'a str,
    },
    Map {
        json: bool,
        path: &'a str,
    },
}

fn parse_review_args(args: &[String]) -> Result<ReviewCommand<'_>, String> {
    let mut json = false;
    let mut command = None;
    let mut paths = Vec::new();

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg == "--diff" || arg == "--map" {
            if command.is_some() {
                return Err(format!("unexpected review command `{arg}`."));
            }
            command = Some(arg.as_str());
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else {
            paths.push(arg.as_str());
        }
    }

    match (command, paths.as_slice()) {
        (Some("--map"), [path]) => Ok(ReviewCommand::Map { json, path }),
        (Some("--diff") | None, [old_path, new_path]) => Ok(ReviewCommand::Diff {
            json,
            old_path,
            new_path,
        }),
        _ => Err("invalid review arguments.".to_string()),
    }
}

fn run_review_diff(json: bool, old_path: &str, new_path: &str) -> ExitCode {
    let old_source = match fs::read_to_string(old_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {old_path}: {error}");
            return ExitCode::from(2);
        }
    };
    let new_source = match fs::read_to_string(new_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {new_path}: {error}");
            return ExitCode::from(2);
        }
    };

    let old_diagnostics = analyze_source(old_path, &old_source);
    let new_diagnostics = analyze_source(new_path, &new_source);
    let has_errors = old_diagnostics
        .iter()
        .chain(new_diagnostics.iter())
        .any(|diagnostic| diagnostic.severity.is_error());
    if has_errors {
        if json {
            let mut diagnostics = old_diagnostics;
            diagnostics.extend(new_diagnostics);
            println!("{}", format_diagnostics_json(&diagnostics));
        } else {
            print!("{}", format_diagnostics_human(&old_diagnostics));
            print!("{}", format_diagnostics_human(&new_diagnostics));
        }
        return ExitCode::from(1);
    }

    let findings = review_sources(old_path, &old_source, new_path, &new_source);
    if json {
        println!("{}", format_review_json(&findings));
    } else {
        print!("{}", format_review_human(&findings));
    }
    ExitCode::SUCCESS
}

fn run_review_map(json: bool, path: &str) -> ExitCode {
    let (map, diagnostics) = match review_map_for_path(path) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        print_diagnostics(json, &diagnostics);
        return ExitCode::from(1);
    }

    if json {
        println!("{}", format_review_map_json(&map));
    } else {
        print!("{}", format_review_map_human(&map));
    }
    ExitCode::SUCCESS
}

fn review_map_for_path(path: &str) -> Result<(ReviewMap, Vec<Diagnostic>), String> {
    if is_package_directory(path) {
        let review = review_package_dir(Path::new(path))?;
        return Ok((review.review_map, review.diagnostics));
    }

    let sources = match read_review_map_sources(path) {
        Ok(sources) => sources,
        Err(error) => return Err(error),
    };
    let source_refs = sources
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let diagnostics = analyze_sources_with_interfaces(source_refs.as_slice(), &[]);
    let map = review_map_sources(source_refs);
    Ok((map, diagnostics))
}
struct ReviewMapSource {
    path: String,
    contents: String,
}

fn read_review_map_sources(path: &str) -> Result<Vec<ReviewMapSource>, String> {
    let path = Path::new(path);
    if path.is_file() {
        return read_review_map_file(path).map(|source| vec![source]);
    }
    if !path.is_dir() {
        return Err(format!(
            "review map path is not a file or directory: {}",
            path.display()
        ));
    }

    let mut files = Vec::new();
    collect_rsscript_files(path, &mut files)?;
    files.sort();
    files
        .into_iter()
        .map(|file| read_review_map_file(&file))
        .collect()
}

fn collect_rsscript_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", directory.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_rsscript_files(&path, files)?;
        } else if is_rsscript_source_path(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_rsscript_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
}

fn read_review_map_file(path: &Path) -> Result<ReviewMapSource, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(ReviewMapSource {
        path: path.display().to_string(),
        contents,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn review_map_for_path_uses_package_review_environment() {
        let dep = unique_temp_dir("review-map-package-dep");
        fs::create_dir_all(dep.join("interface")).expect("dependency interface dir should create");
        fs::write(
            dep.join("rsspkg.toml"),
            r#"[package]
name = "rss-review-map-dep"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]
"#,
        )
        .expect("dependency manifest should write");
        fs::write(
            dep.join("interface/lib.rssi"),
            r#"features: native

native fn Dep.echo(message: read String) -> String
    effects(native)
"#,
        )
        .expect("dependency interface should write");

        let root = unique_temp_dir("review-map-package-root");
        fs::create_dir_all(root.join("interface")).expect("root interface dir should create");
        fs::create_dir_all(root.join("src")).expect("root source dir should create");
        fs::write(
            root.join("rsspkg.toml"),
            format!(
                r#"[package]
name = "rss-review-map-root"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[dependencies]
rss-review-map-dep = {{ path = "{}" }}
"#,
                toml_path(&dep)
            ),
        )
        .expect("root manifest should write");
        fs::write(
            root.join("interface/lib.rssi"),
            "pub fn Api.run(message: read String) -> String\n",
        )
        .expect("root interface should write");
        fs::write(
            root.join("src/main.rss"),
            r#"features: native

pub fn Api.run(message: read String) -> String {
    return Dep.echo(message: read message)
}
"#,
        )
        .expect("root source should write");

        let (map, diagnostics) = super::review_map_for_path(root.to_str().expect("utf-8 path"))
            .expect("package review map should load");
        fs::remove_dir_all(root).expect("root temp package should clean up");
        fs::remove_dir_all(dep).expect("dependency temp package should clean up");

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(map.summary.unknown.functions, 0);
        assert!(map.files.iter().any(|file| {
            file.regions.iter().any(|region| {
                region.function == "Api.run"
                    && region
                        .reasons
                        .iter()
                        .any(|reason| reason == "native call `Dep.echo`")
            })
        }));
    }

    #[test]
    fn parse_review_args_rejects_unknown_flags() {
        let values = args(&["--map", "--wat", "package"]);
        let error = super::parse_review_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--wat`.");
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp directory should create");
        path
    }

    fn toml_path(path: &std::path::Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }
}
