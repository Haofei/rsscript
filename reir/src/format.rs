use crate::*;
use std::collections::HashMap;
use std::fmt::Write;

/// Format reconciliation results as human-readable text.
pub fn format_reconciliations_human(reconciliations: &[Reconciliation]) -> String {
    let missing: Vec<_> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::MissingCapability)
        .collect();
    let excess: Vec<_> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::ExcessCapability)
        .collect();
    let covered = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::Covered)
        .count();

    let mut output = String::new();
    writeln!(
        output,
        "status: {}",
        reconciliation_status_label(reconciliations)
    )
    .unwrap();
    writeln!(output).unwrap();

    for reconciliation in &missing {
        writeln!(output, "missing capability:").unwrap();
        if let Some(target) = reconciliation.target.as_deref() {
            writeln!(output, "  target: {target}").unwrap();
        }
        writeln!(
            output,
            "  {}",
            reconciliation
                .capability
                .as_ref()
                .map(format_capability_label)
                .unwrap_or_else(|| "unknown capability".to_string())
        )
        .unwrap();
        if let Some(required_fact) = reconciliation.required_fact.as_deref() {
            writeln!(output, "  required by: {}", required_fact).unwrap();
        }
        if !reconciliation.evidence.is_empty() {
            write_evidence_block(&mut output, "  ", &reconciliation.evidence);
        }
        writeln!(output).unwrap();
    }

    for reconciliation in &excess {
        writeln!(output, "excess capability:").unwrap();
        if let Some(target) = reconciliation.target.as_deref() {
            writeln!(output, "  target: {target}").unwrap();
        }
        writeln!(
            output,
            "  {}",
            reconciliation
                .capability
                .as_ref()
                .map(format_capability_action_label)
                .unwrap_or_else(|| "unknown capability".to_string())
        )
        .unwrap();
        if !reconciliation.granted_facts.is_empty() {
            writeln!(
                output,
                "  granted to: {}",
                reconciliation.granted_facts.join(", ")
            )
            .unwrap();
        }
        writeln!(output).unwrap();
    }

    writeln!(
        output,
        "summary: {} covered, {} missing, {} excess",
        covered,
        missing.len(),
        excess.len()
    )
    .unwrap();
    output
}

/// Format a deployment review as a PR-bot style comment.
pub fn format_pr_review_comment(
    required_facts: &[Fact],
    granted_facts: &[Fact],
    reconciliations: &[Reconciliation],
) -> String {
    let mut output = String::new();
    let missing_groups = pr_missing_groups(required_facts, reconciliations);
    let covered_required = missing_groups.is_empty().then(|| {
        required_facts
            .iter()
            .filter(|fact| fact.role == Some(FactRole::Required))
            .collect::<Vec<_>>()
    });

    writeln!(output, "## RSScript / REIR deployment review\n").unwrap();
    writeln!(
        output,
        "Status: {}\n",
        if missing_groups.is_empty() {
            "PASS"
        } else {
            "FAIL"
        }
    )
    .unwrap();

    if let Some(required) = covered_required {
        writeln!(output, "### Covered required capabilities\n").unwrap();
        if required.is_empty() {
            writeln!(output, "- none\n").unwrap();
        } else {
            for fact in &required {
                write_pr_required_fact(&mut output, fact);
            }
            writeln!(output).unwrap();
        }
    } else {
        writeln!(
            output,
            "### Required capabilities needing deployment grant\n"
        )
        .unwrap();
        for group in &missing_groups {
            write_pr_missing_group(&mut output, group);
        }
        writeln!(output).unwrap();
    }

    writeln!(output, "### Current prod grants\n").unwrap();
    let cloud_policy_grants = granted_facts
        .iter()
        .filter(|fact| fact.acquisition_mode == AcquisitionMode::CloudPolicy)
        .collect::<Vec<_>>();
    let grant_source = if cloud_policy_grants.is_empty() {
        granted_facts.iter().collect::<Vec<_>>()
    } else {
        cloud_policy_grants
    };
    let mut grants = grant_source
        .into_iter()
        .filter(|fact| fact.role == Some(FactRole::Granted))
        .filter_map(|fact| fact.capability.as_ref())
        .filter(|capability| capability.action.is_some())
        .map(pr_capability_label)
        .collect::<Vec<_>>();
    grants.sort();
    grants.dedup();
    if grants.is_empty() {
        writeln!(output, "- none\n").unwrap();
    } else {
        for grant in grants {
            writeln!(output, "- {grant}").unwrap();
        }
        writeln!(output).unwrap();
    }

    writeln!(output, "### Missing capabilities\n").unwrap();
    if missing_groups.is_empty() {
        writeln!(output, "- none\n").unwrap();
    } else {
        for group in &missing_groups {
            write_pr_missing_group(&mut output, group);
        }
        writeln!(output).unwrap();
    }

    writeln!(output, "### Review decision\n").unwrap();
    if missing_groups.is_empty() {
        writeln!(output, "No missing deployment capability was found.").unwrap();
    } else {
        writeln!(
            output,
            "Block this PR before deploy. Either remove the code paths above, or update IAM and review why the missing access is needed."
        )
        .unwrap();
    }
    output
}

struct PrMissingGroup<'a> {
    capability_label: String,
    missing_label: String,
    required_facts: Vec<&'a Fact>,
}

