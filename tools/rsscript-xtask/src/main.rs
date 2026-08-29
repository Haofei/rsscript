#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use rsscript_provider_api::CancellationToken;
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
    ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError, ProviderErrorMapping,
    ProviderFunction, ProviderFunctionDescriptor, RUNTIME_ABI_VERSION, ResourceCleanupContract,
    WireInterpreterFn,
};
use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime},
};
use serde::{Deserialize, Serialize};

const METRICS_SCHEMA: &str = "rsscript.core_metrics.v1";
const SLO_SCHEMA: &str = "rsscript.core_slo.v1";
const WORKLOAD: &str = r#"
fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 1000 {
        total = (total + index) % 1000000
        index = index + 1
    }
    return total
}
"#;
const CANCELLATION_WORKLOAD: &str = r#"
fn main() -> Int {
    let mut value = 0
    while true {
        value = value + 1
    }
    return value
}
"#;
const PROVIDER_WORKLOAD: &str = r#"
module metrics
use host.metrics.*

fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 1000 {
        total = echo(value: index)
        index = index + 1
    }
    return total
}
"#;
const PROVIDER_INTERFACE: &str = "module host.metrics\npub fn echo(value: read Int) -> Int\n";

#[derive(Debug, Serialize)]
struct MetricDistribution {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Serialize)]
struct CoreMetrics {
    schema: &'static str,
    iterations: usize,
    environment: Environment,
    check: MetricDistribution,
    compile: MetricDistribution,
    artifact_verify: MetricDistribution,
    vm_execute: MetricDistribution,
    provider_execute: MetricDistribution,
    pre_cancel_rejection: MetricDistribution,
    artifact_bytes: usize,
    execution_steps: u64,
    execution_allocated_bytes: usize,
    provider_calls: u64,
    provider_request_bytes: usize,
    provider_response_bytes: usize,
    provider_total_duration_ns: u64,
    provider_max_duration_ns: u64,
}

#[derive(Debug, Serialize)]
struct Environment {
    os: &'static str,
    arch: &'static str,
    profile: &'static str,
    git_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreSlo {
    schema: String,
    max_p95_ms: LatencySlo,
    max_artifact_bytes: usize,
    max_execution_steps: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LatencySlo {
    check: f64,
    compile: f64,
    artifact_verify: f64,
    vm_execute: f64,
    provider_execute: f64,
    pre_cancel_rejection: f64,
}

#[derive(Debug)]
struct Arguments {
    iterations: usize,
    output: Option<PathBuf>,
    check: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("core-metrics") => run_core_metrics(parse_arguments(arguments)?),
        Some("validate-ci") => validate_ci(),
        Some(command @ ("check-tier" | "clippy-tier" | "test-tier" | "doc-tier")) => {
            run_tier_command(command, arguments)
        }
        _ => Err(
            "usage:\n  cargo run -p rsscript-xtask --release -- core-metrics [--iterations N] [--output FILE] [--check SLO]\n  cargo run -p rsscript-xtask -- validate-ci\n  cargo run -p rsscript-xtask -- {check,clippy,test,doc}-tier TIER [--workspace root|experiments] [--feature-set supported|research]"
                .into(),
        ),
    }
}

const PACKAGE_TIERS: &[&str] = &[
    "core",
    "applications",
    "runner",
    "providers",
    "integrations",
    "experimental",
    "optional_engines",
    "research",
    "tooling",
    "examples",
];

fn tier_document(root: &Path) -> Result<toml::Value, Box<dyn Error>> {
    Ok(toml::from_str(&fs::read_to_string(
        root.join("docs/architecture/workspace-tiers.toml"),
    )?)?)
}

fn tier_packages(document: &toml::Value, tier: &str) -> Result<Vec<String>, Box<dyn Error>> {
    if !PACKAGE_TIERS.contains(&tier) {
        return Err(format!("unknown workspace tier `{tier}`").into());
    }
    document[tier]
        .as_array()
        .ok_or_else(|| format!("workspace tier `{tier}` must be an array"))?
        .iter()
        .map(|package| {
            package
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("workspace tier `{tier}` contains a non-string").into())
        })
        .collect()
}

fn run_tier_command(
    operation: &str,
    mut arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let tier = arguments.next().ok_or("test-tier requires a tier name")?;
    let remaining = arguments.collect::<Vec<_>>();
    let workspace = remaining
        .windows(2)
        .find_map(|pair| (pair[0] == "--workspace").then_some(pair[1].as_str()))
        .unwrap_or("root");
    let feature_set = remaining
        .windows(2)
        .find_map(|pair| (pair[0] == "--feature-set").then_some(pair[1].as_str()))
        .unwrap_or("supported");
    if !matches!(workspace, "root" | "experiments") {
        return Err(format!("unknown Cargo workspace `{workspace}`").into());
    }
    if !matches!(feature_set, "supported" | "research") {
        return Err(format!("unknown feature set `{feature_set}`").into());
    }
    let root = workspace_root();
    let document: toml::Value = toml::from_str(&fs::read_to_string(if workspace == "root" {
        root.join("docs/architecture/workspace-tiers.toml")
    } else {
        root.join("docs/architecture/experiments-tiers.toml")
    })?)?;
    let packages = tier_packages(&document, &tier)?;
    if packages.is_empty() {
        println!("workspace tier `{tier}` is empty");
        return Ok(());
    }
    let mut command = Command::new("cargo");
    command.current_dir(&root).arg(match operation {
        "check-tier" => "check",
        "clippy-tier" => "clippy",
        "test-tier" => "test",
        "doc-tier" => "doc",
        _ => unreachable!("validated command"),
    });
    command.arg("--locked");
    if workspace == "experiments" {
        command.args(["--manifest-path", "experiments/Cargo.toml"]);
    }
    if feature_set == "research" {
        command.arg("--all-features");
    }
    if operation == "clippy-tier" {
        command.arg("--all-targets");
    } else if operation == "doc-tier" {
        command.arg("--no-deps");
    }
    for package in &packages {
        command.args(["-p", package]);
    }
    if operation == "clippy-tier" {
        command.args(["--", "-D", "warnings"]);
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Cargo {operation} failed for {workspace} workspace tier `{tier}`").into())
    }
}

