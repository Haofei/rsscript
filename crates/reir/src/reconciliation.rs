use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    Capability, CapabilityCategory, Evidence, EvidenceKind, Fact, FactRole, FactValue,
    GateFactDomain,
};

const MAX_RECONCILIATION_EVIDENCE: usize = 256;
const DEFAULT_MAX_RECONCILIATION_OPERATIONS: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconciliationLimits {
    pub max_candidate_comparisons: usize,
}

impl Default for ReconciliationLimits {
    fn default() -> Self {
        Self {
            max_candidate_comparisons: DEFAULT_MAX_RECONCILIATION_OPERATIONS,
        }
    }
}

#[derive(Default)]
struct ReconciliationBudget {
    candidate_comparisons: usize,
    limit: usize,
}

impl ReconciliationBudget {
    fn new(limits: ReconciliationLimits) -> Self {
        Self {
            candidate_comparisons: 0,
            limit: limits.max_candidate_comparisons,
        }
    }

    fn consume_candidate(&mut self) -> bool {
        let Some(next) = self.candidate_comparisons.checked_add(1) else {
            return false;
        };
        if next > self.limit {
            return false;
        }
        self.candidate_comparisons = next;
        true
    }
}

/// Compares facts with compatible capability keys and subject chains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconciliation {
    pub schema: String,
    pub id: String,
    pub kind: ReconciliationKind,
    pub status: ReconciliationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_chain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_fact: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_facts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_fact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<Capability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<ReconciliationRisk>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationRisk {
    pub class: RiskClass,
    pub severity: RiskSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationKind {
    Covered,
    MissingCapability,
    ExcessCapability,
    UnexpectedObservation,
    UnauthorizedObservation,
    UnusedCapability,
    PartialCoverage,
    UnknownCoverage,
    ChainIncomplete,
    Extension(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Pass,
    Fail,
    Warn,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Availability,
    Security,
    Drift,
    Modeling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    Critical,
    High,
    Medium,
    Low,
    Unknown,
}

/// Reconcile required facts against granted facts for a given capability.
/// Returns reconciliation results: missing capabilities, excess grants, covered.
pub fn reconcile_capabilities(required: &[Fact], granted: &[Fact]) -> Vec<Reconciliation> {
    reconcile_capabilities_for_target(required, granted, None)
}

pub fn reconcile_capabilities_with_limits(
    required: &[Fact],
    granted: &[Fact],
    limits: ReconciliationLimits,
) -> Vec<Reconciliation> {
    reconcile_capabilities_impl(required, granted, None, None, false, limits)
}

/// Reconcile required facts against granted facts and annotate results with a
/// target environment when the caller has one.
pub fn reconcile_capabilities_for_target(
    required: &[Fact],
    granted: &[Fact],
    target: Option<&str>,
) -> Vec<Reconciliation> {
    reconcile_capabilities_impl(
        required,
        granted,
        target,
        None,
        false,
        ReconciliationLimits::default(),
    )
}

pub fn reconcile_capabilities_for_gate(
    required: &[Fact],
    granted: &[Fact],
    target: Option<&str>,
    principal: Option<&str>,
) -> Vec<Reconciliation> {
    reconcile_capabilities_impl(
        required,
        granted,
        target,
        principal,
        true,
        ReconciliationLimits::default(),
    )
}

fn reconcile_capabilities_impl(
    required: &[Fact],
    granted: &[Fact],
    target: Option<&str>,
    principal: Option<&str>,
    validate_input: bool,
    limits: ReconciliationLimits,
) -> Vec<Reconciliation> {
    let mut results = Vec::new();
    let target = target.map(str::to_owned);
    let mut budget = ReconciliationBudget::new(limits);

    let required_with_capability: Vec<&Fact> = required
        .iter()
        .filter(|fact| {
            fact.kind == crate::FactKind::Capability
                && fact.role == Some(FactRole::Required)
                && fact.capability.is_some()
        })
        .collect();
    let granted_with_capability: Vec<&Fact> = granted
        .iter()
        .filter(|fact| {
            fact.kind == crate::FactKind::Capability
                && fact.role == Some(FactRole::Granted)
                && fact.capability.is_some()
        })
        .collect();
    let denied_with_capability: Vec<&Fact> = granted
        .iter()
        .filter(|fact| {
            fact.kind == crate::FactKind::Capability
                && fact.role == Some(FactRole::Denied)
                && fact.capability.is_some()
                && fact.value == FactValue::True
        })
        .collect();
    let required_index = CapabilityIndex::new(&required_with_capability);
    let granted_index = CapabilityIndex::new(&granted_with_capability);
    let denied_index = CapabilityIndex::new(&denied_with_capability);

    for required_fact in &required_with_capability {
        let required_capability = required_fact
            .capability
            .as_ref()
            .expect("filtered required fact should have capability");
        if required_fact.value == FactValue::False {
            continue;
        }
        let required_valid = !validate_input
            || required_fact
                .validate_for_gate(FactRole::Required, GateFactDomain::Requirement)
                .is_empty();
        let category_grants = granted_index.candidates(required_capability);
        let mut matching_denies = Vec::new();
        for denied_fact in denied_index.candidates(required_capability) {
            if !budget.consume_candidate() {
                return vec![reconciliation_budget_exceeded(target)];
            }
            if principal.is_none_or(|id| denied_fact.matches_gate_principal(id))
                && (!validate_input
                    || (!denied_fact.is_unknown_for_gate()
                        && denied_fact
                            .validate_for_gate(FactRole::Denied, GateFactDomain::DeploymentGrant)
                            .is_empty()))
                && denied_fact
                    .capability
                    .as_ref()
                    .is_some_and(|denied| capability_intersects(denied, required_capability))
            {
                matching_denies.push(denied_fact);
            }
        }
        let mut matching_grants = Vec::new();
        let mut compatible_grant_ids = Vec::new();
        let mut has_unknown_coverage = !required_valid || required_fact.is_unknown_for_gate();
        for granted_fact in category_grants {
            if !budget.consume_candidate() {
                return vec![reconciliation_budget_exceeded(target)];
            }
            let principal_matches =
                principal.is_none_or(|id| granted_fact.matches_gate_principal(id));
            let capability_matches = granted_fact
                .capability
                .as_ref()
                .is_some_and(|grant| capability_key_compatible(grant, required_capability));
            let input_valid = !validate_input
                || granted_fact
                    .validate_for_gate(FactRole::Granted, GateFactDomain::DeploymentGrant)
                    .is_empty();
            if capability_matches {
                compatible_grant_ids.push(granted_fact.id.clone());
            }
            if principal_matches
                && capability_matches
                && (granted_fact.is_unknown_for_gate() || !input_valid)
            {
                has_unknown_coverage = true;
            }
            if granted_fact.value == FactValue::True
                && principal_matches
                && !granted_fact.is_unknown_for_gate()
                && input_valid
                && granted_fact
                    .capability
                    .as_ref()
                    .is_some_and(|grant| capability_covers(grant, required_capability))
            {
                matching_grants.push(granted_fact);
            }
        }

        let deny_covers_requirement = matching_denies.iter().any(|fact| {
            fact.capability
                .as_ref()
                .is_some_and(|denied| capability_covers(denied, required_capability))
        });
        if !matching_grants.is_empty() && !matching_denies.is_empty() && !deny_covers_requirement {
            let mut evidence = Vec::new();
            append_evidence(&mut evidence, &required_fact.evidence);
            for fact in matching_grants.iter().chain(matching_denies.iter()) {
                append_evidence(&mut evidence, &fact.evidence);
            }
            results.push(Reconciliation {
                schema: "reir.reconciliation.v0.1".to_string(),
                id: format!("recon.partial.{}", required_fact.id),
                kind: ReconciliationKind::PartialCoverage,
                status: ReconciliationStatus::Fail,
                target: target.clone(),
                subject_chain: None,
                required_fact: Some(required_fact.id.clone()),
                granted_facts: matching_grants
                    .iter()
                    .chain(matching_denies.iter())
                    .map(|fact| fact.id.clone())
                    .collect(),
                observed_fact: None,
                capability: Some(required_capability.clone()),
                risk: Some(ReconciliationRisk {
                    class: RiskClass::Security,
                    severity: RiskSeverity::High,
                    reason: Some(
                        "explicit_deny_intersects_an_otherwise_covering_grant".to_string(),
                    ),
                }),
                evidence,
            });
        } else if matching_grants.is_empty() && has_unknown_coverage {
            results.push(Reconciliation {
                schema: "reir.reconciliation.v0.1".to_string(),
                id: format!("recon.unknown.{}", required_fact.id),
                kind: ReconciliationKind::UnknownCoverage,
                status: ReconciliationStatus::Unknown,
                target: target.clone(),
                subject_chain: None,
                required_fact: Some(required_fact.id.clone()),
                granted_facts: compatible_grant_ids,
                observed_fact: None,
                capability: Some(required_capability.clone()),
                risk: Some(ReconciliationRisk {
                    class: RiskClass::Modeling,
                    severity: RiskSeverity::Unknown,
                    reason: Some("gate_input_cannot_prove_capability_coverage".to_string()),
                }),
                evidence: budget_evidence(&required_fact.evidence),
            });
        } else if matching_grants.is_empty() || deny_covers_requirement {
            results.push(Reconciliation {
                schema: "reir.reconciliation.v0.1".to_string(),
                id: format!("recon.missing.{}", required_fact.id),
                kind: ReconciliationKind::MissingCapability,
                status: ReconciliationStatus::Fail,
                target: target.clone(),
                subject_chain: None,
                required_fact: Some(required_fact.id.clone()),
                granted_facts: Vec::new(),
                observed_fact: None,
                capability: Some(required_capability.clone()),
                risk: Some(ReconciliationRisk {
                    class: RiskClass::Availability,
                    severity: RiskSeverity::High,
                    reason: Some(
                        "deployment_target_does_not_grant_required_capability".to_string(),
                    ),
                }),
                evidence: budget_evidence(&required_fact.evidence),
            });
        } else {
            let mut evidence = Vec::new();
            append_evidence(&mut evidence, &required_fact.evidence);
            for grant in &matching_grants {
                append_evidence(&mut evidence, &grant.evidence);
            }

            results.push(Reconciliation {
                schema: "reir.reconciliation.v0.1".to_string(),
                id: format!("recon.covered.{}", required_fact.id),
                kind: ReconciliationKind::Covered,
                status: ReconciliationStatus::Pass,
                target: target.clone(),
                subject_chain: None,
                required_fact: Some(required_fact.id.clone()),
                granted_facts: matching_grants.iter().map(|fact| fact.id.clone()).collect(),
                observed_fact: None,
                capability: Some(required_capability.clone()),
                risk: None,
                evidence,
            });
        }
    }

    for granted_fact in &granted_with_capability {
        if granted_fact.value != FactValue::True
            || principal.is_some_and(|id| !granted_fact.matches_gate_principal(id))
            || granted_fact.is_unknown_for_gate()
            || (validate_input
                && !granted_fact
                    .validate_for_gate(FactRole::Granted, GateFactDomain::DeploymentGrant)
                    .is_empty())
        {
            continue;
        }
        let granted_capability = granted_fact
            .capability
            .as_ref()
            .expect("filtered granted fact should have capability");
        let mut matches_any_required = false;
        for required_fact in required_index.candidates(granted_capability) {
            if !budget.consume_candidate() {
                return vec![reconciliation_budget_exceeded(target)];
            }
            if required_fact.value == FactValue::True
                && required_fact
                    .capability
                    .as_ref()
                    .is_some_and(|required_capability| {
                        capability_covers(granted_capability, required_capability)
                    })
            {
                matches_any_required = true;
                break;
            }
        }

        if !matches_any_required {
            results.push(Reconciliation {
                schema: "reir.reconciliation.v0.1".to_string(),
                id: format!("recon.excess.{}", granted_fact.id),
                kind: ReconciliationKind::ExcessCapability,
                status: ReconciliationStatus::Warn,
                target: target.clone(),
                subject_chain: None,
                required_fact: None,
                granted_facts: vec![granted_fact.id.clone()],
                observed_fact: None,
                capability: Some(granted_capability.clone()),
                risk: Some(ReconciliationRisk {
                    class: RiskClass::Security,
                    severity: RiskSeverity::High,
                    reason: Some("granted_capability_has_no_matching_requirement".to_string()),
                }),
                evidence: budget_evidence(&granted_fact.evidence),
            });
        }
    }

    results
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExactCapabilityKey {
    category: String,
    provider: String,
    service: String,
    action: String,
    resource: String,
    constraints: Vec<(String, String)>,
}

struct CapabilityIndex<'a> {
    by_category: HashMap<String, Vec<&'a Fact>>,
    exact: HashMap<ExactCapabilityKey, Vec<&'a Fact>>,
    broad_by_category: HashMap<String, Vec<&'a Fact>>,
}

impl<'a> CapabilityIndex<'a> {
    fn new(facts: &[&'a Fact]) -> Self {
        let mut index = Self {
            by_category: HashMap::new(),
            exact: HashMap::new(),
            broad_by_category: HashMap::new(),
        };
        for fact in facts {
            let capability = fact
                .capability
                .as_ref()
                .expect("indexed fact should have capability");
            let category = capability_category_key(&capability.category);
            index
                .by_category
                .entry(category.clone())
                .or_default()
                .push(*fact);
            if let Some(key) = exact_capability_key(capability) {
                index.exact.entry(key).or_default().push(*fact);
            } else {
                index
                    .broad_by_category
                    .entry(category)
                    .or_default()
                    .push(*fact);
            }
        }
        index
    }

    fn candidates(&self, capability: &Capability) -> Vec<&'a Fact> {
        let category = capability_category_key(&capability.category);
        let Some(key) = exact_capability_key(capability) else {
            return self.by_category.get(&category).cloned().unwrap_or_default();
        };
        let exact = self.exact.get(&key).map(Vec::as_slice).unwrap_or_default();
        let broad = self
            .broad_by_category
            .get(&category)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut candidates = Vec::with_capacity(exact.len() + broad.len());
        candidates.extend_from_slice(exact);
        candidates.extend_from_slice(broad);
        candidates
    }
}

fn exact_capability_key(capability: &Capability) -> Option<ExactCapabilityKey> {
    let provider = exact_dimension(capability.provider.as_deref())?;
    let service = exact_dimension(capability.service.as_deref())?;
    let action = exact_dimension(capability.action.as_deref())?;
    let resource = exact_resource_dimension(capability.resource.as_deref())?;
    // Constraint intersection is not equality-based (a missing key may still
    // intersect), so constrained capabilities remain in the broad bucket.
    if !capability.constraints.is_empty() {
        return None;
    }
    Some(ExactCapabilityKey {
        category: capability_category_key(&capability.category),
        provider: provider.to_owned(),
        service: service.to_owned(),
        action: action.to_owned(),
        resource: resource.to_owned(),
        constraints: Vec::new(),
    })
}

fn exact_dimension(value: Option<&str>) -> Option<&str> {
    value.filter(|value| *value != "*")
}

fn exact_resource_dimension(value: Option<&str>) -> Option<&str> {
    exact_dimension(value).filter(|value| !value.ends_with('*'))
}

fn reconciliation_budget_exceeded(target: Option<String>) -> Reconciliation {
    Reconciliation {
        schema: "reir.reconciliation.v0.1".to_owned(),
        id: "recon.unknown.operation_budget".to_owned(),
        kind: ReconciliationKind::UnknownCoverage,
        status: ReconciliationStatus::Unknown,
        target,
        subject_chain: None,
        required_fact: None,
        granted_facts: Vec::new(),
        observed_fact: None,
        capability: None,
        risk: Some(ReconciliationRisk {
            class: RiskClass::Availability,
            severity: RiskSeverity::Unknown,
            reason: Some("reconciliation_candidate_comparison_budget_exceeded".to_owned()),
        }),
        evidence: vec![Evidence {
            kind: EvidenceKind::UnknownReason,
            file: None,
            line: None,
            column: None,
            length: None,
            symbol: Some("reconciliation_operation_budget".to_owned()),
            reason: Some(
                "reconciliation stopped before producing partial results because the candidate comparison budget was exceeded"
                    .to_owned(),
            ),
            json_pointer: None,
            resource: None,
            provider: None,
            value: Some("budget_exceeded".to_owned()),
            event_id: None,
            time: None,
            source: Some("reir.reconciliation".to_owned()),
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: None,
        }],
    }
}

fn capability_category_key(category: &CapabilityCategory) -> String {
    category.clone().into()
}

fn budget_evidence(evidence: &[Evidence]) -> Vec<Evidence> {
    let mut budgeted = Vec::new();
    append_evidence(&mut budgeted, evidence);
    budgeted
}

fn append_evidence(output: &mut Vec<Evidence>, evidence: &[Evidence]) {
    if evidence.is_empty() {
        return;
    }
    if output.len() >= MAX_RECONCILIATION_EVIDENCE {
        if output
            .last()
            .is_some_and(|entry| entry.value.as_deref() != Some("truncated"))
        {
            output.pop();
            output.push(evidence_budget_marker());
        }
        return;
    }
    let available = MAX_RECONCILIATION_EVIDENCE - output.len();
    if evidence.len() <= available {
        output.extend_from_slice(evidence);
        return;
    }

    if available > 1 {
        output.extend_from_slice(&evidence[..available - 1]);
    }
    output.push(evidence_budget_marker());
}

fn evidence_budget_marker() -> Evidence {
    Evidence {
        kind: EvidenceKind::UnknownReason,
        file: None,
        line: None,
        column: None,
        length: None,
        symbol: Some("reconciliation_evidence_budget".to_owned()),
        reason: Some(format!(
            "reconciliation evidence exceeded the {MAX_RECONCILIATION_EVIDENCE} item budget; additional evidence was omitted"
        )),
        json_pointer: None,
        resource: None,
        provider: None,
        value: Some("truncated".to_owned()),
        event_id: None,
        time: None,
        source: Some("reir.reconciliation".to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn capability_covers(granted: &Capability, required: &Capability) -> bool {
    granted.category == required.category
        && optional_field_covers(granted.provider.as_deref(), required.provider.as_deref())
        && optional_field_covers(granted.service.as_deref(), required.service.as_deref())
        && action_covers(granted.action.as_deref(), required.action.as_deref())
        && resource_covers(granted.resource.as_deref(), required.resource.as_deref())
        && constraints_cover(&granted.constraints, &required.constraints)
}

fn capability_intersects(left: &Capability, right: &Capability) -> bool {
    left.category == right.category
        && optional_field_intersects(left.provider.as_deref(), right.provider.as_deref())
        && optional_field_intersects(left.service.as_deref(), right.service.as_deref())
        && optional_field_intersects(left.action.as_deref(), right.action.as_deref())
        && resource_intersects(left.resource.as_deref(), right.resource.as_deref())
        && constraints_intersect(&left.constraints, &right.constraints)
}

fn optional_field_intersects(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, _) | (_, None) | (Some("*"), _) | (_, Some("*")) => true,
        (Some(left), Some(right)) => left == right,
    }
}

fn resource_intersects(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, _) | (_, None) | (Some("*"), _) | (_, Some("*")) => true,
        (Some(left), Some(right)) => {
            let left_prefix = left.strip_suffix('*').unwrap_or(left);
            let right_prefix = right.strip_suffix('*').unwrap_or(right);
            if left.ends_with('*') || right.ends_with('*') {
                left_prefix.starts_with(right_prefix) || right_prefix.starts_with(left_prefix)
            } else {
                left == right
            }
        }
    }
}

fn constraints_intersect(
    left: &std::collections::HashMap<String, String>,
    right: &std::collections::HashMap<String, String>,
) -> bool {
    left.iter().all(|(key, left_value)| {
        right.get(key).is_none_or(|right_value| {
            left_value == "*" || right_value == "*" || left_value == right_value
        })
    })
}

fn capability_key_compatible(granted: &Capability, required: &Capability) -> bool {
    granted.category == required.category
        && optional_field_covers(granted.provider.as_deref(), required.provider.as_deref())
        && optional_field_covers(granted.service.as_deref(), required.service.as_deref())
        && action_covers(granted.action.as_deref(), required.action.as_deref())
        && resource_covers(granted.resource.as_deref(), required.resource.as_deref())
}

fn constraints_cover(
    granted: &std::collections::HashMap<String, String>,
    required: &std::collections::HashMap<String, String>,
) -> bool {
    required.iter().all(|(key, required_value)| {
        granted
            .get(key)
            .is_some_and(|value| value == "*" || value == required_value)
    }) && granted.keys().all(|key| required.contains_key(key))
}

fn optional_field_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        // The requirement does not constrain this dimension.
        (_, None) => true,
        // An explicit wildcard grant covers any specific requirement.
        (Some("*"), Some(_)) => true,
        (Some(granted), Some(required)) => granted == required,
        // A grant that does not name this field is UNKNOWN, not a wildcard: it
        // cannot prove it covers a requirement that names a specific value.
        (None, Some(_)) => false,
    }
}