fn pr_missing_groups<'a>(
    required_facts: &'a [Fact],
    reconciliations: &[Reconciliation],
) -> Vec<PrMissingGroup<'a>> {
    let mut group_indexes = HashMap::new();
    let mut groups = Vec::new();
    for reconciliation in reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::MissingCapability)
    {
        let Some(capability) = &reconciliation.capability else {
            continue;
        };
        let capability_label = pr_capability_label(capability);
        let group_index = *group_indexes
            .entry(capability_label.clone())
            .or_insert_with(|| {
                groups.push(PrMissingGroup {
                    capability_label,
                    missing_label: pr_missing_capability_label(capability),
                    required_facts: Vec::new(),
                });
                groups.len() - 1
            });
        let Some(required_fact) = reconciliation
            .required_fact
            .as_deref()
            .and_then(|id| required_facts.iter().find(|fact| fact.id == id))
        else {
            continue;
        };
        if !groups[group_index]
            .required_facts
            .iter()
            .any(|fact| fact.id == required_fact.id)
        {
            groups[group_index].required_facts.push(required_fact);
        }
    }
    groups
}

fn write_pr_required_fact(output: &mut String, fact: &Fact) {
    writeln!(output, "- subject: {}", pr_subject_name(fact)).unwrap();
    if let Some(capability) = &fact.capability {
        writeln!(output, "  capability: {}", pr_capability_label(capability)).unwrap();
    }
    if let Some(evidence) = fact.evidence.first() {
        writeln!(output, "  evidence: {}", pr_evidence_label(evidence)).unwrap();
    }
}

fn write_pr_missing_group(output: &mut String, group: &PrMissingGroup<'_>) {
    writeln!(output, "- {}", group.missing_label).unwrap();
    writeln!(output, "  capability: {}", group.capability_label).unwrap();
    if !group.required_facts.is_empty() {
        writeln!(output, "  required by:").unwrap();
        for fact in &group.required_facts {
            writeln!(output, "    - subject: {}", pr_subject_name(fact)).unwrap();
            if let Some(evidence) = fact.evidence.first() {
                writeln!(output, "      evidence: {}", pr_evidence_label(evidence)).unwrap();
            }
        }
    }
}

/// Format a diff as human-readable text (spec section 9.4 format).
pub fn format_diff_human(diff: &Diff) -> String {
    let mut output = String::new();
    writeln!(output, "REIR DIFF\n").unwrap();

    let mut items: Vec<_> = diff.items.iter().enumerate().collect();
    items.sort_by_key(|(index, item)| (diff_group_rank(&item.kind), *index));

    for (_, item) in items {
        write_prefixed_block(
            &mut output,
            diff_item_prefix(&item.kind),
            item.description.as_deref().unwrap_or(&item.id),
        );

        let description = item.description.as_deref().unwrap_or("");
        if !description.contains('\n') {
            if let Some(subject) = item.subject.as_ref() {
                writeln!(
                    output,
                    "  {}: {}",
                    diff_subject_label(subject),
                    format_subject_label(subject)
                )
                .unwrap();
            }
            if !item.evidence.is_empty() {
                write_evidence_block(&mut output, "  ", &item.evidence);
            }
        }

        writeln!(output).unwrap();
    }

    writeln!(output, "review result:").unwrap();
    writeln!(output, "  {}", diff_review_result_label(diff)).unwrap();
    output
}

fn pr_subject_name(fact: &Fact) -> String {
    fact.subject
        .name
        .clone()
        .or_else(|| fact.subject.id.rsplit("::").next().map(str::to_owned))
        .unwrap_or_else(|| fact.subject.id.clone())
}

fn pr_capability_label(capability: &Capability) -> String {
    format!(
        "{} {}/{} {} {}",
        capability_category_label(&capability.category),
        capability.provider.as_deref().unwrap_or("unknown"),
        capability.service.as_deref().unwrap_or("unknown"),
        capability.action.as_deref().unwrap_or("unknown"),
        capability.resource.as_deref().unwrap_or("unknown")
    )
}

fn pr_missing_capability_label(capability: &Capability) -> String {
    match (capability.action.as_deref(), capability.resource.as_deref()) {
        (Some(action), Some(resource)) => format!("{action} on {resource}"),
        (Some(action), None) => action.to_string(),
        _ => pr_capability_label(capability),
    }
}

fn pr_evidence_label(evidence: &Evidence) -> String {
    let location = match (evidence.file.as_deref(), evidence.line) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (Some(file), None) => file.to_string(),
        _ => "unknown".to_string(),
    };
    let reason = evidence
        .reason
        .as_deref()
        .and_then(|reason| {
            reason
                .split_once(" propagated through ")
                .map(|(_, chain)| chain)
        })
        .or(evidence.reason.as_deref());
    match reason {
        Some(reason) => format!("{location} {reason}"),
        None => location,
    }
}

/// Format slices as human-readable text (spec section 10.3 format).
pub fn format_slices_human(slices: &[Slice]) -> String {
    let mut output = String::new();

    for (index, slice) in slices.iter().enumerate() {
        writeln!(output, "{}\n", slice_kind_heading(&slice.kind)).unwrap();

        write_string_section(&mut output, "facts", &slice.facts);
        write_string_section(&mut output, "edges", &slice.edges);
        write_string_section(&mut output, "reconciliations", &slice.reconciliations);

        if !slice.evidence.is_empty() {
            writeln!(output, "evidence:").unwrap();
            for evidence in &slice.evidence {
                writeln!(output, "  {}", format_evidence_label(evidence)).unwrap();
            }
        }

        if index + 1 != slices.len() {
            writeln!(output).unwrap();
        }
    }

    output
}