#[derive(Debug)]
struct CargoPackageInventory {
    manifest_dir: PathBuf,
    features: BTreeSet<String>,
    tests: BTreeSet<String>,
    test_sources: BTreeSet<PathBuf>,
    test_functions: BTreeSet<String>,
}

/// Check workflow Cargo package, feature, and integration-test references
/// against the workspace that each command explicitly selects.
fn validate_ci() -> Result<(), Box<dyn Error>> {
    let root = workspace_root();
    let root_inventory = cargo_inventory(&root, None)?;
    let experiments_inventory = cargo_inventory(&root, Some("experiments/Cargo.toml"))?;
    validate_workspace_tiers(&root, &root_inventory)?;
    validate_experiments_tiers(&root, &experiments_inventory)?;
    validate_security_workflow_coverage(&root, &root_inventory)?;
    validate_lint_inheritance(&root, &root_inventory, &experiments_inventory)?;
    validate_experimental_retention(&root, &root_inventory, &experiments_inventory)?;
    validate_security_debt(&root)?;
    validate_test_closures(&root)?;
    validate_module_sizes(&root)?;
    let workflow_dir = root.join(".github/workflows");
    validate_sdk_test_reachability(&root, &root_inventory)?;
    let mut workflows = fs::read_dir(&workflow_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
        .collect::<Vec<_>>();
    workflows.sort();

    let mut checked = 0usize;
    for path in workflows {
        let source = fs::read_to_string(&path)?.replace("\\\n", " ");
        for (line_index, line) in source.lines().enumerate() {
            if !line.contains("cargo ") && !line.contains("cargo +") {
                continue;
            }
            let inventory = if line.contains("--manifest-path experiments/Cargo.toml") {
                &experiments_inventory
            } else {
                &root_inventory
            };
            validate_workflow_cargo_command(line, inventory)
                .map_err(|error| format!("{}:{}: {error}", path.display(), line_index + 1))?;
            checked += 1;
        }
    }
    println!("validated {checked} workflow Cargo command line(s)");
    Ok(())
}

fn parse_civil_day(value: &str) -> Result<i64, Box<dyn Error>> {
    let parts = value
        .split('-')
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() != 3 || !(1..=12).contains(&parts[1]) || !(1..=31).contains(&parts[2]) {
        return Err(format!("invalid calendar date `{value}`").into());
    }
    let mut year = parts[0];
    let month = parts[1];
    let day = parts[2];
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Ok(era * 146_097 + day_of_era - 719_468)
}

fn current_unix_day() -> Result<i64, Box<dyn Error>> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok(i64::try_from(seconds / 86_400)?)
}

