use reir::{
    Bundle, Capability, Diff, DiffItem, DiffItemKind, Evidence, Fact, Reconciliation,
    ReconciliationKind, ReconciliationStatus, Slice, SliceKind, compute_diff,
    reconcile_capabilities, slice_by_kind,
};
use std::env;
use std::fs;
use std::process::ExitCode;

const USAGE: &str = "Usage:
  reir reconcile --required required.json --granted granted.json [--json]
  reir diff --baseline baseline.json --current current.json [--json]
  reir slice --bundle bundle.json [--kind missing_capability] [--json]
  reir merge file1.json file2.json [...] --out merged.json
  reir show bundle.json [--json]";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(command) = args.get(1).map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };

    match command {
        "reconcile" => run_reconcile(&args[2..]),
        "diff" => run_diff(&args[2..]),
        "slice" => run_slice(&args[2..]),
        "merge" => run_merge(&args[2..]),
        "show" => run_show(&args[2..]),
        "--help" | "-h" | "help" => {
            print_usage();
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("unknown command: {command}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Runtime(String),
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(message.into())
    }
}

fn run_reconcile(args: &[String]) -> ExitCode {
    match try_run_reconcile(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_reconcile(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut required = None;
    let mut granted = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--required" => required = Some(take_value(args, &mut index, "--required")?),
            "--granted" => granted = Some(take_value(args, &mut index, "--granted")?),
            "--json" => json = true,
            other => {
                return Err(CliError::usage(format!(
                    "unknown reconcile argument: {other}"
                )));
            }
        }
        index += 1;
    }

    let required_path = required.ok_or_else(|| CliError::usage("missing --required <file>"))?;
    let granted_path = granted.ok_or_else(|| CliError::usage("missing --granted <file>"))?;

    let required_bundle = read_bundle(&required_path)?;
    let granted_bundle = read_bundle(&granted_path)?;
    let reconciliations = reconcile_capabilities(&required_bundle.facts, &granted_bundle.facts);

    if json {
        print_json(&reconciliations)?;
    } else {
        print_reconciliation_text(&reconciliations, &required_bundle, &granted_bundle);
    }

    Ok(
        if reconciliations
            .iter()
            .any(|reconciliation| reconciliation.status == ReconciliationStatus::Fail)
        {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        },
    )
}

fn run_diff(args: &[String]) -> ExitCode {
    match try_run_diff(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_diff(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut baseline = None;
    let mut current = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--baseline" => baseline = Some(take_value(args, &mut index, "--baseline")?),
            "--current" => current = Some(take_value(args, &mut index, "--current")?),
            "--json" => json = true,
            other => return Err(CliError::usage(format!("unknown diff argument: {other}"))),
        }
        index += 1;
    }

    let baseline_path = baseline.ok_or_else(|| CliError::usage("missing --baseline <file>"))?;
    let current_path = current.ok_or_else(|| CliError::usage("missing --current <file>"))?;

    let baseline_bundle = read_bundle(&baseline_path)?;
    let current_bundle = read_bundle(&current_path)?;
    let diff = compute_diff(&baseline_bundle, &current_bundle);

    if json {
        print_json(&diff)?;
    } else {
        print_diff_text(&diff);
    }

    Ok(ExitCode::SUCCESS)
}

fn run_slice(args: &[String]) -> ExitCode {
    match try_run_slice(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_slice(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut bundle = None;
    let mut filter_kind = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bundle" => bundle = Some(take_value(args, &mut index, "--bundle")?),
            "--kind" => {
                filter_kind = Some(parse_slice_kind(&take_value(args, &mut index, "--kind")?)?)
            }
            "--json" => json = true,
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown slice argument: {value}")));
            }
            value => {
                if bundle.replace(value.to_owned()).is_some() {
                    return Err(CliError::usage("slice accepts a single bundle file"));
                }
            }
        }
        index += 1;
    }

    let bundle_path = bundle.ok_or_else(|| CliError::usage("missing --bundle <file>"))?;
    let bundle = read_bundle(&bundle_path)?;
    let mut slices = slice_by_kind(&bundle);
    if let Some(kind) = filter_kind {
        slices.retain(|slice| slice.kind == kind);
    }

    if json {
        print_json(&slices)?;
    } else {
        print_slice_text(&slices);
    }

    Ok(ExitCode::SUCCESS)
}

