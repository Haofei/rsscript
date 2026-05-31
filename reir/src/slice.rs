use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Bundle, Evidence, ReconciliationKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slice {
    pub schema: String,
    pub id: String,
    pub kind: SliceKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconciliations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceKind {
    MissingCapabilitySlice,
    ExcessCapabilitySlice,
    UnexpectedObservationSlice,
    NetworkSlice,
    PublicIngressSlice,
    ObjectStorageSlice,
    DatabaseSlice,
    SecretSlice,
    FilesystemSlice,
    IdentitySlice,
    RbacSlice,
    StorageSlice,
    BuildTimeSlice,
    NativeUnsafeSlice,
    PackageRiskSlice,
    RuntimeDriftSlice,
    SubjectChainSlice,
    DiffSlice,
    UnknownSlice,
    Extension(String),
}

/// Generate slices from reconciliation results.
pub fn slice_by_kind(bundle: &Bundle) -> Vec<Slice> {
    let mut grouped: BTreeMap<String, Slice> = BTreeMap::new();

    for reconciliation in &bundle.reconciliations {
        let Some(slice_kind) = reconciliation_slice_kind(&reconciliation.kind) else {
            continue;
        };

        let key = slice_kind_key(&slice_kind).to_string();
        let entry = grouped.entry(key.clone()).or_insert_with(|| Slice {
            schema: "reir.slice.v0.1".to_string(),
            id: format!("slice.{}", key),
            kind: slice_kind.clone(),
            facts: Vec::new(),
            edges: Vec::new(),
            reconciliations: Vec::new(),
            evidence: Vec::new(),
        });

        if let Some(required_fact) = &reconciliation.required_fact {
            entry.facts.push(required_fact.clone());
        }
        entry
            .facts
            .extend(reconciliation.granted_facts.iter().cloned());
        if let Some(observed_fact) = &reconciliation.observed_fact {
            entry.facts.push(observed_fact.clone());
        }
        entry.reconciliations.push(reconciliation.id.clone());
        entry.evidence.extend(reconciliation.evidence.clone());
    }

    grouped
        .into_values()
        .map(|mut slice| {
            slice.facts.sort();
            slice.facts.dedup();
            slice.edges.sort();
            slice.edges.dedup();
            slice.reconciliations.sort();
            slice.reconciliations.dedup();
            slice
        })
        .collect()
}

fn reconciliation_slice_kind(kind: &ReconciliationKind) -> Option<SliceKind> {
    match kind {
        ReconciliationKind::Covered => None,
        ReconciliationKind::MissingCapability => Some(SliceKind::MissingCapabilitySlice),
        ReconciliationKind::ExcessCapability => Some(SliceKind::ExcessCapabilitySlice),
        ReconciliationKind::UnexpectedObservation => Some(SliceKind::UnexpectedObservationSlice),
        ReconciliationKind::UnauthorizedObservation | ReconciliationKind::UnusedCapability => {
            Some(SliceKind::RuntimeDriftSlice)
        }
        ReconciliationKind::PartialCoverage | ReconciliationKind::UnknownCoverage => {
            Some(SliceKind::UnknownSlice)
        }
        ReconciliationKind::ChainIncomplete => Some(SliceKind::SubjectChainSlice),
        ReconciliationKind::Extension(_) => Some(SliceKind::UnknownSlice),
    }
}

fn slice_kind_key(kind: &SliceKind) -> &'static str {
    match kind {
        SliceKind::MissingCapabilitySlice => "missing_capability",
        SliceKind::ExcessCapabilitySlice => "excess_capability",
        SliceKind::UnexpectedObservationSlice => "unexpected_observation",
        SliceKind::NetworkSlice => "network",
        SliceKind::PublicIngressSlice => "public_ingress",
        SliceKind::ObjectStorageSlice => "object_storage",
        SliceKind::DatabaseSlice => "database",
        SliceKind::SecretSlice => "secret",
        SliceKind::FilesystemSlice => "filesystem",
        SliceKind::IdentitySlice => "identity",
        SliceKind::RbacSlice => "rbac",
        SliceKind::StorageSlice => "storage",
        SliceKind::BuildTimeSlice => "build_time",
        SliceKind::NativeUnsafeSlice => "native_unsafe",
        SliceKind::PackageRiskSlice => "package_risk",
        SliceKind::RuntimeDriftSlice => "runtime_drift",
        SliceKind::SubjectChainSlice => "subject_chain",
        SliceKind::DiffSlice => "diff",
        SliceKind::UnknownSlice => "unknown",
        SliceKind::Extension(_) => "extension",
    }
}