/// Format the full MVP reconciliation report (spec section 19.4).
pub fn format_reconciliation_report(
    reconciliations: &[Reconciliation],
    subject_chains: &[SubjectChain],
) -> String {
    let subject_chains_by_id: HashMap<_, _> = subject_chains
        .iter()
        .map(|subject_chain| (subject_chain.id.as_str(), subject_chain))
        .collect();
    let missing: Vec<_> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::MissingCapability)
        .collect();
    let excess: Vec<_> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::ExcessCapability)
        .collect();
    let covered = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::Covered)
        .count();

    let mut output = String::new();
    writeln!(output, "REIR CAPABILITY RECONCILIATION\n").unwrap();
    writeln!(
        output,
        "status: {}\n",
        reconciliation_status_label(reconciliations)
    )
    .unwrap();

    for (index, reconciliation) in missing.iter().enumerate() {
        let subject_chain = reconciliation
            .subject_chain
            .as_deref()
            .and_then(|subject_chain_id| subject_chains_by_id.get(subject_chain_id).copied());

        writeln!(output, "missing capability:").unwrap();
        if let Some(target) = reconciliation.target.as_deref() {
            writeln!(output, "  target: {target}").unwrap();
        }
        writeln!(
            output,
            "  {}\n",
            reconciliation
                .capability
                .as_ref()
                .map(format_capability_label)
                .unwrap_or_else(|| "unknown capability".to_string())
        )
        .unwrap();

        writeln!(output, "required by:").unwrap();
        writeln!(
            output,
            "  {}",
            required_subject_label(reconciliation, subject_chain)
        )
        .unwrap();
        if !reconciliation.evidence.is_empty() {
            write_evidence_block(&mut output, "  ", &reconciliation.evidence);
        }
        writeln!(output).unwrap();

        if let Some(subject_chain) = subject_chain {
            if subject_chain.nodes.len() > 1 || !subject_chain.evidence.is_empty() {
                writeln!(output, "deployment chain:").unwrap();
                for node in subject_chain.nodes.iter().skip(1) {
                    writeln!(
                        output,
                        "  {}: {}",
                        subject_chain_node_label(node),
                        format_subject_label(node)
                    )
                    .unwrap();
                }
                if !subject_chain.evidence.is_empty() {
                    write_evidence_block(&mut output, "  ", &subject_chain.evidence);
                }
                writeln!(output).unwrap();
            }
        }

        writeln!(output, "reason:").unwrap();
        writeln!(
            output,
            "  {}\n",
            reconciliation
                .risk
                .as_ref()
                .and_then(|risk| risk.reason.as_deref())
                .map(humanize_reason)
                .unwrap_or_else(|| "risk not provided".to_string())
        )
        .unwrap();

        writeln!(output, "impact:").unwrap();
        writeln!(
            output,
            "  deployment likely fails at runtime{}",
            required_subject_for_impact(reconciliation, subject_chain)
                .map(|subject| format!(" when {} runs", subject))
                .unwrap_or_default()
        )
        .unwrap();

        if index + 1 != missing.len() || !excess.is_empty() {
            writeln!(output).unwrap();
        }
    }

    for (index, reconciliation) in excess.iter().enumerate() {
        let subject_chain = reconciliation
            .subject_chain
            .as_deref()
            .and_then(|subject_chain_id| subject_chains_by_id.get(subject_chain_id).copied());

        writeln!(output, "excess capability:").unwrap();
        if let Some(target) = reconciliation.target.as_deref() {
            writeln!(output, "  target: {target}").unwrap();
        }
        writeln!(
            output,
            "  {}",
            reconciliation
                .capability
                .as_ref()
                .map(format_capability_action_label)
                .unwrap_or_else(|| "unknown capability".to_string())
        )
        .unwrap();
        writeln!(
            output,
            "  granted to: {}",
            granted_subject_label(reconciliation, subject_chain)
        )
        .unwrap();
        writeln!(output, "  no code requires this capability").unwrap();

        if index + 1 != excess.len() {
            writeln!(output).unwrap();
        }
    }

    writeln!(output, "\n---").unwrap();
    writeln!(
        output,
        "summary: {} covered, {} missing, {} excess",
        covered,
        missing.len(),
        excess.len()
    )
    .unwrap();
    output
}

/// Format a bundle summary (one-line per category).
pub fn format_bundle_summary(bundle: &Bundle) -> String {
    let mut output = String::new();
    writeln!(output, "REIR BUNDLE SUMMARY").unwrap();
    writeln!(output, "producers: {}", bundle.producers.len()).unwrap();
    writeln!(output, "subjects: {}", bundle.subjects.len()).unwrap();
    writeln!(output, "subject chains: {}", bundle.subject_chains.len()).unwrap();
    writeln!(output, "facts: {}", bundle.facts.len()).unwrap();
    writeln!(output, "edges: {}", bundle.edges.len()).unwrap();
    writeln!(output, "reconciliations: {}", bundle.reconciliations.len()).unwrap();
    writeln!(output, "slices: {}", bundle.slices.len()).unwrap();
    writeln!(output, "policy results: {}", bundle.policy_results.len()).unwrap();
    writeln!(output, "profiles: {}", bundle.profiles.len()).unwrap();
    writeln!(output, "diffs: {}", bundle.diffs.len()).unwrap();
    writeln!(output, "exceptions: {}", bundle.exceptions.len()).unwrap();
    output
}