fn validate_experimental_retention(
    root: &Path,
    root_inventory: &BTreeMap<String, CargoPackageInventory>,
    experiments_inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), Box<dyn Error>> {
    let path = root.join("docs/architecture/experimental-retention.toml");
    let document: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
    if document["schema"].as_integer() != Some(2) {
        return Err("experimental retention inventory must use schema 2".into());
    }
    let surfaces = document["surface"]
        .as_array()
        .ok_or("experimental retention inventory must contain [[surface]] entries")?;
    let mut ids = BTreeSet::new();
    for surface in surfaces {
        let table = surface
            .as_table()
            .ok_or("experimental retention surface must be a table")?;
        let required_strings = [
            "id",
            "workspace",
            "kind",
            "package",
            "owner",
            "status",
            "maturity",
            "last_measured_at",
            "decision_by",
            "removal_rule",
        ];
        for field in required_strings {
            if table
                .get(field)
                .and_then(toml::Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(format!("experimental retention entry is missing `{field}`").into());
            }
        }
        let id = table["id"].as_str().expect("validated id");
        if !ids.insert(id) {
            return Err(format!("experimental retention id `{id}` is duplicated").into());
        }
        let decision = parse_civil_day(table["decision_by"].as_str().expect("validated date"))?;
        let measured =
            parse_civil_day(table["last_measured_at"].as_str().expect("validated date"))?;
        if measured > decision {
            return Err(format!(
                "experimental retention `{id}` was measured after its decision date"
            )
            .into());
        }
        let evidence_uri = table
            .get("evidence_uri")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        let evidence_sha = table
            .get("evidence_sha256")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        if decision < current_unix_day()? && (evidence_uri.is_empty() || evidence_sha.is_empty()) {
            return Err(format!(
                "experimental retention `{id}` expired without immutable evidence; remove it or renew through an ADR"
            )
            .into());
        }
        if !table["owner"]
            .as_str()
            .expect("validated owner")
            .starts_with('@')
        {
            return Err(format!("experimental retention `{id}` needs a concrete @owner").into());
        }
        let workspace = table["workspace"].as_str().expect("validated workspace");
        let inventory = match workspace {
            "root" => root_inventory,
            "experiments" => experiments_inventory,
            _ => {
                return Err(format!(
                    "experimental retention `{id}` has unknown workspace `{workspace}`"
                )
                .into());
            }
        };
        let package = table["package"].as_str().expect("validated package");
        let package_metadata = inventory.get(package).ok_or_else(|| {
            format!("experimental retention `{id}` names missing package `{package}`")
        })?;
        match table["kind"].as_str().expect("validated kind") {
            "cargo_feature" => {
                let feature = table
                    .get("feature")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| format!("experimental retention `{id}` requires feature"))?;
                if !package_metadata.features.contains(feature) {
                    return Err(format!(
                        "experimental retention `{id}` names missing feature `{package}/{feature}`"
                    )
                    .into());
                }
            }
            "cargo_package" | "internal" => {}
            kind => {
                return Err(
                    format!("experimental retention `{id}` has unknown kind `{kind}`").into(),
                );
            }
        }
        let workloads = table
            .get("workloads")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| format!("experimental retention `{id}` needs workloads"))?;
        if workloads.is_empty() {
            return Err(format!("experimental retention `{id}` has no workload").into());
        }
        if table
            .get("minimum_end_to_end_gain_percent")
            .and_then(toml::Value::as_integer)
            .is_none_or(|gain| !(0..=100).contains(&gain))
        {
            return Err(format!(
                "experimental retention `{id}` needs a 0-100 minimum gain threshold"
            )
            .into());
        }
    }
    if surfaces.is_empty() {
        return Err("experimental retention inventory must not be empty".into());
    }
    let cases = fs::read_to_string(root.join("benchmarks/vm-jit/cases.tsv"))?;
    let case_ids = cases
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_whitespace().next())
        .collect::<BTreeSet<_>>();
    let all_tests = root_inventory
        .values()
        .chain(experiments_inventory.values())
        .flat_map(|package| package.tests.iter().chain(package.test_functions.iter()))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for surface in surfaces {
        let table = surface.as_table().expect("validated table");
        let id = table["id"].as_str().expect("validated id");
        for workload in table["workloads"].as_array().expect("validated workloads") {
            let workload = workload.as_str().ok_or("workload must be a string")?;
            if !case_ids.contains(workload) && !all_tests.contains(workload) {
                return Err(format!(
                    "experimental retention `{id}` names missing workload/test `{workload}`"
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_security_debt(root: &Path) -> Result<(), Box<dyn Error>> {
    let document: toml::Value = toml::from_str(&fs::read_to_string(
        root.join("docs/architecture/security-debt.toml"),
    )?)?;
    if document["schema"].as_integer() != Some(1) {
        return Err("security debt inventory must use schema 1".into());
    }
    for exception in document["exception"]
        .as_array()
        .ok_or("security debt inventory needs [[exception]] entries")?
    {
        let id = exception["id"].as_str().ok_or("security debt needs id")?;
        for field in [
            "owner",
            "advisory",
            "scope",
            "tracking",
            "decision_by",
            "removal_rule",
        ] {
            if exception[field]
                .as_str()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(format!("security debt `{id}` is missing `{field}`").into());
            }
        }
        if parse_civil_day(exception["decision_by"].as_str().expect("validated"))?
            < current_unix_day()?
        {
            return Err(format!("security debt `{id}` has expired").into());
        }
    }
    Ok(())
}

fn validate_test_closures(root: &Path) -> Result<(), Box<dyn Error>> {
    let document: toml::Value = toml::from_str(&fs::read_to_string(
        root.join("docs/architecture/test-closures.toml"),
    )?)?;
    if document["schema"].as_integer() != Some(1) {
        return Err("test closure inventory must use schema 1".into());
    }
    for closure in document["closure"]
        .as_array()
        .ok_or("test closure inventory needs [[closure]] entries")?
    {
        let id = closure["id"].as_str().ok_or("test closure needs id")?;
        let paths = closure["paths"]
            .as_array()
            .ok_or_else(|| format!("test closure `{id}` needs paths"))?;
        let workflows = closure["workflows"]
            .as_array()
            .ok_or_else(|| format!("test closure `{id}` needs workflows"))?;
        for workflow in workflows {
            let workflow = workflow.as_str().ok_or("workflow must be a string")?;
            let source = fs::read_to_string(root.join(".github/workflows").join(workflow))?;
            for path in paths {
                let path = path.as_str().ok_or("closure path must be a string")?;
                if !source.contains(&format!("\"{path}\"")) {
                    return Err(format!(
                        "test closure `{id}` workflow `{workflow}` is missing path `{path}`"
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn cargo_inventory(
    root: &Path,
    manifest_path: Option<&str>,
) -> Result<BTreeMap<String, CargoPackageInventory>, Box<dyn Error>> {
    let mut command = Command::new("cargo");
    command
        .args(["metadata", "--locked", "--no-deps", "--format-version=1"])
        .current_dir(root);
    if let Some(manifest_path) = manifest_path {
        command.args(["--manifest-path", manifest_path]);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed for {}: {}",
            manifest_path.unwrap_or("Cargo.toml"),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mut inventory = BTreeMap::new();
    for package in metadata["packages"]
        .as_array()
        .ok_or("cargo metadata packages must be an array")?
    {
        let name = package["name"]
            .as_str()
            .ok_or("cargo metadata package name must be a string")?;
        let features = package["features"]
            .as_object()
            .ok_or("cargo metadata package features must be an object")?
            .keys()
            .cloned()
            .collect();
        let test_targets = package["targets"]
            .as_array()
            .ok_or("cargo metadata package targets must be an array")?
            .iter()
            .filter(|target| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("test")))
            })
            .collect::<Vec<_>>();
        let tests = test_targets
            .iter()
            .filter_map(|target| target["name"].as_str().map(str::to_owned))
            .collect();
        let test_sources = test_targets
            .iter()
            .filter_map(|target| target["src_path"].as_str().map(PathBuf::from))
            .collect();
        let manifest_path = package["manifest_path"]
            .as_str()
            .ok_or("cargo metadata package manifest_path must be a string")?;
        let package_root = Path::new(manifest_path)
            .parent()
            .ok_or("Cargo package manifest must have a parent directory")?;
        let test_functions = collect_test_functions(package_root)?;
        inventory.insert(
            name.to_owned(),
            CargoPackageInventory {
                manifest_dir: package_root.to_path_buf(),
                features,
                tests,
                test_sources,
                test_functions,
            },
        );
    }
    Ok(inventory)
}

fn validate_workspace_tiers(
    root: &Path,
    inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), Box<dyn Error>> {
    let document = tier_document(root)?;
    let mut classified = BTreeSet::new();
    let ci = document["ci"]
        .as_table()
        .ok_or("workspace tier inventory must contain a [ci] table")?;
    for tier in PACKAGE_TIERS {
        let packages = tier_packages(&document, tier)?;
        if !packages.is_empty() {
            let workflows = ci
                .get(*tier)
                .and_then(toml::Value::as_array)
                .ok_or_else(|| format!("non-empty tier `{tier}` must map to CI workflows"))?;
            if workflows.is_empty() {
                return Err(format!("non-empty tier `{tier}` has no CI workflow").into());
            }
            for workflow in workflows {
                let workflow = workflow
                    .as_str()
                    .ok_or_else(|| format!("CI mapping for `{tier}` contains a non-string"))?;
                if !root.join(".github/workflows").join(workflow).is_file() {
                    return Err(format!("tier `{tier}` names missing workflow `{workflow}`").into());
                }
            }
        }
        for package in packages {
            if !inventory.contains_key(&package) {
                return Err(format!("tier `{tier}` contains unknown package `{package}`").into());
            }
            if !classified.insert(package.clone()) {
                return Err(format!("package `{package}` occurs in multiple tiers").into());
            }
        }
    }
    let actual = inventory.keys().cloned().collect::<BTreeSet<_>>();
    if classified != actual {
        let missing = actual.difference(&classified).cloned().collect::<Vec<_>>();
        let stale = classified.difference(&actual).cloned().collect::<Vec<_>>();
        return Err(format!(
            "workspace tier inventory mismatch; missing={missing:?}, stale={stale:?}"
        )
        .into());
    }
    Ok(())
}

fn validate_experiments_tiers(
    root: &Path,
    inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), Box<dyn Error>> {
    let path = root.join("docs/architecture/experiments-tiers.toml");
    let document: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
    if document["schema"].as_integer() != Some(1) {
        return Err("experiments tier inventory must use schema 1".into());
    }
    let ci = document["ci"]
        .as_table()
        .ok_or("experiments tier inventory needs [ci]")?;
    let mut classified = BTreeSet::new();
    for tier in ["experimental", "integrations"] {
        let packages = tier_packages(&document, tier)?;
        let workflows = ci
            .get(tier)
            .and_then(toml::Value::as_array)
            .ok_or_else(|| format!("experiments tier `{tier}` needs CI mapping"))?;
        if workflows.is_empty() && !packages.is_empty() {
            return Err(format!("experiments tier `{tier}` has no workflow").into());
        }
        for package in packages {
            if !inventory.contains_key(&package) {
                return Err(format!(
                    "experiments tier `{tier}` contains missing package `{package}`"
                )
                .into());
            }
            if !classified.insert(package.clone()) {
                return Err(
                    format!("experiments package `{package}` occurs more than once").into(),
                );
            }
        }
    }
    let actual = inventory.keys().cloned().collect::<BTreeSet<_>>();
    if classified != actual {
        return Err(format!(
            "experiments tier inventory mismatch; missing={:?}, stale={:?}",
            actual.difference(&classified).collect::<Vec<_>>(),
            classified.difference(&actual).collect::<Vec<_>>()
        )
        .into());
    }
    let workflow = fs::read_to_string(root.join(".github/workflows/security-sensitive.yml"))?;
    for boundary in document["security_boundaries"]
        .as_array()
        .ok_or("experiments tier inventory needs security_boundaries")?
    {
        let package = boundary
            .as_str()
            .ok_or("security boundary must be a string")?;
        let metadata = inventory
            .get(package)
            .ok_or_else(|| format!("missing experimental security boundary `{package}`"))?;
        let directory = metadata
            .manifest_dir
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if !workflow.contains(&format!("\"{directory}/**\"")) {
            return Err(format!(
                "experimental security boundary `{package}` lacks `{directory}/**` workflow coverage"
            )
            .into());
        }
    }
    Ok(())
}

fn validate_lint_inheritance(
    root: &Path,
    root_inventory: &BTreeMap<String, CargoPackageInventory>,
    experiments_inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), Box<dyn Error>> {
    let unsafe_boundaries = BTreeSet::from(["rss-process-guard", "rsscript-jit-cranelift"]);
    for (package, metadata) in root_inventory.iter().chain(experiments_inventory) {
        let manifest = fs::read_to_string(metadata.manifest_dir.join("Cargo.toml"))?;
        let document: toml::Value = toml::from_str(&manifest)?;
        if document["lints"]["workspace"].as_bool() != Some(true) {
            return Err(format!("package `{package}` does not inherit workspace lints").into());
        }
        let source_root = metadata.manifest_dir.join("src");
        if unsafe_boundaries.contains(package.as_str()) {
            let lib = fs::read_to_string(source_root.join("lib.rs"))?;
            for required in [
                "#![deny(unsafe_op_in_unsafe_fn)]",
                "#![deny(clippy::undocumented_unsafe_blocks)]",
                "#![deny(clippy::missing_safety_doc)]",
            ] {
                if !lib.contains(required) {
                    return Err(
                        format!("unsafe boundary `{package}` is missing `{required}`").into(),
                    );
                }
            }
        } else if source_root.is_dir() {
            for path in rust_files_below(&source_root)? {
                let source = fs::read_to_string(&path)?;
                if contains_unsafe_syntax(&source) {
                    return Err(format!(
                        "non-allowlisted package `{package}` contains unsafe source in {}",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    )
                    .into());
                }
            }
        }
    }
    let owners = fs::read_to_string(root.join(".github/CODEOWNERS"))?;
    for directory in ["/crates/process-guard/", "/crates/rsscript-jit-cranelift/"] {
        if !owners.lines().any(|line| line.starts_with(directory)) {
            return Err(format!("unsafe boundary `{directory}` is missing CODEOWNERS").into());
        }
    }
    Ok(())
}

fn contains_unsafe_syntax(source: &str) -> bool {
    source.lines().any(|line| {
        ["unsafe {", "unsafe fn"].into_iter().any(|needle| {
            let Some(index) = line.find(needle) else {
                return false;
            };
            let prefix = &line[..index];
            if prefix.find("//").is_some() {
                return false;
            }
            prefix.chars().filter(|character| *character == '"').count() % 2 == 0
        })
    })
}

fn validate_security_workflow_coverage(
    root: &Path,
    inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), Box<dyn Error>> {
    let document = tier_document(root)?;
    let boundaries = document["security_boundaries"]
        .as_array()
        .ok_or("workspace tier inventory must declare security_boundaries")?;
    let workflow = fs::read_to_string(root.join(".github/workflows/security-sensitive.yml"))?;
    let path_prefixes = workflow
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- \"")?.strip_suffix("/**\""))
        .collect::<Vec<_>>();
    for package in boundaries {
        let package = package
            .as_str()
            .ok_or("security_boundaries contains a non-string")?;
        let metadata = inventory
            .get(package)
            .ok_or_else(|| format!("security boundary package `{package}` does not exist"))?;
        let directory = metadata
            .manifest_dir
            .strip_prefix(root)
            .map_err(|_| format!("package `{package}` is outside the root workspace"))?
            .to_string_lossy()
            .replace('\\', "/");
        if !path_prefixes
            .iter()
            .any(|prefix| directory == *prefix || directory.starts_with(&format!("{prefix}/")))
        {
            return Err(format!(
                "security boundary `{package}` is missing a path filter covering `{directory}/**`"
            )
            .into());
        }
    }
    Ok(())
}

fn validate_module_sizes(root: &Path) -> Result<(), Box<dyn Error>> {
    let allowlist_path = root.join("docs/architecture/module-size-allowlist.toml");
    let document: toml::Value = toml::from_str(&fs::read_to_string(&allowlist_path)?)?;
    let warn_bytes = document["warn_bytes"].as_integer().unwrap_or(50_000) as u64;
    let hard_bytes = document["hard_bytes"].as_integer().unwrap_or(80_000) as u64;
    let mut allowed = BTreeMap::new();
    for entry in document["allow"]
        .as_array()
        .ok_or("module size allowlist must contain [[allow]] entries")?
    {
        let path = entry["path"]
            .as_str()
            .ok_or("module size allow entry requires path")?;
        let max_bytes = entry["max_bytes"]
            .as_integer()
            .ok_or("module size allow entry requires max_bytes")? as u64;
        let reason = entry["reason"]
            .as_str()
            .ok_or("module size allow entry requires reason")?;
        if reason.trim().is_empty() {
            return Err(format!("module size allow entry `{path}` has no reason").into());
        }
        let owner = entry["owner"]
            .as_str()
            .ok_or("module size allow entry requires owner")?;
        let target_bytes = entry["target_bytes"]
            .as_integer()
            .ok_or("module size allow entry requires target_bytes")?
            as u64;
        let decision_by = entry["decision_by"]
            .as_str()
            .ok_or("module size allow entry requires decision_by")?;
        let tracking = entry["tracking"]
            .as_str()
            .ok_or("module size allow entry requires tracking")?;
        if !owner.starts_with('@') || tracking.trim().is_empty() {
            return Err(format!(
                "module size allow entry `{path}` needs an @owner and tracking reference"
            )
            .into());
        }
        if target_bytes >= max_bytes || target_bytes > hard_bytes {
            return Err(format!(
                "module size allow entry `{path}` needs a target below its ceiling and hard limit"
            )
            .into());
        }
        if parse_civil_day(decision_by)? < current_unix_day()? {
            return Err(format!("module size debt `{path}` expired on `{decision_by}`").into());
        }
        let source_path = root.join(path);
        if !source_path.is_file() {
            return Err(format!(
                "module size allow entry `{path}` is stale because the file is gone"
            )
            .into());
        }
        let current = fs::metadata(&source_path)?.len();
        if current <= hard_bytes {
            return Err(format!(
                "module size allow entry `{path}` is stale; the file is below the hard limit"
            )
            .into());
        }
        if max_bytes > current.saturating_add(4096) {
            return Err(format!(
                "module size debt ceiling for `{path}` must ratchet down after shrinkage (current={current}, max={max_bytes})"
            )
            .into());
        }
        allowed.insert(path.to_owned(), max_bytes);
    }

    let mut pending = vec![
        root.join("crates"),
        root.join("providers"),
        root.join("tools"),
        root.join("examples"),
        root.join("experiments"),
        root.join("fuzz"),
    ];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    pending.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let size = fs::metadata(&path)?.len();
            let relative = path
                .strip_prefix(root)
                .expect("scanned source is below workspace root")
                .to_string_lossy()
                .replace('\\', "/");
            if size > hard_bytes {
                let max = allowed.get(&relative).ok_or_else(|| {
                    format!(
                        "Rust source `{relative}` is {size} bytes (> {hard_bytes}); split it or add a reviewed debt entry"
                    )
                })?;
                if size > *max {
                    return Err(format!(
                        "Rust source `{relative}` grew to {size} bytes above its debt ceiling {max}"
                    )
                    .into());
                }
            } else if size > warn_bytes {
                eprintln!("warning: large Rust source `{relative}` is {size} bytes");
            }
        }
    }
    Ok(())
}

fn rust_files_below(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn collect_test_functions(package_root: &Path) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let mut pending = vec![package_root.to_path_buf()];
    let mut functions = BTreeSet::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                if !path.ends_with("target") && !path.ends_with(".git") {
                    pending.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = fs::read_to_string(path)?;
            let mut test_attribute = false;
            for line in source.lines() {
                let line = line.trim();
                if line == "#[test]" || line.ends_with("::test]") {
                    test_attribute = true;
                    continue;
                }
                if !test_attribute || line.is_empty() || line.starts_with("#[") {
                    continue;
                }
                let declaration = line.strip_prefix("pub ").unwrap_or(line);
                let declaration = declaration.strip_prefix("async ").unwrap_or(declaration);
                if let Some(declaration) = declaration.strip_prefix("fn ")
                    && let Some(name) = declaration
                        .split(|character: char| {
                            !(character.is_ascii_alphanumeric() || character == '_')
                        })
                        .next()
                        .filter(|name| !name.is_empty())
                {
                    functions.insert(name.to_owned());
                }
                test_attribute = false;
            }
        }
    }
    Ok(functions)
}

fn validate_sdk_test_reachability(
    root: &Path,
    inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), Box<dyn Error>> {
    let package = inventory
        .get("rsscript-sdk")
        .ok_or("root workspace must contain rsscript-sdk")?;
    let test_dir = root.join("crates/rsscript-sdk/tests");
    let mut unreachable = Vec::new();
    for entry in fs::read_dir(&test_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "rs")
            && !package.test_sources.contains(&path)
        {
            unreachable.push(path);
        }
    }
    if !unreachable.is_empty() {
        unreachable.sort();
        let paths = unreachable
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "rsscript-sdk disables automatic tests; every top-level test source must be an explicit target: {paths}"
        )
        .into());
    }
    Ok(())
}

fn validate_workflow_cargo_command(
    line: &str,
    inventory: &BTreeMap<String, CargoPackageInventory>,
) -> Result<(), String> {
    let tokens = line
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| matches!(character, '\'' | '"' | ';' | ')' | '('))
        })
        .collect::<Vec<_>>();
    let mut packages = Vec::new();
    let mut features = Vec::new();
    let mut test_target = None;
    let mut index = 0usize;
    while index < tokens.len() {
        match tokens[index] {
            "-p" | "--package" if index + 1 < tokens.len() => {
                packages.push(tokens[index + 1].trim_end_matches('\\'));
                index += 2;
                continue;
            }
            "--features" if index + 1 < tokens.len() => {
                features.extend(tokens[index + 1].trim_end_matches('\\').split(','));
                index += 2;
                continue;
            }
            "--test" if index + 1 < tokens.len() => {
                test_target = Some(tokens[index + 1].trim_end_matches('\\'));
                index += 2;
                continue;
            }
            token if token.starts_with("--features=") => {
                features.extend(token.trim_start_matches("--features=").split(','));
            }
            _ => {}
        }
        index += 1;
    }
    for package in &packages {
        if !inventory.contains_key(*package) {
            return Err(format!("unknown Cargo package `{package}` in `{line}`"));
        }
    }
    if !features.is_empty() {
        for package in &packages {
            let package_inventory = &inventory[*package];
            for feature in &features {
                if !package_inventory.features.contains(*feature) {
                    return Err(format!(
                        "package `{package}` has no feature `{feature}` in `{line}`"
                    ));
                }
            }
        }
    }
    if let Some(test_target) = test_target {
        if packages.len() != 1 {
            return Err(format!(
                "workflow test target `{test_target}` must name exactly one package in `{line}`"
            ));
        }
        let package = packages[0];
        if !inventory[package].tests.contains(test_target) {
            return Err(format!(
                "package `{package}` has no test target `{test_target}` in `{line}`"
            ));
        }
    }
    if let Some(filter) = cargo_test_filter(&tokens) {
        if packages.len() != 1 {
            return Err(format!(
                "workflow test filter `{filter}` must name exactly one package in `{line}`"
            ));
        }
        let package = packages[0];
        let leaf = filter.rsplit("::").next().unwrap_or(filter);
        if !inventory[package]
            .test_functions
            .iter()
            .any(|name| name.contains(leaf))
        {
            return Err(format!(
                "package `{package}` has no test function matching filter `{filter}` in `{line}`"
            ));
        }
    }
    Ok(())
}

