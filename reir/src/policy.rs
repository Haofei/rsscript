use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Evidence, Reconciliation, ReconciliationKind, Subject};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyResult {
    pub schema: String,
    pub id: String,
    pub kind: String,
    pub status: PolicyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyStatus {
    Pass,
    Fail,
    Warn,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exception {
    pub id: String,
    pub accepted_by: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub kind: String,
    #[serde(default)]
    pub allow: HashMap<String, ProfilePermission>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<ProfileBudget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub enum ProfilePermission {
    Bool(bool),
    Action(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBudget {
    #[serde(default)]
    pub max_missing_capabilities: usize,
    #[serde(default)]
    pub max_unknown_coverage: usize,
    #[serde(default)]
    pub max_excess_grants: usize,
}

/// Evaluate reconciliation results against a profile.
pub fn evaluate_policy(profile: &Profile, reconciliations: &[Reconciliation]) -> Vec<PolicyResult> {
    let budget = profile.budget.clone().unwrap_or(ProfileBudget {
        max_missing_capabilities: 0,
        max_unknown_coverage: 0,
        max_excess_grants: 0,
    });

    let missing: Vec<&Reconciliation> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::MissingCapability)
        .collect();
    let unknown: Vec<&Reconciliation> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::UnknownCoverage)
        .collect();
    let excess: Vec<&Reconciliation> = reconciliations
        .iter()
        .filter(|reconciliation| reconciliation.kind == ReconciliationKind::ExcessCapability)
        .collect();

    vec![
        budget_result(
            profile,
            "missing_capabilities",
            missing.len(),
            budget.max_missing_capabilities,
            missing
                .first()
                .map(|reconciliation| reconciliation.id.clone()),
        ),
        budget_result(
            profile,
            "unknown_coverage",
            unknown.len(),
            budget.max_unknown_coverage,
            unknown
                .first()
                .map(|reconciliation| reconciliation.id.clone()),
        ),
        budget_result(
            profile,
            "excess_grants",
            excess.len(),
            budget.max_excess_grants,
            excess
                .first()
                .map(|reconciliation| reconciliation.id.clone()),
        ),
    ]
}

fn budget_result(
    profile: &Profile,
    budget_name: &str,
    count: usize,
    max_allowed: usize,
    reconciliation: Option<String>,
) -> PolicyResult {
    let status = if count > max_allowed {
        PolicyStatus::Fail
    } else {
        PolicyStatus::Pass
    };
    let reason = if count > max_allowed {
        format!("max_{} exceeded: {} > {}", budget_name, count, max_allowed)
    } else {
        format!(
            "max_{} within budget: {} <= {}",
            budget_name, count, max_allowed
        )
    };

    PolicyResult {
        schema: "reir.policy_result.v0.1".to_string(),
        id: format!("policy.{}.{}", profile.kind, budget_name),
        kind: "policy_result".to_string(),
        status,
        policy: Some(profile.kind.clone()),
        subject: None,
        reconciliation,
        reason: Some(reason),
        evidence: Vec::new(),
    }
}