fn run_merge(args: &[String]) -> ExitCode {
    match try_run_merge(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_merge(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut inputs = Vec::new();
    let mut out = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--out" => out = Some(take_value(args, &mut index, "--out")?),
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown merge argument: {value}")));
            }
            value => inputs.push(value.to_owned()),
        }
        index += 1;
    }

    if inputs.is_empty() {
        return Err(CliError::usage("merge requires at least one input bundle"));
    }

    let out_path = out.ok_or_else(|| CliError::usage("missing --out <file>"))?;
    let merged = merge_bundles(&inputs)?;
    write_json_file(&out_path, &merged)?;

    println!("merged {} bundle(s) into {out_path}", inputs.len());
    print_bundle_summary(&merged);
    Ok(ExitCode::SUCCESS)
}

fn run_show(args: &[String]) -> ExitCode {
    match try_run_show(args) {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

fn try_run_show(args: &[String]) -> Result<ExitCode, CliError> {
    if wants_help(args) {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    }

    let mut bundle = None;
    let mut json = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            value if value.starts_with('-') => {
                return Err(CliError::usage(format!("unknown show argument: {value}")));
            }
            value => {
                if bundle.replace(value.to_owned()).is_some() {
                    return Err(CliError::usage("show accepts a single bundle file"));
                }
            }
        }
        index += 1;
    }

    let bundle_path = bundle.ok_or_else(|| CliError::usage("missing bundle file"))?;
    let bundle = read_bundle(&bundle_path)?;

    if json {
        print_json(&bundle)?;
    } else {
        print_bundle_text(&bundle);
    }

    Ok(ExitCode::SUCCESS)
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, CliError> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| CliError::usage(format!("missing value for {flag}")))
}

fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
}

fn read_bundle(path: &str) -> Result<Bundle, CliError> {
    let json = fs::read_to_string(path)
        .map_err(|error| CliError::runtime(format!("failed to read {path}: {error}")))?;
    Bundle::from_json(&json)
        .map_err(|error| CliError::runtime(format!("failed to parse {path}: {error}")))
}

fn write_json_file(path: &str, bundle: &Bundle) -> Result<(), CliError> {
    let json = serde_json::to_string_pretty(bundle)
        .map_err(|error| CliError::runtime(format!("failed to serialize bundle: {error}")))?;
    fs::write(path, format!("{json}\n"))
        .map_err(|error| CliError::runtime(format!("failed to write {path}: {error}")))
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), CliError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::runtime(format!("failed to serialize JSON: {error}")))?;
    println!("{json}");
    Ok(())
}

fn merge_bundles(paths: &[String]) -> Result<Bundle, CliError> {
    let mut bundles = paths
        .iter()
        .map(|path| read_bundle(path))
        .collect::<Result<Vec<_>, _>>()?;

    let mut merged = bundles.drain(..).next().unwrap_or_default();
    for bundle in bundles {
        merged.producers.extend(bundle.producers);
        merged.subjects.extend(bundle.subjects);
        merged.subject_chains.extend(bundle.subject_chains);
        merged.facts.extend(bundle.facts);
        merged.edges.extend(bundle.edges);
        merged.reconciliations.extend(bundle.reconciliations);
        merged.slices.extend(bundle.slices);
        merged.policy_results.extend(bundle.policy_results);
        merged.diffs.extend(bundle.diffs);
        merged.exceptions.extend(bundle.exceptions);
    }

    Ok(merged)
}