/// Format policy results as human-readable text.
pub fn format_policy_results_human(results: &[PolicyResult]) -> String {
    let mut output = String::new();
    writeln!(output, "REIR POLICY EVALUATION\n").unwrap();
    for result in results {
        writeln!(
            output,
            "{}: {}",
            policy_status_label(&result.status),
            result.reason.as_deref().unwrap_or(&result.id)
        )
        .unwrap();
    }
    output
}

fn format_capability_label(capability: &Capability) -> String {
    let action = capability_action_name(capability);
    match (action, capability.resource.as_deref()) {
        (Some(action), Some(resource)) => format!("{} on {}", action, resource),
        (Some(action), None) => action,
        (None, Some(resource)) => format!(
            "{} on {}",
            capability_category_label(&capability.category),
            resource
        ),
        (None, None) => capability_category_label(&capability.category),
    }
}

fn format_capability_action_label(capability: &Capability) -> String {
    capability_action_name(capability)
        .unwrap_or_else(|| capability_category_label(&capability.category))
}

fn format_evidence_label(evidence: &Evidence) -> String {
    if let Some(file) = evidence.file.as_deref() {
        if let Some(line) = evidence.line {
            return format!("{}:{}", file, line);
        }
        if let Some(pointer) = evidence.json_pointer.as_deref() {
            return format!("{}#{}", file, pointer);
        }
        if let Some(symbol) = evidence.symbol.as_deref() {
            return format!("{} {}", file, symbol);
        }
        return file.to_string();
    }

    if let Some(source) = evidence.source.as_deref() {
        return source.to_string();
    }
    if let Some(resource) = evidence.resource.as_deref() {
        return resource.to_string();
    }
    if let Some(value) = evidence.value.as_deref() {
        return value.to_string();
    }
    if let Some(event_id) = evidence.event_id.as_deref() {
        return event_id.to_string();
    }
    if let Some(reason) = evidence.reason.as_deref() {
        return reason.to_string();
    }

    String::from(evidence.kind.clone())
}

fn format_subject_label(subject: &Subject) -> String {
    let kind = subject_kind_key(&subject.kind);
    match (subject.package.as_deref(), subject.name.as_deref()) {
        (Some(package), Some(name)) if kind.starts_with("code.") => {
            format!("{}::{}", package, name)
        }
        (_, Some(name)) => name.to_string(),
        _ => subject.id.clone(),
    }
}

fn capability_action_name(capability: &Capability) -> Option<String> {
    capability.action.as_ref().map(|action| {
        match (
            capability.provider.as_deref(),
            capability.service.as_deref(),
        ) {
            (Some(provider), Some(service)) => format!("{}.{}.{}", provider, service, action),
            (Some(provider), None) => format!("{}.{}", provider, action),
            (None, Some(service)) => format!("{}.{}", service, action),
            (None, None) => action.clone(),
        }
    })
}

fn capability_category_label(category: &CapabilityCategory) -> String {
    String::from(category.clone())
}

fn subject_kind_key(kind: &SubjectKind) -> String {
    String::from(kind.clone())
}

fn reconciliation_status_label(reconciliations: &[Reconciliation]) -> &'static str {
    if reconciliations.iter().any(|reconciliation| {
        reconciliation.kind != ReconciliationKind::Covered
            || reconciliation.status != ReconciliationStatus::Pass
    }) {
        "fail"
    } else {
        "pass"
    }
}

fn policy_status_label(status: &PolicyStatus) -> &'static str {
    match status {
        PolicyStatus::Pass => "pass",
        PolicyStatus::Fail => "fail",
        PolicyStatus::Warn => "warn",
        PolicyStatus::Review => "review",
    }
}

fn diff_item_prefix(kind: &DiffItemKind) -> &'static str {
    match kind {
        DiffItemKind::FactAdded
        | DiffItemKind::EdgeAdded
        | DiffItemKind::SubjectChainAdded
        | DiffItemKind::ReconciliationAdded
        | DiffItemKind::SliceAdded
        | DiffItemKind::DiffAdded
        | DiffItemKind::PolicyResultAdded
        | DiffItemKind::ExceptionAdded => "+ ",
        DiffItemKind::FactRemoved
        | DiffItemKind::EdgeRemoved
        | DiffItemKind::SubjectChainRemoved
        | DiffItemKind::ReconciliationRemoved
        | DiffItemKind::SliceRemoved
        | DiffItemKind::DiffRemoved
        | DiffItemKind::PolicyResultRemoved
        | DiffItemKind::ExceptionExpired => "- ",
        DiffItemKind::FactChanged
        | DiffItemKind::EdgeChanged
        | DiffItemKind::SubjectChainChanged
        | DiffItemKind::ReconciliationChanged
        | DiffItemKind::SliceChanged
        | DiffItemKind::DiffChanged
        | DiffItemKind::PolicyResultChanged
        | DiffItemKind::ProfileRuleChanged
        | DiffItemKind::ExceptionChanged
        | DiffItemKind::SchemaChanged
        | DiffItemKind::ProducerChanged
        | DiffItemKind::OntologyChanged
        | DiffItemKind::Extension(_) => "~ ",
    }
}