fn cargo_test_filter<'a>(tokens: &'a [&'a str]) -> Option<&'a str> {
    let cargo = tokens.iter().position(|token| *token == "cargo")?;
    let test = tokens[cargo + 1..]
        .iter()
        .position(|token| *token == "test")?
        + cargo
        + 1;
    let options_with_values = [
        "-p",
        "--package",
        "--features",
        "--test",
        "--manifest-path",
        "--target",
        "--profile",
        "-j",
        "--jobs",
        "--exclude",
        "--bin",
        "--example",
        "--bench",
        "--color",
        "--config",
        "--target-dir",
    ];
    let mut index = test + 1;
    while index < tokens.len() {
        let token = tokens[index].trim_end_matches('\\');
        if token == "--" {
            return None;
        }
        if options_with_values.contains(&token) {
            index += 2;
            continue;
        }
        if token.starts_with('-') || token.contains('=') {
            index += 1;
            continue;
        }
        return Some(token);
    }
    None
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask must remain under the workspace tools directory")
        .to_path_buf()
}

fn parse_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Arguments, Box<dyn Error>> {
    let mut parsed = Arguments {
        iterations: 20,
        output: None,
        check: None,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--iterations" => {
                parsed.iterations = arguments
                    .next()
                    .ok_or("--iterations requires a value")?
                    .parse()?;
                if parsed.iterations < 2 {
                    return Err("--iterations must be at least 2".into());
                }
            }
            "--output" => {
                parsed.output = Some(PathBuf::from(
                    arguments.next().ok_or("--output requires a path")?,
                ));
            }
            "--check" => {
                parsed.check = Some(PathBuf::from(
                    arguments.next().ok_or("--check requires a path")?,
                ));
            }
            _ => return Err(format!("unknown core-metrics argument: {argument}").into()),
        }
    }
    Ok(parsed)
}