fn print_reconciliation_text(
    reconciliations: &[Reconciliation],
    required: &Bundle,
    granted: &Bundle,
) {
    println!("REIR RECONCILIATION\n");
    println!("status: {}", overall_reconciliation_status(reconciliations));

    if reconciliations.is_empty() {
        println!("\n(no reconciliation items)");
        return;
    }

    for reconciliation in reconciliations {
        println!();
        println!("{}:", reconciliation_kind_label(&reconciliation.kind));
        if let Some(capability) = &reconciliation.capability {
            println!("  {}", display_capability(capability));
        }
        if let Some(required_fact_id) = &reconciliation.required_fact {
            if let Some(fact) = find_fact(&required.facts, required_fact_id) {
                println!("  required by: {}", display_fact_subject(fact));
            } else {
                println!("  required fact: {required_fact_id}");
            }
        }
        if !reconciliation.granted_facts.is_empty() {
            for granted_fact_id in &reconciliation.granted_facts {
                if let Some(fact) = find_fact(&granted.facts, granted_fact_id) {
                    println!("  granted to: {}", display_fact_subject(fact));
                } else {
                    println!("  granted fact: {granted_fact_id}");
                }
            }
        }
        if let Some(observed_fact_id) = &reconciliation.observed_fact {
            println!("  observed fact: {observed_fact_id}");
        }
        if let Some(subject_chain) = &reconciliation.subject_chain {
            println!("  subject chain: {subject_chain}");
        }
        if let Some(evidence) = display_evidence(&reconciliation.evidence) {
            println!("  evidence: {evidence}");
        }
        if let Some(reason) = reconciliation
            .risk
            .as_ref()
            .and_then(|risk| risk.reason.as_deref())
        {
            println!("  reason: {reason}");
        }
    }
}

fn print_diff_text(diff: &Diff) {
    println!("REIR DIFF\n");
    println!("items: {}", diff.items.len());

    if diff.items.is_empty() {
        println!("\n(no diff items)");
        return;
    }

    for item in &diff.items {
        print_diff_item(item);
    }
}

fn print_diff_item(item: &DiffItem) {
    println!();
    println!("{}: {}", diff_item_kind_label(&item.kind), item.id);
    if let Some(subject) = &item.subject {
        println!("  subject: {}", subject.id);
    }
    if let Some(description) = &item.description {
        println!("  description: {description}");
    }
    if let Some(evidence) = display_evidence(&item.evidence) {
        println!("  evidence: {evidence}");
    }
}

fn print_slice_text(slices: &[Slice]) {
    println!("REIR SLICES\n");
    println!("slices: {}", slices.len());

    if slices.is_empty() {
        println!("\n(no slices)");
        return;
    }

    for slice in slices {
        println!();
        println!("{}:", slice_kind_label(&slice.kind));
        println!("  id: {}", slice.id);
        println!("  facts: {}", slice.facts.len());
        for fact in &slice.facts {
            println!("    - {fact}");
        }
        println!("  reconciliations: {}", slice.reconciliations.len());
        for reconciliation in &slice.reconciliations {
            println!("    - {reconciliation}");
        }
        if let Some(evidence) = display_evidence(&slice.evidence) {
            println!("  evidence: {evidence}");
        }
    }
}

fn print_bundle_text(bundle: &Bundle) {
    println!("REIR BUNDLE\n");
    println!("schema: {}", bundle.schema);
    println!("ontology: {}", bundle.ontology);
    print_bundle_summary(bundle);
}

fn print_bundle_summary(bundle: &Bundle) {
    println!("producers: {}", bundle.producers.len());
    println!("subjects: {}", bundle.subjects.len());
    println!("subject_chains: {}", bundle.subject_chains.len());
    println!("facts: {}", bundle.facts.len());
    println!("edges: {}", bundle.edges.len());
    println!("reconciliations: {}", bundle.reconciliations.len());
    println!("slices: {}", bundle.slices.len());
    println!("policy_results: {}", bundle.policy_results.len());
    println!("diffs: {}", bundle.diffs.len());
    println!("exceptions: {}", bundle.exceptions.len());
}