fn diff_group_rank(kind: &DiffItemKind) -> u8 {
    match kind {
        DiffItemKind::FactAdded | DiffItemKind::FactRemoved | DiffItemKind::FactChanged => 0,
        DiffItemKind::EdgeAdded | DiffItemKind::EdgeRemoved | DiffItemKind::EdgeChanged => 1,
        DiffItemKind::SubjectChainAdded
        | DiffItemKind::SubjectChainRemoved
        | DiffItemKind::SubjectChainChanged => 2,
        DiffItemKind::ReconciliationAdded
        | DiffItemKind::ReconciliationRemoved
        | DiffItemKind::ReconciliationChanged => 3,
        DiffItemKind::SliceAdded | DiffItemKind::SliceRemoved | DiffItemKind::SliceChanged => 4,
        DiffItemKind::DiffAdded | DiffItemKind::DiffRemoved | DiffItemKind::DiffChanged => 5,
        DiffItemKind::PolicyResultAdded
        | DiffItemKind::PolicyResultRemoved
        | DiffItemKind::PolicyResultChanged
        | DiffItemKind::ProfileRuleChanged => 6,
        DiffItemKind::ExceptionAdded
        | DiffItemKind::ExceptionExpired
        | DiffItemKind::ExceptionChanged => 7,
        DiffItemKind::SchemaChanged => 8,
        DiffItemKind::ProducerChanged => 9,
        DiffItemKind::OntologyChanged => 10,
        DiffItemKind::Extension(_) => 11,
    }
}

fn diff_review_result_label(diff: &Diff) -> &'static str {
    if diff.items.is_empty() {
        "pass"
    } else {
        "fail"
    }
}

fn diff_subject_label(subject: &Subject) -> &'static str {
    if subject_kind_key(&subject.kind).starts_with("code.") {
        "code"
    } else {
        "subject"
    }
}

fn slice_kind_heading(kind: &SliceKind) -> String {
    let key = match kind {
        SliceKind::MissingCapabilitySlice => "missing_capability_slice",
        SliceKind::ExcessCapabilitySlice => "excess_capability_slice",
        SliceKind::UnexpectedObservationSlice => "unexpected_observation_slice",
        SliceKind::NetworkSlice => "network_slice",
        SliceKind::PublicIngressSlice => "public_ingress_slice",
        SliceKind::ObjectStorageSlice => "object_storage_slice",
        SliceKind::DatabaseSlice => "database_slice",
        SliceKind::SecretSlice => "secret_slice",
        SliceKind::EnvSlice => "env_slice",
        SliceKind::TimeSlice => "time_slice",
        SliceKind::RandomnessSlice => "randomness_slice",
        SliceKind::ComputeSlice => "compute_slice",
        SliceKind::TelemetrySlice => "telemetry_slice",
        SliceKind::ProcessSlice => "process_slice",
        SliceKind::AsyncSlice => "async_slice",
        SliceKind::DiagnosticSlice => "diagnostic_slice",
        SliceKind::PackageFeatureSlice => "package_feature_slice",
        SliceKind::ProviderImplementationSlice => "provider_implementation_slice",
        SliceKind::FilesystemSlice => "filesystem_slice",
        SliceKind::IdentitySlice => "identity_slice",
        SliceKind::RbacSlice => "rbac_slice",
        SliceKind::StorageSlice => "storage_slice",
        SliceKind::BuildTimeSlice => "build_time_slice",
        SliceKind::NativeUnsafeSlice => "native_unsafe_slice",
        SliceKind::PackageRiskSlice => "package_risk_slice",
        SliceKind::RuntimeDriftSlice => "runtime_drift_slice",
        SliceKind::SubjectChainSlice => "subject_chain_slice",
        SliceKind::DiffSlice => "diff_slice",
        SliceKind::UnknownSlice => "unknown_slice",
        SliceKind::Extension(value) => value,
    };
    key.replace('_', " ").to_uppercase()
}

fn subject_chain_node_label(subject: &Subject) -> String {
    match subject_kind_key(&subject.kind).as_str() {
        "container.image" => "image".to_string(),
        "k8s.pod_template" => "pod template".to_string(),
        "k8s.deployment" => "workload".to_string(),
        "k8s.service_account" => "service account".to_string(),
        "cloud.role" => "role".to_string(),
        "cloud.function" => "function".to_string(),
        "cloud.bucket" => "bucket".to_string(),
        other => other.rsplit('.').next().unwrap_or(other).replace('_', " "),
    }
}

fn required_subject_label(
    reconciliation: &Reconciliation,
    subject_chain: Option<&SubjectChain>,
) -> String {
    subject_chain
        .and_then(|subject_chain| subject_chain.nodes.first())
        .map(format_subject_label)
        .or_else(|| reconciliation.required_fact.clone())
        .unwrap_or_else(|| "unknown subject".to_string())
}

fn granted_subject_label(
    reconciliation: &Reconciliation,
    subject_chain: Option<&SubjectChain>,
) -> String {
    subject_chain
        .and_then(|subject_chain| subject_chain.nodes.last())
        .map(format_subject_label)
        .or_else(|| {
            if reconciliation.granted_facts.is_empty() {
                None
            } else {
                Some(reconciliation.granted_facts.join(", "))
            }
        })
        .unwrap_or_else(|| "unknown subject".to_string())
}

fn required_subject_for_impact(
    reconciliation: &Reconciliation,
    subject_chain: Option<&SubjectChain>,
) -> Option<String> {
    subject_chain
        .and_then(|subject_chain| subject_chain.nodes.first())
        .map(format_subject_label)
        .or_else(|| reconciliation.required_fact.clone())
}

fn humanize_reason(reason: &str) -> String {
    if reason.contains('_') && !reason.contains(' ') {
        reason.replace('_', " ")
    } else {
        reason.to_string()
    }
}

