use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Evidence, Reconciliation, ReconciliationKind, Subject};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
        .filter(|reconciliation| {
            matches!(
                reconciliation.kind,
                ReconciliationKind::MissingCapability | ReconciliationKind::PartialCoverage
            )
        })
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

/// A gate policy file (`rss-policy.toml`): per-target capability-gate settings
/// with an optional `[default]` fallback, resolving to a `CiGatePolicy`.
///
/// ```toml
/// [default]
/// fail_on_missing = true
///
/// [target.prod]
/// fail_on_unknown = true
/// fail_on_excess = true
/// require_verified_capabilities = true
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatePolicyFile {
    #[serde(default)]
    pub default: TargetGatePolicy,
    #[serde(default)]
    pub target: HashMap<String, TargetGatePolicy>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetGatePolicy {
    pub fail_on_missing: Option<bool>,
    pub fail_on_unknown: Option<bool>,
    pub fail_on_excess: Option<bool>,
    pub require_verified_capabilities: Option<bool>,
    pub principal: Option<String>,
}

impl GatePolicyFile {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("invalid gate policy file: {error}"))
    }

    /// Resolve the effective gate policy for `target`: start from the built-in
    /// default, layer the `[default]` section, then the target-specific section.
    pub fn gate_policy_for(&self, target: Option<&str>) -> Result<crate::GatePolicy, String> {
        let mut policy = crate::GatePolicy::production();
        self.default.apply_to(&mut policy);
        if let Some(name) = target {
            let target_policy = self.target.get(name).ok_or_else(|| {
                format!("unknown gate policy target `{name}`; add a `[target.{name}]` section")
            })?;
            target_policy.apply_to(&mut policy);
        }
        Ok(policy)
    }

    pub fn principal_for(&self, target: &str) -> Result<&str, String> {
        let target_policy = self.target.get(target).ok_or_else(|| {
            format!("unknown gate policy target `{target}`; add a `[target.{target}]` section")
        })?;
        target_policy
            .principal
            .as_deref()
            .ok_or_else(|| format!("gate policy target `{target}` is missing required `principal`"))
    }
}

impl TargetGatePolicy {
    pub fn apply_to(&self, policy: &mut crate::GatePolicy) {
        if let Some(value) = self.fail_on_missing {
            policy.fail_on_missing = value;
        }
        if let Some(value) = self.fail_on_unknown {
            policy.fail_on_unknown = value;
        }
        if let Some(value) = self.fail_on_excess {
            policy.fail_on_excess = value;
        }
        if let Some(value) = self.require_verified_capabilities {
            policy.require_verified_capabilities = value;
        }
    }
}

#[cfg(test)]
mod gate_policy_tests {
    use super::*;

    #[test]
    fn target_section_overrides_default_and_builtin() {
        let file = GatePolicyFile::parse(
            r#"
[default]
fail_on_unknown = false

[target.prod]
fail_on_unknown = true
fail_on_excess = true
require_verified_capabilities = true
"#,
        )
        .expect("policy parses");

        let prod = file.gate_policy_for(Some("prod")).expect("known target");
        assert!(prod.fail_on_missing); // built-in default
        assert!(prod.fail_on_unknown);
        assert!(prod.fail_on_excess);
        assert!(prod.require_verified_capabilities);

        let error = file
            .gate_policy_for(Some("staging"))
            .expect_err("unknown target must be rejected");
        assert!(error.contains("unknown gate policy target `staging`"));
    }

    #[test]
    fn production_policy_is_fail_closed() {
        let policy = crate::GatePolicy::production();
        assert!(policy.fail_on_missing);
        assert!(policy.fail_on_unknown);
        assert!(policy.fail_on_excess);
        assert!(policy.require_verified_capabilities);
    }

    #[test]
    fn policy_files_reject_unknown_fields() {
        let error = GatePolicyFile::parse(
            r#"
[default]
fail_on_unkown = false
"#,
        )
        .expect_err("misspelled security policy fields must fail closed");

        assert!(error.contains("unknown field `fail_on_unkown`"), "{error}");
    }
}