fn display_capability(capability: &Capability) -> String {
    let capability_name = capability
        .action
        .clone()
        .unwrap_or_else(|| String::from(capability.category.clone()));

    match &capability.resource {
        Some(resource) => format!("{capability_name} on {resource}"),
        None => capability_name,
    }
}

fn display_fact_subject(fact: &Fact) -> String {
    fact.subject
        .name
        .clone()
        .unwrap_or_else(|| fact.subject.id.clone())
}

fn display_evidence(evidence: &[Evidence]) -> Option<String> {
    evidence
        .iter()
        .find_map(|entry| {
            entry
                .file
                .as_ref()
                .map(|file| match (entry.line, entry.column) {
                    (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
                    (Some(line), None) => format!("{file}:{line}"),
                    _ => file.clone(),
                })
        })
        .or_else(|| {
            evidence
                .iter()
                .find_map(|entry| entry.symbol.clone().or_else(|| entry.reason.clone()))
        })
}

fn find_fact<'a>(facts: &'a [Fact], id: &str) -> Option<&'a Fact> {
    facts.iter().find(|fact| fact.id == id)
}

fn overall_reconciliation_status(reconciliations: &[Reconciliation]) -> &'static str {
    if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Fail)
    {
        "fail"
    } else if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Warn)
    {
        "warn"
    } else if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Unknown)
    {
        "unknown"
    } else {
        "pass"
    }
}

fn reconciliation_kind_label(kind: &ReconciliationKind) -> String {
    match kind {
        ReconciliationKind::Covered => "covered capability".to_owned(),
        ReconciliationKind::MissingCapability => "missing capability".to_owned(),
        ReconciliationKind::ExcessCapability => "excess capability".to_owned(),
        ReconciliationKind::UnexpectedObservation => "unexpected observation".to_owned(),
        ReconciliationKind::UnauthorizedObservation => "unauthorized observation".to_owned(),
        ReconciliationKind::UnusedCapability => "unused capability".to_owned(),
        ReconciliationKind::PartialCoverage => "partial coverage".to_owned(),
        ReconciliationKind::UnknownCoverage => "unknown coverage".to_owned(),
        ReconciliationKind::ChainIncomplete => "chain incomplete".to_owned(),
        ReconciliationKind::Extension(value) => value.replace('_', " "),
    }
}

fn diff_item_kind_label(kind: &DiffItemKind) -> String {
    match kind {
        DiffItemKind::FactAdded => "fact added".to_owned(),
        DiffItemKind::FactRemoved => "fact removed".to_owned(),
        DiffItemKind::FactChanged => "fact changed".to_owned(),
        DiffItemKind::EdgeAdded => "edge added".to_owned(),
        DiffItemKind::EdgeRemoved => "edge removed".to_owned(),
        DiffItemKind::EdgeChanged => "edge changed".to_owned(),
        DiffItemKind::SubjectChainAdded => "subject chain added".to_owned(),
        DiffItemKind::SubjectChainChanged => "subject chain changed".to_owned(),
        DiffItemKind::ReconciliationAdded => "reconciliation added".to_owned(),
        DiffItemKind::ReconciliationRemoved => "reconciliation removed".to_owned(),
        DiffItemKind::ReconciliationChanged => "reconciliation changed".to_owned(),
        DiffItemKind::ProfileRuleChanged => "profile rule changed".to_owned(),
        DiffItemKind::ExceptionAdded => "exception added".to_owned(),
        DiffItemKind::ExceptionExpired => "exception expired".to_owned(),
        DiffItemKind::ProducerChanged => "producer changed".to_owned(),
        DiffItemKind::OntologyChanged => "ontology changed".to_owned(),
        DiffItemKind::Extension(value) => value.replace('_', " "),
    }
}