fn run_core_metrics(arguments: Arguments) -> Result<(), Box<dyn Error>> {
    let compiler = Compiler;
    let provider_package = compiler.compile_with_interfaces(
        &[("provider-metrics.rss", PROVIDER_WORKLOAD)],
        &[("provider-metrics.rssi", PROVIDER_INTERFACE)],
    )?;
    let provider_package = ArtifactVerifier
        .verify(provider_package)?
        .admit_trusted_input();
    let provider_runtime = metrics_provider_runtime()?;

    // Warm each path before collecting distributions so the report measures
    // steady Core behavior rather than one-time process and allocator setup.
    for _ in 0..3 {
        let _ = compiler.check("core-metrics.rss", WORKLOAD);
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        let bytes = package.bundle_bytes()?;
        let loaded = ArtifactVerifier.verify_bytes(&bytes)?.admit_trusted_input();
        Runtime::default()
            .link(&loaded)?
            .execute(ExecutionRequest::default());
        provider_runtime
            .link(&provider_package)?
            .execute(ExecutionRequest::default());
    }

    let mut check = Vec::with_capacity(arguments.iterations);
    let mut compile = Vec::with_capacity(arguments.iterations);
    let mut verify = Vec::with_capacity(arguments.iterations);
    let mut execute = Vec::with_capacity(arguments.iterations);
    let mut provider_execute = Vec::with_capacity(arguments.iterations);
    let mut cancel = Vec::with_capacity(arguments.iterations);
    let mut artifact_bytes = 0;
    let mut execution_steps = 0;
    let mut execution_allocated_bytes = 0;
    let mut provider_calls = 0;
    let mut provider_request_bytes = 0;
    let mut provider_response_bytes = 0;
    let mut provider_total_duration_ns = 0;
    let mut provider_max_duration_ns = 0;

    let cancellation_package = ArtifactVerifier
        .verify(compiler.compile("cancel.rss", CANCELLATION_WORKLOAD)?)?
        .admit_trusted_input();
    for _ in 0..arguments.iterations {
        check.push(measure(|| {
            let diagnostics = compiler.check("core-metrics.rss", WORKLOAD);
            assert!(
                diagnostics.is_empty(),
                "Core metric workload must remain valid"
            );
        }));

        let started = Instant::now();
        let package = compiler.compile("core-metrics.rss", WORKLOAD)?;
        compile.push(elapsed_ms(started));

        let bytes = package.bundle_bytes()?;
        artifact_bytes = bytes.len();
        let started = Instant::now();
        let loaded = ArtifactVerifier.verify_bytes(&bytes)?.admit_trusted_input();
        verify.push(elapsed_ms(started));

        let runtime = Runtime::default();
        let linked = runtime.link(&loaded)?;
        let started = Instant::now();
        let report = linked.execute(ExecutionRequest::default());
        if let Some(error) = report.failure() {
            return Err(error.to_string().into());
        }
        execute.push(elapsed_ms(started));
        execution_steps = report.usage.steps_consumed;
        execution_allocated_bytes = report.usage.allocation_bytes_consumed;

        let linked = provider_runtime.link(&provider_package)?;
        let started = Instant::now();
        let report = linked.execute(ExecutionRequest::default());
        if let Some(error) = report.failure() {
            return Err(error.to_string().into());
        }
        provider_execute.push(elapsed_ms(started));
        let summary = report
            .telemetry
            .provider_functions
            .first()
            .ok_or("Provider metric workload did not record its external call")?;
        provider_calls = summary.calls;
        provider_request_bytes = summary.request_bytes;
        provider_response_bytes = summary.response_bytes;
        provider_total_duration_ns = summary.total_duration_ns;
        provider_max_duration_ns = summary.max_duration_ns;

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = Runtime::new(ProviderRegistry::default());
        let linked = runtime.link(&cancellation_package)?;
        let started = Instant::now();
        let report = linked.execute(
            ExecutionRequest::default()
                .limits(RunLimits::unbounded_for_trusted_host().with_cancellation(cancellation)),
        );
        cancel.push(elapsed_ms(started));
        if report.termination_reason().as_str() != "cancelled" {
            return Err(format!(
                "pre-cancel workload terminated as {}",
                report.termination_reason().as_str()
            )
            .into());
        }
    }

    let metrics = CoreMetrics {
        schema: METRICS_SCHEMA,
        iterations: arguments.iterations,
        environment: Environment {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            git_revision: std::env::var("GITHUB_SHA").ok(),
        },
        check: distribution(check),
        compile: distribution(compile),
        artifact_verify: distribution(verify),
        vm_execute: distribution(execute),
        provider_execute: distribution(provider_execute),
        pre_cancel_rejection: distribution(cancel),
        artifact_bytes,
        execution_steps,
        execution_allocated_bytes,
        provider_calls,
        provider_request_bytes,
        provider_response_bytes,
        provider_total_duration_ns,
        provider_max_duration_ns,
    };
    let json = serde_json::to_string_pretty(&metrics)?;
    println!("{json}");
    if let Some(output) = arguments.output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, format!("{json}\n"))?;
    }
    if let Some(slo_path) = arguments.check {
        check_slo(&metrics, &slo_path)?;
    }
    Ok(())
}

