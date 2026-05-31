use serde::{Deserialize, Serialize};

use crate::{Capability, Evidence, Fact};

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

/// Reconcile required facts against granted facts and annotate results with a
/// target environment when the caller has one.
pub fn reconcile_capabilities_for_target(
    required: &[Fact],
    granted: &[Fact],
    target: Option<&str>,
) -> Vec<Reconciliation> {
    let mut results = Vec::new();
    let target = target.map(str::to_owned);

    let required_with_capability: Vec<&Fact> = required
        .iter()
        .filter(|fact| fact.capability.is_some())
        .collect();
    let granted_with_capability: Vec<&Fact> = granted
        .iter()
        .filter(|fact| fact.capability.is_some())
        .collect();

    for required_fact in &required_with_capability {
        let required_capability = required_fact
            .capability
            .as_ref()
            .expect("filtered required fact should have capability");
        let matching_grants: Vec<&Fact> = granted_with_capability
            .iter()
            .copied()
            .filter(|granted_fact| {
                granted_fact
                    .capability
                    .as_ref()
                    .is_some_and(|granted_capability| {
                        capability_covers(granted_capability, required_capability)
                    })
            })
            .collect();

        if matching_grants.is_empty() {
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
                evidence: required_fact.evidence.clone(),
            });
        } else {
            let mut evidence = required_fact.evidence.clone();
            for grant in &matching_grants {
                evidence.extend(grant.evidence.clone());
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
        let granted_capability = granted_fact
            .capability
            .as_ref()
            .expect("filtered granted fact should have capability");
        let matches_any_required = required_with_capability.iter().any(|required_fact| {
            required_fact
                .capability
                .as_ref()
                .is_some_and(|required_capability| {
                    capability_covers(granted_capability, required_capability)
                })
        });

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
                evidence: granted_fact.evidence.clone(),
            });
        }
    }

    results
}

fn capability_covers(granted: &Capability, required: &Capability) -> bool {
    granted.category == required.category
        && optional_field_covers(granted.provider.as_deref(), required.provider.as_deref())
        && optional_field_covers(granted.service.as_deref(), required.service.as_deref())
        && action_covers(granted.action.as_deref(), required.action.as_deref())
        && resource_covers(granted.resource.as_deref(), required.resource.as_deref())
}

fn optional_field_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        (_, None) => true,
        (None, Some(_)) => true,
        (Some(granted), Some(required)) => granted == required,
    }
}

fn action_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        (Some(granted), Some(required)) => granted == required,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

fn resource_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(granted), Some(required)) if granted == required => true,
        (Some(granted), Some(required)) if granted.ends_with('*') => {
            required.starts_with(&granted[..granted.len() - 1])
        }
        (Some(_), Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapabilityCategory;
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
}
