use serde::Serialize;

use crate::{
    AcquisitionMode, Evidence, Fact, FactKind, FactRole, GateFactDomain, Reconciliation,
    ReconciliationKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Fail,
    Warn,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GatePolicy {
    pub fail_on_missing: bool,
    pub fail_on_unknown: bool,
    pub fail_on_excess: bool,
    pub require_verified_capabilities: bool,
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self {
            fail_on_missing: true,
            fail_on_unknown: false,
            fail_on_excess: false,
            require_verified_capabilities: false,
        }
    }
}

impl GatePolicy {
    pub fn production() -> Self {
        Self {
            fail_on_missing: true,
            fail_on_unknown: true,
            fail_on_excess: true,
            require_verified_capabilities: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateIssueKind {
    InvalidEvidence,
    MissingCapability,
    UnknownCapability,
    ExcessCapability,
    UnverifiedCapability,
}

impl GateIssueKind {
    pub fn rule_id(self) -> &'static str {
        match self {
            Self::InvalidEvidence => "invalid_evidence",
            Self::MissingCapability => "missing_capability",
            Self::UnknownCapability => "unknown_capability",
            Self::ExcessCapability => "excess_capability",
            Self::UnverifiedCapability => "unverified_capability",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateIssue {
    pub kind: GateIssueKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateDecision {
    pub status: GateStatus,
    pub valid_for_gating: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gating_reason: Option<String>,
    pub policy: GatePolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<GateIssue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<GateIssue>,
}

impl GateDecision {
    pub fn review_action(&self) -> &'static str {
        match self.status {
            GateStatus::Fail => "block",
            GateStatus::Warn | GateStatus::Unknown => "warn",
            GateStatus::Pass => "approve",
        }
    }

    pub fn review_reason(&self) -> String {
        if !self.blockers.is_empty() {
            return self
                .blockers
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
        }
        if !self.warnings.is_empty() {
            return self
                .warnings
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
        }
        "All required capabilities satisfy the effective gate policy.".to_string()
    }
}

pub fn decide_gate(
    required_facts: &[Fact],
    granted_facts: &[Fact],
    reconciliations: &[Reconciliation],
    policy: GatePolicy,
) -> GateDecision {
    decide_gate_impl(
        required_facts,
        granted_facts,
        reconciliations,
        policy,
        false,
    )
}

pub fn decide_validated_gate(
    required_facts: &[Fact],
    granted_facts: &[Fact],
    reconciliations: &[Reconciliation],
    policy: GatePolicy,
) -> GateDecision {
    decide_gate_impl(required_facts, granted_facts, reconciliations, policy, true)
}

fn decide_gate_impl(
    required_facts: &[Fact],
    granted_facts: &[Fact],
    reconciliations: &[Reconciliation],
    policy: GatePolicy,
    validate_input: bool,
) -> GateDecision {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    for fact in required_facts.iter().chain(granted_facts) {
        if fact.kind == FactKind::Diagnostic && fact.unknown_reason.is_some() {
            blockers.push(GateIssue {
                kind: GateIssueKind::InvalidEvidence,
                message: format!(
                    "Evidence is invalid because diagnostic `{}` reports an error.",
                    fact.id
                ),
                fact_id: Some(fact.id.clone()),
                capability: None,
                evidence: fact.evidence.clone(),
            });
        }
    }

    for (facts, role, domain) in validate_input
        .then_some([
            (
                required_facts,
                FactRole::Required,
                GateFactDomain::Requirement,
            ),
            (
                granted_facts,
                FactRole::Granted,
                GateFactDomain::DeploymentGrant,
            ),
        ])
        .into_iter()
        .flatten()
    {
        for fact in facts
            .iter()
            .filter(|fact| fact.kind == FactKind::Capability)
        {
            let expected_role = if domain == GateFactDomain::DeploymentGrant
                && fact.role == Some(FactRole::Denied)
            {
                FactRole::Denied
            } else {
                role.clone()
            };
            for error in fact.validate_for_gate(expected_role, domain) {
                blockers.push(GateIssue {
                    kind: GateIssueKind::InvalidEvidence,
                    message: format!("Invalid gate input `{}`: {}.", error.fact_id, error.reason),
                    fact_id: Some(error.fact_id),
                    capability: fact.capability.as_ref().map(capability_label),
                    evidence: fact.evidence.clone(),
                });
            }
        }
    }

    for reconciliation in reconciliations {
        let (kind, message) = match reconciliation.kind {
            ReconciliationKind::MissingCapability => (
                GateIssueKind::MissingCapability,
                "Required capability is not granted by the deployment target.",
            ),
            ReconciliationKind::PartialCoverage => (
                GateIssueKind::MissingCapability,
                "Required capability is only partially granted by the deployment target.",
            ),
            ReconciliationKind::ExcessCapability => (
                GateIssueKind::ExcessCapability,
                "Deployment grant exceeds the capabilities required by the code.",
            ),
            ReconciliationKind::UnknownCoverage => (
                GateIssueKind::UnknownCapability,
                "Capability coverage cannot be proven from the gate input.",
            ),
            _ => continue,
        };
        let issue = GateIssue {
            kind,
            message: message.to_string(),
            fact_id: reconciliation
                .required_fact
                .clone()
                .or_else(|| reconciliation.granted_facts.first().cloned()),
            capability: reconciliation.capability.as_ref().map(capability_label),
            evidence: reconciliation.evidence.clone(),
        };
        let fails = match kind {
            GateIssueKind::MissingCapability => policy.fail_on_missing,
            GateIssueKind::ExcessCapability => policy.fail_on_excess,
            GateIssueKind::UnknownCapability => policy.fail_on_unknown,
            _ => false,
        };
        if fails {
            blockers.push(issue);
        } else if matches!(
            kind,
            GateIssueKind::ExcessCapability | GateIssueKind::UnknownCapability
        ) {
            warnings.push(issue);
        }
    }

    for fact in required_facts
        .iter()
        .filter(|fact| is_capability_fact(fact, FactRole::Required))
    {
        let unknown = fact.is_unknown_for_gate();
        if unknown {
            let issue = GateIssue {
                kind: GateIssueKind::UnknownCapability,
                message: "Required capability has unknown coverage.".to_string(),
                fact_id: Some(fact.id.clone()),
                capability: fact.capability.as_ref().map(capability_label),
                evidence: fact.evidence.clone(),
            };
            if policy.fail_on_unknown {
                blockers.push(issue);
            } else {
                warnings.push(issue);
            }
            continue;
        }

        if policy.require_verified_capabilities && is_author_declared(fact) {
            blockers.push(GateIssue {
                kind: GateIssueKind::UnverifiedCapability,
                message: "Required capability is author-declared without independent verification."
                    .to_string(),
                fact_id: Some(fact.id.clone()),
                capability: fact.capability.as_ref().map(capability_label),
                evidence: fact.evidence.clone(),
            });
        }
    }

    deduplicate_issues(&mut blockers);
    deduplicate_issues(&mut warnings);
    let valid_for_gating = !blockers
        .iter()
        .any(|issue| issue.kind == GateIssueKind::InvalidEvidence);
    let status = if !blockers.is_empty() {
        GateStatus::Fail
    } else if !warnings.is_empty() {
        GateStatus::Warn
    } else {
        GateStatus::Pass
    };

    GateDecision {
        status,
        valid_for_gating,
        gating_reason: (!valid_for_gating).then(|| "error_diagnostics".to_string()),
        policy,
        blockers,
        warnings,
    }
}

fn is_capability_fact(fact: &Fact, role: FactRole) -> bool {
    fact.kind == FactKind::Capability && fact.role == Some(role) && fact.capability.is_some()
}

fn is_author_declared(fact: &Fact) -> bool {
    matches!(
        fact.acquisition_mode,
        AcquisitionMode::BindingManifest
            | AcquisitionMode::ManualDeclaration
            | AcquisitionMode::ManualException
    )
}

fn capability_label(capability: &crate::Capability) -> String {
    let mut parts = vec![format!("{:?}", capability.category)];
    parts.extend(
        [
            capability.provider.as_ref(),
            capability.service.as_ref(),
            capability.action.as_ref(),
            capability.resource.as_ref(),
        ]
        .into_iter()
        .flatten()
        .cloned(),
    );
    parts.join(" / ")
}

fn deduplicate_issues(issues: &mut Vec<GateIssue>) {
    issues.sort_by(|left, right| {
        (left.kind.rule_id(), &left.capability, &left.fact_id).cmp(&(
            right.kind.rule_id(),
            &right.capability,
            &right.fact_id,
        ))
    });
    let mut deduplicated: Vec<GateIssue> = Vec::with_capacity(issues.len());
    for issue in issues.drain(..) {
        let same_logical_issue = deduplicated.last_mut().filter(|existing| {
            existing.kind == issue.kind
                && if issue.capability.is_some() {
                    existing.capability == issue.capability
                } else {
                    existing.fact_id == issue.fact_id
                }
        });
        if let Some(existing) = same_logical_issue {
            existing.evidence.extend(issue.evidence);
            continue;
        }
        deduplicated.push(issue);
    }
    *issues = deduplicated;
}