fn slice_kind_label(kind: &SliceKind) -> String {
    match kind {
        SliceKind::MissingCapabilitySlice => "missing_capability".to_owned(),
        SliceKind::ExcessCapabilitySlice => "excess_capability".to_owned(),
        SliceKind::UnexpectedObservationSlice => "unexpected_observation".to_owned(),
        SliceKind::NetworkSlice => "network".to_owned(),
        SliceKind::PublicIngressSlice => "public_ingress".to_owned(),
        SliceKind::ObjectStorageSlice => "object_storage".to_owned(),
        SliceKind::DatabaseSlice => "database".to_owned(),
        SliceKind::SecretSlice => "secret".to_owned(),
        SliceKind::FilesystemSlice => "filesystem".to_owned(),
        SliceKind::IdentitySlice => "identity".to_owned(),
        SliceKind::RbacSlice => "rbac".to_owned(),
        SliceKind::StorageSlice => "storage".to_owned(),
        SliceKind::BuildTimeSlice => "build_time".to_owned(),
        SliceKind::NativeUnsafeSlice => "native_unsafe".to_owned(),
        SliceKind::PackageRiskSlice => "package_risk".to_owned(),
        SliceKind::RuntimeDriftSlice => "runtime_drift".to_owned(),
        SliceKind::SubjectChainSlice => "subject_chain".to_owned(),
        SliceKind::DiffSlice => "diff".to_owned(),
        SliceKind::UnknownSlice => "unknown".to_owned(),
        SliceKind::Extension(value) => value.clone(),
    }
}

fn parse_slice_kind(value: &str) -> Result<SliceKind, CliError> {
    match value {
        "missing_capability" | "missing_capability_slice" => Ok(SliceKind::MissingCapabilitySlice),
        "excess_capability" | "excess_capability_slice" => Ok(SliceKind::ExcessCapabilitySlice),
        "unexpected_observation" | "unexpected_observation_slice" => {
            Ok(SliceKind::UnexpectedObservationSlice)
        }
        "network" | "network_slice" => Ok(SliceKind::NetworkSlice),
        "public_ingress" | "public_ingress_slice" => Ok(SliceKind::PublicIngressSlice),
        "object_storage" | "object_storage_slice" => Ok(SliceKind::ObjectStorageSlice),
        "database" | "database_slice" => Ok(SliceKind::DatabaseSlice),
        "secret" | "secret_slice" => Ok(SliceKind::SecretSlice),
        "filesystem" | "filesystem_slice" => Ok(SliceKind::FilesystemSlice),
        "identity" | "identity_slice" => Ok(SliceKind::IdentitySlice),
        "rbac" | "rbac_slice" => Ok(SliceKind::RbacSlice),
        "storage" | "storage_slice" => Ok(SliceKind::StorageSlice),
        "build_time" | "build_time_slice" => Ok(SliceKind::BuildTimeSlice),
        "native_unsafe" | "native_unsafe_slice" => Ok(SliceKind::NativeUnsafeSlice),
        "package_risk" | "package_risk_slice" => Ok(SliceKind::PackageRiskSlice),
        "runtime_drift" | "runtime_drift_slice" => Ok(SliceKind::RuntimeDriftSlice),
        "subject_chain" | "subject_chain_slice" => Ok(SliceKind::SubjectChainSlice),
        "diff" | "diff_slice" => Ok(SliceKind::DiffSlice),
        "unknown" | "unknown_slice" => Ok(SliceKind::UnknownSlice),
        _ => Err(CliError::usage(format!("unknown slice kind: {value}"))),
    }
}

fn report_error(error: CliError) -> ExitCode {
    match error {
        CliError::Usage(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        CliError::Runtime(message) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

fn print_usage() {
    println!("{USAGE}");
}
