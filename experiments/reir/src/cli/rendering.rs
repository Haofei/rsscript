use super::{CliError, USAGE};
use reir::api::v1::{
    model::{Bundle, Capability, Evidence, Fact},
    reconciliation::{
        Diff, DiffItem, DiffItemKind, Reconciliation, ReconciliationKind, ReconciliationStatus,
        Slice, SliceKind,
    },
};
use std::process::ExitCode;

pub(super) fn print_reconciliation_text(
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
        if let Some(target) = &reconciliation.target {
            println!("  target: {target}");
        }
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

pub(super) fn print_diff_text(diff: &Diff) {
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

pub(super) fn print_diff_item(item: &DiffItem) {
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

pub(super) fn print_slice_text(slices: &[Slice]) {
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

pub(super) fn print_bundle_text(bundle: &Bundle) {
    println!("REIR BUNDLE\n");
    println!("schema: {}", bundle.schema);
    println!("ontology: {}", bundle.ontology);
    print_bundle_summary(bundle);
}

pub(super) fn print_bundle_summary(bundle: &Bundle) {
    println!("producers: {}", bundle.producers.len());
    println!("subjects: {}", bundle.subjects.len());
    println!("subject_chains: {}", bundle.subject_chains.len());
    println!("facts: {}", bundle.facts.len());
    println!("edges: {}", bundle.edges.len());
    println!("reconciliations: {}", bundle.reconciliations.len());
    println!("slices: {}", bundle.slices.len());
    println!("policy_results: {}", bundle.policy_results.len());
    println!("profiles: {}", bundle.profiles.len());
    println!("diffs: {}", bundle.diffs.len());
    println!("exceptions: {}", bundle.exceptions.len());
}

pub(super) fn display_capability(capability: &Capability) -> String {
    let capability_name = capability
        .action
        .clone()
        .unwrap_or_else(|| String::from(capability.category.clone()));

    match &capability.resource {
        Some(resource) => format!("{capability_name} on {resource}"),
        None => capability_name,
    }
}

pub(super) fn display_fact_subject(fact: &Fact) -> String {
    fact.subject
        .name
        .clone()
        .unwrap_or_else(|| fact.subject.id.clone())
}

pub(super) fn display_evidence(evidence: &[Evidence]) -> Option<String> {
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

pub(super) fn find_fact<'a>(facts: &'a [Fact], id: &str) -> Option<&'a Fact> {
    facts.iter().find(|fact| fact.id == id)
}

pub(super) fn overall_reconciliation_status(reconciliations: &[Reconciliation]) -> &'static str {
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

pub(super) fn reconciliation_kind_label(kind: &ReconciliationKind) -> String {
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

pub(super) fn diff_item_kind_label(kind: &DiffItemKind) -> String {
    match kind {
        DiffItemKind::FactAdded => "fact added".to_owned(),
        DiffItemKind::FactRemoved => "fact removed".to_owned(),
        DiffItemKind::FactChanged => "fact changed".to_owned(),
        DiffItemKind::EdgeAdded => "edge added".to_owned(),
        DiffItemKind::EdgeRemoved => "edge removed".to_owned(),
        DiffItemKind::EdgeChanged => "edge changed".to_owned(),
        DiffItemKind::SubjectChainAdded => "subject chain added".to_owned(),
        DiffItemKind::SubjectChainRemoved => "subject chain removed".to_owned(),
        DiffItemKind::SubjectChainChanged => "subject chain changed".to_owned(),
        DiffItemKind::ReconciliationAdded => "reconciliation added".to_owned(),
        DiffItemKind::ReconciliationRemoved => "reconciliation removed".to_owned(),
        DiffItemKind::ReconciliationChanged => "reconciliation changed".to_owned(),
        DiffItemKind::SliceAdded => "slice added".to_owned(),
        DiffItemKind::SliceRemoved => "slice removed".to_owned(),
        DiffItemKind::SliceChanged => "slice changed".to_owned(),
        DiffItemKind::DiffAdded => "diff added".to_owned(),
        DiffItemKind::DiffRemoved => "diff removed".to_owned(),
        DiffItemKind::DiffChanged => "diff changed".to_owned(),
        DiffItemKind::PolicyResultAdded => "policy result added".to_owned(),
        DiffItemKind::PolicyResultRemoved => "policy result removed".to_owned(),
        DiffItemKind::PolicyResultChanged => "policy result changed".to_owned(),
        DiffItemKind::ProfileRuleChanged => "profile rule changed".to_owned(),
        DiffItemKind::ExceptionAdded => "exception added".to_owned(),
        DiffItemKind::ExceptionExpired => "exception expired".to_owned(),
        DiffItemKind::ExceptionChanged => "exception changed".to_owned(),
        DiffItemKind::SchemaChanged => "schema changed".to_owned(),
        DiffItemKind::ProducerChanged => "producer changed".to_owned(),
        DiffItemKind::OntologyChanged => "ontology changed".to_owned(),
        DiffItemKind::Extension(value) => value.replace('_', " "),
    }
}

pub(super) fn slice_kind_label(kind: &SliceKind) -> String {
    match kind {
        SliceKind::MissingCapabilitySlice => "missing_capability".to_owned(),
        SliceKind::ExcessCapabilitySlice => "excess_capability".to_owned(),
        SliceKind::UnexpectedObservationSlice => "unexpected_observation".to_owned(),
        SliceKind::NetworkSlice => "network".to_owned(),
        SliceKind::PublicIngressSlice => "public_ingress".to_owned(),
        SliceKind::ObjectStorageSlice => "object_storage".to_owned(),
        SliceKind::DatabaseSlice => "database".to_owned(),
        SliceKind::SecretSlice => "secret".to_owned(),
        SliceKind::EnvSlice => "env".to_owned(),
        SliceKind::TimeSlice => "time".to_owned(),
        SliceKind::RandomnessSlice => "randomness".to_owned(),
        SliceKind::ComputeSlice => "compute".to_owned(),
        SliceKind::TelemetrySlice => "telemetry".to_owned(),
        SliceKind::ProcessSlice => "process".to_owned(),
        SliceKind::AsyncSlice => "async".to_owned(),
        SliceKind::DiagnosticSlice => "diagnostic".to_owned(),
        SliceKind::PackageFeatureSlice => "package_feature".to_owned(),
        SliceKind::ProviderImplementationSlice => "provider_implementation".to_owned(),
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

pub(super) fn parse_slice_kind(value: &str) -> Result<SliceKind, CliError> {
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
        "env" | "env_slice" => Ok(SliceKind::EnvSlice),
        "time" | "time_slice" => Ok(SliceKind::TimeSlice),
        "randomness" | "randomness_slice" => Ok(SliceKind::RandomnessSlice),
        "compute" | "compute_slice" => Ok(SliceKind::ComputeSlice),
        "telemetry" | "telemetry_slice" => Ok(SliceKind::TelemetrySlice),
        "process" | "process_slice" => Ok(SliceKind::ProcessSlice),
        "async" | "async_slice" => Ok(SliceKind::AsyncSlice),
        "diagnostic" | "diagnostic_slice" => Ok(SliceKind::DiagnosticSlice),
        "package_feature" | "package_feature_slice" => Ok(SliceKind::PackageFeatureSlice),
        "provider_implementation" | "provider_implementation_slice" => {
            Ok(SliceKind::ProviderImplementationSlice)
        }
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

pub(super) fn report_error(error: CliError) -> ExitCode {
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

pub(super) fn print_usage() {
    println!("{USAGE}");
}