fn write_prefixed_block(output: &mut String, prefix: &str, text: &str) {
    let mut lines = text.lines();
    if let Some(first) = lines.next() {
        writeln!(output, "{}{}", prefix, first).unwrap();
        for line in lines {
            writeln!(output, "{}", line).unwrap();
        }
    } else {
        writeln!(output, "{}", prefix.trim_end()).unwrap();
    }
}

fn write_evidence_block(output: &mut String, indent: &str, evidence: &[Evidence]) {
    if evidence.is_empty() {
        return;
    }

    if evidence.len() == 1 {
        writeln!(
            output,
            "{}evidence: {}",
            indent,
            format_evidence_label(&evidence[0])
        )
        .unwrap();
        return;
    }

    writeln!(output, "{}evidence:", indent).unwrap();
    for evidence in evidence {
        writeln!(output, "{}  {}", indent, format_evidence_label(evidence)).unwrap();
    }
}

fn write_string_section(output: &mut String, title: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }

    writeln!(output, "{}:", title).unwrap();
    for value in values {
        writeln!(output, "  {}", value).unwrap();
    }
    writeln!(output).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChainEdge, ChainEdgeKind, Exception, PolicyResult, Producer, ReconciliationRisk};

    fn source_evidence(file: &str, line: usize) -> Evidence {
        Evidence {
            kind: EvidenceKind::SourceSpan,
            file: Some(file.to_string()),
            line: Some(line),
            column: None,
            length: None,
            symbol: None,
            reason: None,
            json_pointer: None,
            resource: None,
            provider: None,
            value: None,
            event_id: None,
            time: None,
            source: None,
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: None,
        }
    }

    fn manifest_evidence(file: &str, pointer: &str) -> Evidence {
        Evidence {
            kind: EvidenceKind::RenderedManifestPointer,
            file: Some(file.to_string()),
            line: None,
            column: None,
            length: None,
            symbol: None,
            reason: None,
            json_pointer: Some(pointer.to_string()),
            resource: None,
            provider: None,
            value: None,
            event_id: None,
            time: None,
            source: None,
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: None,
        }
    }

    fn cloud_policy_evidence(file: &str, symbol: &str) -> Evidence {
        Evidence {
            kind: EvidenceKind::CloudPolicyPointer,
            file: Some(file.to_string()),
            line: None,
            column: None,
            length: None,
            symbol: Some(symbol.to_string()),
            reason: None,
            json_pointer: None,
            resource: None,
            provider: None,
            value: None,
            event_id: None,
            time: None,
            source: None,
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: None,
        }
    }

    fn put_object_capability() -> Capability {
        Capability {
            category: CapabilityCategory::ObjectStorageWrite,
            provider: Some("aws".to_string()),
            service: Some("s3".to_string()),
            action: Some("PutObject".to_string()),
            resource: Some("arn:aws:s3:::reports-prod/exports/*".to_string()),
            constraints: Default::default(),
        }
    }

    fn report_uploader() -> Subject {
        Subject {
            kind: SubjectKind::CodeFunction,
            id: "code.fn.report_uploader".to_string(),
            name: Some("ReportUploader.upload".to_string()),
            package: None,
        }
    }

    fn checkout_chain() -> SubjectChain {
        SubjectChain {
            schema: "reir.subject_chain.v0.1".to_string(),
            id: "chain.checkout.upload".to_string(),
            kind: "deployment".to_string(),
            nodes: vec![
                report_uploader(),
                Subject {
                    kind: SubjectKind::ContainerImage,
                    id: "image.checkout".to_string(),
                    name: Some("registry.example.com/checkout@sha256:abc...".to_string()),
                    package: None,
                },
                Subject {
                    kind: SubjectKind::K8sDeployment,
                    id: "deployment.checkout".to_string(),
                    name: Some("Deployment/prod/checkout-api".to_string()),
                    package: None,
                },
                Subject {
                    kind: SubjectKind::K8sServiceAccount,
                    id: "sa.checkout".to_string(),
                    name: Some("ServiceAccount/prod/checkout-api".to_string()),
                    package: None,
                },
                Subject {
                    kind: SubjectKind::CloudRole,
                    id: "role.checkout".to_string(),
                    name: Some("arn:aws:iam::123456789012:role/checkout-prod".to_string()),
                    package: None,
                },
            ],
            edges: vec![ChainEdge {
                kind: ChainEdgeKind::RunsAs,
                from: 2,
                to: 3,
            }],
            evidence: vec![
                manifest_evidence(
                    "rendered/prod/deployment.yaml",
                    "/spec/template/spec/serviceAccountName",
                ),
                cloud_policy_evidence("terraform/prod/iam.tf", "aws_iam_role.checkout_prod"),
            ],
        }
    }

    fn missing_reconciliation() -> Reconciliation {
        Reconciliation {
            schema: "reir.reconciliation.v0.1".to_string(),
            id: "recon.missing.upload".to_string(),
            kind: ReconciliationKind::MissingCapability,
            status: ReconciliationStatus::Fail,
            target: None,
            subject_chain: Some("chain.checkout.upload".to_string()),
            required_fact: Some("fact.req.upload".to_string()),
            granted_facts: vec![],
            observed_fact: None,
            capability: Some(put_object_capability()),
            risk: Some(ReconciliationRisk {
                class: RiskClass::Availability,
                severity: RiskSeverity::High,
                reason: Some(
                    "target role does not grant s3:PutObject for the required resource".to_string(),
                ),
            }),
            evidence: vec![source_evidence("src/report_upload.rs", 88)],
        }
    }

    #[test]
    fn capability_subject_and_evidence_helpers_are_human_readable() {
        let capability = put_object_capability();
        let subject = Subject {
            kind: SubjectKind::CodeFunction,
            id: "code.fn.upload".to_string(),
            name: Some("ReportUploader.upload".to_string()),
            package: Some("checkout".to_string()),
        };
        let evidence = manifest_evidence(
            "rendered/prod/deployment.yaml",
            "/spec/template/spec/serviceAccountName",
        );

        assert_eq!(
            format_capability_label(&capability),
            "aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*"
        );
        assert_eq!(
            format_subject_label(&subject),
            "checkout::ReportUploader.upload"
        );
        assert_eq!(
            format_evidence_label(&evidence),
            "rendered/prod/deployment.yaml#/spec/template/spec/serviceAccountName"
        );
    }

    #[test]
    fn formats_reconciliations_human_summary() {
        let excess = Reconciliation {
            schema: "reir.reconciliation.v0.1".to_string(),
            id: "recon.excess.role".to_string(),
            kind: ReconciliationKind::ExcessCapability,
            status: ReconciliationStatus::Warn,
            target: None,
            subject_chain: None,
            required_fact: None,
            granted_facts: vec!["role/checkout-prod".to_string()],
            observed_fact: None,
            capability: Some(Capability {
                category: CapabilityCategory::ObjectStorageRead,
                provider: Some("aws".to_string()),
                service: Some("s3".to_string()),
                action: Some("GetObject".to_string()),
                resource: Some("arn:aws:s3:::reports-prod/exports/*".to_string()),
                constraints: Default::default(),
            }),
            risk: None,
            evidence: vec![],
        };
        let covered = Reconciliation {
            schema: "reir.reconciliation.v0.1".to_string(),
            id: "recon.covered.upload".to_string(),
            kind: ReconciliationKind::Covered,
            status: ReconciliationStatus::Pass,
            target: None,
            subject_chain: None,
            required_fact: Some("fact.req.covered".to_string()),
            granted_facts: vec!["fact.grant.covered".to_string()],
            observed_fact: None,
            capability: Some(put_object_capability()),
            risk: None,
            evidence: vec![],
        };

        let rendered = format_reconciliations_human(&[missing_reconciliation(), excess, covered]);

        assert!(rendered.contains("status: fail"));
        assert!(rendered.contains(
            "missing capability:\n  aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*"
        ));
        assert!(rendered.contains("required by: fact.req.upload"));
        assert!(rendered.contains("evidence: src/report_upload.rs:88"));
        assert!(
            rendered.contains(
                "excess capability:\n  aws.s3.GetObject\n  granted to: role/checkout-prod"
            )
        );
        assert!(rendered.contains("summary: 1 covered, 1 missing, 1 excess"));
    }

    #[test]
    fn formats_diff_using_spec_style() {
        let diff = Diff {
            schema: "reir.diff.v0.1".to_string(),
            id: "diff.bundle".to_string(),
            items: vec![
                DiffItem {
                    kind: DiffItemKind::FactAdded,
                    id: "fact.req.upload".to_string(),
                    subject: None,
                    description: Some(
                        "required capability\n  code: ReportUploader.upload\n  capability: aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*\n  evidence: src/report_upload.rs:88"
                            .to_string(),
                    ),
                    evidence: vec![],
                },
                DiffItem {
                    kind: DiffItemKind::ReconciliationAdded,
                    id: "recon.missing.upload".to_string(),
                    subject: None,
                    description: Some(
                        "reconciliation failure\n  kind: missing_capability\n  target: prod checkout-api\n  chain: ReportUploader.upload -> checkout-api image -> Deployment/prod/checkout-api -> ServiceAccount/checkout-api -> IAM role checkout-prod\n  reason: role does not grant s3:PutObject"
                            .to_string(),
                    ),
                    evidence: vec![],
                },
            ],
        };

        let expected = "REIR DIFF\n\n+ required capability\n  code: ReportUploader.upload\n  capability: aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*\n  evidence: src/report_upload.rs:88\n\n+ reconciliation failure\n  kind: missing_capability\n  target: prod checkout-api\n  chain: ReportUploader.upload -> checkout-api image -> Deployment/prod/checkout-api -> ServiceAccount/checkout-api -> IAM role checkout-prod\n  reason: role does not grant s3:PutObject\n\nreview result:\n  fail\n";

        assert_eq!(format_diff_human(&diff), expected);
    }

    #[test]
    fn formats_diff_metadata_and_embedded_diff_changes() {
        let diff = Diff {
            schema: "reir.diff.v0.1".to_string(),
            id: "diff.bundle".to_string(),
            items: vec![
                DiffItem {
                    kind: DiffItemKind::OntologyChanged,
                    id: "ontology".to_string(),
                    subject: None,
                    description: Some(
                        "ontology changed from reir.capability_ontology.v0.1 to reir.capability_ontology.v0.2"
                            .to_string(),
                    ),
                    evidence: vec![],
                },
                DiffItem {
                    kind: DiffItemKind::SchemaChanged,
                    id: "schema".to_string(),
                    subject: None,
                    description: Some(
                        "schema changed from reir.bundle.v0.1 to reir.bundle.v0.2".to_string(),
                    ),
                    evidence: vec![],
                },
                DiffItem {
                    kind: DiffItemKind::ProfileRuleChanged,
                    id: "profile.prod".to_string(),
                    subject: None,
                    description: Some("profile rule changed".to_string()),
                    evidence: vec![],
                },
                DiffItem {
                    kind: DiffItemKind::DiffChanged,
                    id: "diff.accepted".to_string(),
                    subject: None,
                    description: Some("diff changed".to_string()),
                    evidence: vec![],
                },
            ],
        };

        let expected = "REIR DIFF\n\n~ diff changed\n\n~ profile rule changed\n\n~ schema changed from reir.bundle.v0.1 to reir.bundle.v0.2\n\n~ ontology changed from reir.capability_ontology.v0.1 to reir.capability_ontology.v0.2\n\nreview result:\n  fail\n";

        assert_eq!(format_diff_human(&diff), expected);
    }

    #[test]
    fn formats_slices_human_readably() {
        let slice = Slice {
            schema: "reir.slice.v0.1".to_string(),
            id: "slice.missing.upload".to_string(),
            kind: SliceKind::MissingCapabilitySlice,
            facts: vec![
                "ReportUploader.upload requires aws.s3.PutObject on reports-prod/exports/*"
                    .to_string(),
            ],
            edges: vec![
                "ReportUploader.upload -> checkout-api image -> Deployment/prod/checkout-api -> ServiceAccount/prod/checkout-api -> arn:aws:iam::123456789012:role/checkout-prod"
                    .to_string(),
            ],
            reconciliations: vec![
                "missing grant; deployment likely fails at runtime".to_string(),
            ],
            evidence: vec![source_evidence("src/report_upload.rs", 88)],
        };

        let expected = "MISSING CAPABILITY SLICE\n\nfacts:\n  ReportUploader.upload requires aws.s3.PutObject on reports-prod/exports/*\n\nedges:\n  ReportUploader.upload -> checkout-api image -> Deployment/prod/checkout-api -> ServiceAccount/prod/checkout-api -> arn:aws:iam::123456789012:role/checkout-prod\n\nreconciliations:\n  missing grant; deployment likely fails at runtime\n\nevidence:\n  src/report_upload.rs:88\n";

        assert_eq!(format_slices_human(&[slice]), expected);
    }

    #[test]
    fn formats_reconciliation_report_using_mvp_layout() {
        let expected = "REIR CAPABILITY RECONCILIATION\n\nstatus: fail\n\nmissing capability:\n  aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*\n\nrequired by:\n  ReportUploader.upload\n  evidence: src/report_upload.rs:88\n\ndeployment chain:\n  image: registry.example.com/checkout@sha256:abc...\n  workload: Deployment/prod/checkout-api\n  service account: ServiceAccount/prod/checkout-api\n  role: arn:aws:iam::123456789012:role/checkout-prod\n  evidence:\n    rendered/prod/deployment.yaml#/spec/template/spec/serviceAccountName\n    terraform/prod/iam.tf aws_iam_role.checkout_prod\n\nreason:\n  target role does not grant s3:PutObject for the required resource\n\nimpact:\n  deployment likely fails at runtime when ReportUploader.upload runs\n\n---\nsummary: 0 covered, 1 missing, 0 excess\n";

        assert_eq!(
            format_reconciliation_report(&[missing_reconciliation()], &[checkout_chain()]),
            expected
        );
    }

    #[test]
    fn formats_bundle_summary_and_policy_results() {
        let bundle = Bundle {
            producers: vec![Producer {
                name: "rsscript".to_string(),
                version: "0.1.0".to_string(),
                adapter: None,
                adapter_version: None,
                source: None,
            }],
            subjects: vec![report_uploader()],
            subject_chains: vec![checkout_chain()],
            facts: vec![],
            edges: vec![],
            reconciliations: vec![missing_reconciliation()],
            slices: vec![Slice {
                schema: "reir.slice.v0.1".to_string(),
                id: "slice.missing.upload".to_string(),
                kind: SliceKind::MissingCapabilitySlice,
                facts: vec![],
                edges: vec![],
                reconciliations: vec![],
                evidence: vec![],
            }],
            policy_results: vec![PolicyResult {
                schema: "reir.policy_result.v0.1".to_string(),
                id: "policy.web-service.missing".to_string(),
                kind: "budget".to_string(),
                status: PolicyStatus::Fail,
                policy: Some("max_missing_capabilities".to_string()),
                subject: None,
                reconciliation: Some("recon.missing.upload".to_string()),
                reason: Some("max_missing_capabilities exceeded: 1 > 0".to_string()),
                evidence: vec![],
            }],
            profiles: vec![Profile {
                kind: "web-service".to_string(),
                allow: HashMap::new(),
                budget: None,
            }],
            diffs: vec![Diff {
                schema: "reir.diff.v0.1".to_string(),
                id: "diff.bundle".to_string(),
                items: vec![],
            }],
            exceptions: vec![Exception {
                id: "ex-1".to_string(),
                accepted_by: "reviewer@example.com".to_string(),
                reason: "accepted".to_string(),
                expires: None,
                evidence: vec![],
            }],
            ..Bundle::new()
        };

        assert_eq!(
            format_bundle_summary(&bundle),
            "REIR BUNDLE SUMMARY\nproducers: 1\nsubjects: 1\nsubject chains: 1\nfacts: 0\nedges: 0\nreconciliations: 1\nslices: 1\npolicy results: 1\nprofiles: 1\ndiffs: 1\nexceptions: 1\n"
        );
        assert_eq!(
            format_policy_results_human(&bundle.policy_results),
            "REIR POLICY EVALUATION\n\nfail: max_missing_capabilities exceeded: 1 > 0\n"
        );
    }
}