fn action_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        // An explicit wildcard grant covers any required action.
        (Some("*"), _) => true,
        (Some(granted), Some(required)) => granted == required,
        // Both unconstrained.
        (None, None) => true,
        // A grant with no action is unknown, not a wildcard — it does not cover a
        // requirement that names a specific action.
        (None, Some(_)) => false,
        // A grant scoped to a specific action does not cover an unconstrained
        // (broad) requirement.
        (Some(_), None) => false,
    }
}

fn resource_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        // Both unconstrained.
        (None, None) => true,
        // A grant with no resource is unknown, not a wildcard — it does not cover
        // a requirement that names a specific resource.
        (None, Some(_)) => false,
        // A narrow grant cannot prove coverage of a broad requirement.
        (Some(_), None) => false,
        (Some(granted), Some(required)) if granted == required => true,
        // Explicit prefix wildcard, e.g. `arn:aws:s3:::bucket/*`.
        (Some(granted), Some(required)) if granted.ends_with('*') => {
            required.starts_with(&granted[..granted.len() - 1])
        }
        (Some(_), Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AcquisitionMode, CapabilityCategory, Confidence, ConfidenceLevel, EvidenceKind, Fact,
        FactKind, FactRole, FactValue, Precision, Subject, SubjectKind,
    };
    use std::collections::HashMap;

    fn capability(service: &str) -> Capability {
        Capability {
            category: CapabilityCategory::ObjectStorageWrite,
            provider: Some("aws".to_owned()),
            service: Some(service.to_owned()),
            action: Some("s3:PutObject".to_owned()),
            resource: Some("arn:aws:s3:::reports-prod/*".to_owned()),
            constraints: HashMap::new(),
        }
    }

    #[test]
    fn capability_cover_requires_matching_provider_service_when_required_is_specific() {
        assert!(capability_covers(&capability("s3"), &capability("s3")));
        assert!(!capability_covers(
            &capability("dynamodb"),
            &capability("s3")
        ));
    }

    fn broad(category: CapabilityCategory) -> Capability {
        Capability {
            category,
            provider: None,
            service: None,
            action: None,
            resource: None,
            constraints: HashMap::new(),
        }
    }

    fn wildcard(category: CapabilityCategory, provider: &str) -> Capability {
        Capability {
            category,
            provider: Some(provider.to_owned()),
            service: Some("*".to_owned()),
            action: Some("*".to_owned()),
            resource: Some("*".to_owned()),
            constraints: HashMap::new(),
        }
    }

    fn exact_fact(id: usize, role: FactRole) -> Fact {
        let required = role == FactRole::Required;
        Fact {
            schema: "reir.fact.v0.1".to_owned(),
            id: format!("fact.{role:?}.{id}"),
            kind: FactKind::Capability,
            role: Some(role),
            subject: Subject {
                kind: if required {
                    SubjectKind::CodeFunction
                } else {
                    SubjectKind::CloudRole
                },
                id: format!("subject.{id}"),
                name: None,
                package: None,
            },
            capability: Some(Capability {
                category: CapabilityCategory::ObjectStorageRead,
                provider: Some("aws".to_owned()),
                service: Some("s3".to_owned()),
                action: Some("s3:GetObject".to_owned()),
                resource: Some(format!("arn:aws:s3:::bucket-{id}/object")),
                constraints: HashMap::new(),
            }),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Authoritative,
                source: Some("test".to_owned()),
            },
            acquisition_mode: AcquisitionMode::CloudPolicy,
            precision: Precision::ResourceScoped,
            evidence: Vec::new(),
            unknown_reason: None,
        }
    }

    #[test]
    fn reconciliation_budget_exhaustion_fails_closed_without_partial_results() {
        let required = exact_fact(1, FactRole::Required);
        let granted = exact_fact(1, FactRole::Granted);
        let results = reconcile_capabilities_with_limits(
            &[required],
            &[granted],
            ReconciliationLimits {
                max_candidate_comparisons: 0,
            },
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, ReconciliationKind::UnknownCoverage);
        assert_eq!(results[0].status, ReconciliationStatus::Unknown);
        assert_eq!(
            results[0]
                .risk
                .as_ref()
                .and_then(|risk| risk.reason.as_deref()),
            Some("reconciliation_candidate_comparison_budget_exceeded")
        );
    }

    #[test]
    fn exact_capability_index_keeps_comparisons_near_linear() {
        let required = (0..2_000)
            .map(|id| exact_fact(id, FactRole::Required))
            .collect::<Vec<_>>();
        let granted = (0..2_000)
            .map(|id| exact_fact(id, FactRole::Granted))
            .collect::<Vec<_>>();

        let results = reconcile_capabilities_with_limits(
            &required,
            &granted,
            ReconciliationLimits {
                max_candidate_comparisons: 5_000,
            },
        );

        assert_eq!(results.len(), required.len());
        assert!(
            results
                .iter()
                .all(|result| result.kind == ReconciliationKind::Covered)
        );
    }

    #[test]
    fn category_only_grant_does_not_cover_specific_requirement() {
        // The classic bypass: a broad `object_storage.write` grant must NOT
        // satisfy a requirement that names a specific provider/service/action/resource.
        let grant = broad(CapabilityCategory::ObjectStorageWrite);
        let required = capability("s3");
        assert!(!capability_covers(&grant, &required));
    }

    #[test]
    fn missing_grant_fields_are_unknown_not_wildcard() {
        // Each specific field on the requirement must be matched by the grant.
        let required = capability("s3");
        let mut missing_provider = capability("s3");
        missing_provider.provider = None;
        assert!(!capability_covers(&missing_provider, &required));
        let mut missing_action = capability("s3");
        missing_action.action = None;
        assert!(!capability_covers(&missing_action, &required));
        let mut missing_resource = capability("s3");
        missing_resource.resource = None;
        assert!(!capability_covers(&missing_resource, &required));
    }

    #[test]
    fn explicit_wildcard_grant_covers_specific_requirement() {
        // Breadth must be explicit (`*`), and then it does cover.
        let grant = wildcard(CapabilityCategory::ObjectStorageWrite, "aws");
        let required = capability("s3");
        assert!(capability_covers(&grant, &required));
    }

    #[test]
    fn category_level_grant_covers_category_level_requirement() {
        // Genuinely broad-on-both (e.g. runtime.native / network.client) still works.
        let grant = broad(CapabilityCategory::RuntimeNative);
        let required = broad(CapabilityCategory::RuntimeNative);
        assert!(capability_covers(&grant, &required));
    }

    #[test]
    fn category_only_requirement_reports_missing_against_specific_grant_only() {
        // A category-only requirement is broad; a specific grant does not cover it.
        let required = vec![Fact {
            schema: "reir.fact.v0.1".to_string(),
            id: "req.broad".to_string(),
            kind: FactKind::Capability,
            role: Some(FactRole::Required),
            subject: Subject {
                kind: SubjectKind::Package,
                id: "pkg".to_string(),
                name: None,
                package: None,
            },
            capability: Some(broad(CapabilityCategory::ObjectStorageWrite)),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Authoritative,
                source: None,
            },
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: Vec::new(),
            unknown_reason: None,
        }];
        let granted = vec![Fact {
            capability: Some(capability("s3")),
            role: Some(FactRole::Granted),
            subject: Subject {
                kind: SubjectKind::CloudRole,
                id: "role".to_string(),
                name: None,
                package: None,
            },
            ..required[0].clone()
        }];
        let results = reconcile_capabilities(&required, &granted);
        assert!(
            results
                .iter()
                .any(|r| matches!(r.kind, ReconciliationKind::MissingCapability))
        );
    }

    #[test]
    fn specific_resource_does_not_cover_unconstrained_requirement() {
        let grant = capability("s3");
        let mut required = grant.clone();
        required.resource = None;
        assert!(!capability_covers(&grant, &required));
    }

    #[test]
    fn constraints_must_be_at_least_as_permissive() {
        let mut required = capability("s3");
        required
            .constraints
            .insert("region".to_string(), "us-west-2".to_string());
        let mut grant = required.clone();
        assert!(capability_covers(&grant, &required));
        grant.constraints.clear();
        assert!(!capability_covers(&grant, &required));
        grant
            .constraints
            .insert("region".to_string(), "*".to_string());
        assert!(capability_covers(&grant, &required));
    }

    #[test]
    fn explicit_deny_takes_precedence_over_matching_grant() {
        let required = Fact {
            schema: "reir.fact.v0.1".to_string(),
            id: "required".to_string(),
            kind: FactKind::Capability,
            role: Some(FactRole::Required),
            subject: Subject {
                kind: SubjectKind::CodeFunction,
                id: "code::run".to_string(),
                name: None,
                package: None,
            },
            capability: Some(capability("s3")),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Computed,
                source: Some("test".to_string()),
            },
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::ResourceScoped,
            evidence: Vec::new(),
            unknown_reason: None,
        };
        let grant_subject = Subject {
            kind: SubjectKind::CloudRole,
            id: "role.prod".to_string(),
            name: None,
            package: None,
        };
        let grant = Fact {
            id: "grant".to_string(),
            role: Some(FactRole::Granted),
            subject: grant_subject.clone(),
            ..required.clone()
        };
        let deny = Fact {
            id: "deny".to_string(),
            role: Some(FactRole::Denied),
            subject: grant_subject,
            ..grant.clone()
        };
        let results = reconcile_capabilities(&[required], &[grant, deny]);
        assert!(
            results
                .iter()
                .any(|result| result.kind == ReconciliationKind::MissingCapability)
        );
        assert!(
            !results
                .iter()
                .any(|result| result.kind == ReconciliationKind::Covered)
        );
    }

    #[test]
    fn narrow_deny_prevents_a_broad_grant_from_reporting_full_coverage() {
        let required = Fact {
            schema: "reir.fact.v0.1".to_string(),
            id: "required".to_string(),
            kind: FactKind::Capability,
            role: Some(FactRole::Required),
            subject: Subject {
                kind: SubjectKind::CodeFunction,
                id: "code::run".to_string(),
                name: None,
                package: None,
            },
            capability: Some(wildcard(CapabilityCategory::ObjectStorageWrite, "aws")),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Computed,
                source: Some("test".to_string()),
            },
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::ResourceScoped,
            evidence: Vec::new(),
            unknown_reason: None,
        };
        let principal = Subject {
            kind: SubjectKind::CloudRole,
            id: "role.prod".to_string(),
            name: None,
            package: None,
        };
        let grant = Fact {
            id: "grant".to_string(),
            role: Some(FactRole::Granted),
            subject: principal.clone(),
            ..required.clone()
        };
        let deny = Fact {
            id: "deny".to_string(),
            role: Some(FactRole::Denied),
            subject: principal,
            capability: Some(capability("s3")),
            ..required.clone()
        };

        let results = reconcile_capabilities(&[required], &[grant, deny]);
        assert!(results.iter().any(|result| {
            result.kind == ReconciliationKind::PartialCoverage
                && result.status == ReconciliationStatus::Fail
        }));
        assert!(
            !results
                .iter()
                .any(|result| result.kind == ReconciliationKind::Covered)
        );
    }

    #[test]
    fn validated_reconciliation_rejects_invalid_narrow_deny_as_coverage_input() {
        let required = Fact {
            schema: "reir.fact.v0.1".to_string(),
            id: "required".to_string(),
            kind: FactKind::Capability,
            role: Some(FactRole::Required),
            subject: Subject {
                kind: SubjectKind::CodeFunction,
                id: "code::run".to_string(),
                name: None,
                package: None,
            },
            capability: Some(wildcard(CapabilityCategory::ObjectStorageWrite, "aws")),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Computed,
                source: Some("test".to_string()),
            },
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::ResourceScoped,
            evidence: vec![Evidence {
                kind: EvidenceKind::SourceSpan,
                file: Some("src/lib.rs".to_string()),
                line: Some(1),
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
            }],
            unknown_reason: None,
        };
        let principal = Subject {
            kind: SubjectKind::CloudRole,
            id: "role.prod".to_string(),
            name: None,
            package: None,
        };
        let grant = Fact {
            id: "grant".to_string(),
            role: Some(FactRole::Granted),
            subject: principal.clone(),
            acquisition_mode: AcquisitionMode::CloudPolicy,
            evidence: vec![Evidence {
                kind: EvidenceKind::ManifestPointer,
                file: Some("policy.json".to_string()),
                json_pointer: Some("/Statement/0".to_string()),
                principal: Some("role.prod".to_string()),
                ..required.evidence[0].clone()
            }],
            ..required.clone()
        };
        let deny = Fact {
            id: "deny".to_string(),
            role: Some(FactRole::Denied),
            subject: principal,
            capability: Some(capability("s3")),
            confidence: Confidence {
                level: ConfidenceLevel::Authoritative,
                source: None,
            },
            acquisition_mode: AcquisitionMode::CloudPolicy,
            evidence: Vec::new(),
            ..required.clone()
        };

        let results =
            reconcile_capabilities_for_gate(&[required], &[grant, deny], Some("prod"), None);

        assert!(
            results
                .iter()
                .any(|result| result.kind == ReconciliationKind::Covered)
        );
        assert!(
            !results
                .iter()
                .any(|result| result.kind == ReconciliationKind::PartialCoverage)
        );
    }

    #[test]
    fn code_function_cannot_act_as_deployment_grant_principal() {
        let mut fact = Fact {
            schema: "reir.fact.v0.1".to_string(),
            id: "grant".to_string(),
            kind: FactKind::Capability,
            role: Some(FactRole::Granted),
            subject: Subject {
                kind: SubjectKind::CodeFunction,
                id: "code::run".to_string(),
                name: None,
                package: None,
            },
            capability: Some(capability("s3")),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Authoritative,
                source: Some("test".to_string()),
            },
            acquisition_mode: AcquisitionMode::CloudPolicy,
            precision: Precision::ResourceScoped,
            evidence: Vec::new(),
            unknown_reason: None,
        };
        let errors = fact.validate_for_gate(FactRole::Granted, GateFactDomain::DeploymentGrant);
        assert!(
            errors
                .iter()
                .any(|error| error.reason.contains("deployment identity"))
        );
        fact.subject.kind = SubjectKind::CloudRole;
        assert!(
            fact.validate_for_gate(FactRole::Granted, GateFactDomain::DeploymentGrant)
                .iter()
                .all(|error| !error.reason.contains("deployment identity"))
        );
    }

    #[test]
    fn reconciliation_caps_aggregated_evidence_and_marks_truncation() {
        let mut required = Fact {
            schema: "reir.fact.v0.1".to_owned(),
            id: "required".to_owned(),
            kind: FactKind::Capability,
            role: Some(FactRole::Required),
            subject: Subject {
                kind: SubjectKind::CodeFunction,
                id: "code::run".to_owned(),
                name: None,
                package: None,
            },
            capability: Some(capability("s3")),
            value: FactValue::True,
            confidence: Confidence {
                level: ConfidenceLevel::Computed,
                source: Some("test".to_owned()),
            },
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::ResourceScoped,
            evidence: Vec::new(),
            unknown_reason: None,
        };
        required.evidence = (0..MAX_RECONCILIATION_EVIDENCE)
            .map(|line| Evidence {
                kind: EvidenceKind::SourceSpan,
                file: Some("src/lib.rs".to_owned()),
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
            })
            .collect();
        let grant = Fact {
            id: "grant".to_owned(),
            role: Some(FactRole::Granted),
            subject: Subject {
                kind: SubjectKind::CloudRole,
                id: "role.prod".to_owned(),
                name: None,
                package: None,
            },
            evidence: vec![required.evidence[0].clone()],
            ..required.clone()
        };

        let results = reconcile_capabilities(&[required], &[grant]);
        let covered = results
            .iter()
            .find(|result| result.kind == ReconciliationKind::Covered)
            .expect("coverage result");

        assert_eq!(covered.evidence.len(), MAX_RECONCILIATION_EVIDENCE);
        assert_eq!(
            covered
                .evidence
                .last()
                .and_then(|entry| entry.value.as_deref()),
            Some("truncated")
        );
    }
}
