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
        // A grant scoped to a specific resource covers an unconstrained requirement.
        (Some(_), None) => true,
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
        AcquisitionMode, CapabilityCategory, Confidence, ConfidenceLevel, Fact, FactKind, FactRole,
        FactValue, Precision, Subject, SubjectKind,
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
            ..required[0].clone()
        }];
        let results = reconcile_capabilities(&required, &granted);
        assert!(
            results
                .iter()
                .any(|r| matches!(r.kind, ReconciliationKind::MissingCapability))
        );
    }
}