fn metrics_provider_runtime() -> Result<Runtime, Box<dyn Error>> {
    let symbol = ExternalSymbol::new("host.metrics.echo")
        .map_err(|_| "invalid built-in metrics provider symbol")?;
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "value".into(),
            effect: DataEffect::Read,
            ty: "Int".into(),
            retained: false,
        }],
        result: "Int".into(),
        asynchronous: false,
    };
    let descriptor = ProviderDescriptor {
        provider_id: "rsscript.metrics".into(),
        provider_version: "1.0.0".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "echo".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    };
    let mut providers = ProviderRegistry::default();
    providers.register(
        &descriptor,
        BTreeMap::from([(
            symbol,
            ProviderFunction {
                signature,
                callable: WireInterpreterFn::new(|mut values| {
                    values.pop().filter(|_| values.is_empty()).ok_or_else(|| {
                        ProviderError::invalid_argument("metrics echo expects one argument")
                    })
                }),
            },
        )]),
    )?;
    Ok(Runtime::new(providers))
}

fn measure(action: impl FnOnce()) -> f64 {
    let started = Instant::now();
    action();
    elapsed_ms(started)
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn distribution(mut samples: Vec<f64>) -> MetricDistribution {
    samples.sort_by(f64::total_cmp);
    MetricDistribution {
        p50_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        max_ms: *samples.last().expect("at least two metric samples"),
    }
}

fn percentile(samples: &[f64], quantile: f64) -> f64 {
    let index = ((samples.len() - 1) as f64 * quantile).ceil() as usize;
    samples[index]
}

fn check_slo(metrics: &CoreMetrics, path: &PathBuf) -> Result<(), Box<dyn Error>> {
    let slo: CoreSlo = serde_json::from_slice(&fs::read(path)?)?;
    if slo.schema != SLO_SCHEMA {
        return Err(format!("unsupported Core SLO schema: {}", slo.schema).into());
    }
    let latency = [
        ("check", metrics.check.p95_ms, slo.max_p95_ms.check),
        ("compile", metrics.compile.p95_ms, slo.max_p95_ms.compile),
        (
            "artifact_verify",
            metrics.artifact_verify.p95_ms,
            slo.max_p95_ms.artifact_verify,
        ),
        (
            "vm_execute",
            metrics.vm_execute.p95_ms,
            slo.max_p95_ms.vm_execute,
        ),
        (
            "provider_execute",
            metrics.provider_execute.p95_ms,
            slo.max_p95_ms.provider_execute,
        ),
        (
            "pre_cancel_rejection",
            metrics.pre_cancel_rejection.p95_ms,
            slo.max_p95_ms.pre_cancel_rejection,
        ),
    ];
    let mut failures = latency
        .into_iter()
        .filter(|(_, observed, maximum)| observed > maximum)
        .map(|(name, observed, maximum)| {
            format!("{name} p95 {observed:.3} ms exceeds {maximum:.3} ms")
        })
        .collect::<Vec<_>>();
    if metrics.artifact_bytes > slo.max_artifact_bytes {
        failures.push(format!(
            "artifact size {} exceeds {} bytes",
            metrics.artifact_bytes, slo.max_artifact_bytes
        ));
    }
    if metrics.execution_steps > slo.max_execution_steps {
        failures.push(format!(
            "execution steps {} exceed {}",
            metrics.execution_steps, slo.max_execution_steps
        ));
    }
    if failures.is_empty() {
        println!("Core SLO check passed: {}", path.display());
        Ok(())
    } else {
        Err(format!("Core SLO check failed:\n- {}", failures.join("\n- ")).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_inventory(functions: &[&str]) -> BTreeMap<String, CargoPackageInventory> {
        BTreeMap::from([(
            "example".into(),
            CargoPackageInventory {
                manifest_dir: PathBuf::from("crates/example"),
                features: BTreeSet::from(["execution".into()]),
                tests: BTreeSet::from(["integration".into()]),
                test_sources: BTreeSet::new(),
                test_functions: functions.iter().map(|name| (*name).to_owned()).collect(),
            },
        )])
    }

    #[test]
    fn percentile_uses_a_conservative_nearest_rank() {
        let samples = (1..=20).map(f64::from).collect::<Vec<_>>();
        assert_eq!(percentile(&samples, 0.50), 11.0);
        assert_eq!(percentile(&samples, 0.95), 20.0);
    }

    #[test]
    fn argument_parser_rejects_single_sample_runs() {
        let error = parse_arguments(["--iterations".into(), "1".into()].into_iter())
            .expect_err("one iteration cannot produce a useful distribution");
        assert!(error.to_string().contains("at least 2"));
    }

    #[test]
    fn workflow_validator_accepts_a_filter_that_names_a_real_test() {
        let inventory = test_inventory(&["actual_contract_test"]);
        validate_workflow_cargo_command(
            "cargo test -p example --features execution --lib module::actual_contract_test -- --exact",
            &inventory,
        )
        .unwrap();
    }

    #[test]
    fn workflow_validator_rejects_a_filter_that_would_run_zero_tests() {
        let inventory = test_inventory(&["actual_contract_test"]);
        let error = validate_workflow_cargo_command(
            "cargo test -p example --lib retired_contract_ -- --nocapture",
            &inventory,
        )
        .unwrap_err();
        assert!(error.contains("no test function matching filter `retired_contract_`"));
    }

    #[test]
    fn metrics_shape_matches_the_published_schema() {
        let distribution = || MetricDistribution {
            p50_ms: 1.0,
            p95_ms: 2.0,
            max_ms: 3.0,
        };
        let metrics = CoreMetrics {
            schema: METRICS_SCHEMA,
            iterations: 20,
            environment: Environment {
                os: "test",
                arch: "test",
                profile: "release",
                git_revision: None,
            },
            check: distribution(),
            compile: distribution(),
            artifact_verify: distribution(),
            vm_execute: distribution(),
            provider_execute: distribution(),
            pre_cancel_rejection: distribution(),
            artifact_bytes: 1,
            execution_steps: 1,
            execution_allocated_bytes: 0,
            provider_calls: 1000,
            provider_request_bytes: 8000,
            provider_response_bytes: 8000,
            provider_total_duration_ns: 1000,
            provider_max_duration_ns: 1,
        };
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/rsscript.core_metrics.v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let instance = serde_json::to_value(metrics).unwrap();
        assert!(validator.is_valid(&instance));
    }

    #[test]
    fn controlled_jit_baseline_schema_accepts_only_auditable_evidence() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../benchmarks/vm-jit/baseline/schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let valid = serde_json::json!({
            "schema": "rsscript.native_jit_baseline.v1",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "cpu": "controlled test CPU",
            "os": "linux",
            "arch": "x86_64",
            "rust_version": "rustc test",
            "cranelift_version": "cranelift-codegen test",
            "profile": "release",
            "warmup": 3,
            "samples": 25,
            "fixture_digest": "sha256:test",
            "controlled": true,
            "cpu_affinity": "2",
            "cpu_governor": "performance",
            "sample_order": "alternating",
            "cases": [{
                "case": "mixed-mode-continuation",
                "pass": "continuation",
                "status": "entered",
                "interpreter_ns": 100,
                "cold_e2e_native_ns": 50,
                "speedup": 2.0,
                "compile_nanos": 10,
                "resident_code_bytes": 128,
                "native_calls": 1,
                "native_bails": 0,
                "osr_entries": 0,
                "continuation_entries": 1,
                "runtime_helper_call_sites": 0,
                "readonly_licm_sites": 0,
                "bounds_check_sites": 0,
                "bounds_checks_elided": 0,
                "semantic_match": true,
                "controlled": true,
                "retention_threshold_met": true
            }]
        });
        assert!(validator.is_valid(&valid));
        let mut invalid = valid;
        invalid["samples"] = 5.into();
        assert!(!validator.is_valid(&invalid));
    }
}
