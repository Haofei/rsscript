use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Bundle, CapabilityCategory, EdgeKind, Evidence, FactKind, ReconciliationKind};

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
    EnvSlice,
    TimeSlice,
    RandomnessSlice,
    ComputeSlice,
    TelemetrySlice,
    ProcessSlice,
    AsyncSlice,
    DiagnosticSlice,
    PackageFeatureSlice,
    ProviderImplementationSlice,
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

        let entry = slice_entry(&mut grouped, slice_kind);
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

    for fact in &bundle.facts {
        let Some(slice_kind) = fact_slice_kind(fact) else {
            continue;
        };
        let entry = slice_entry(&mut grouped, slice_kind);
        entry.facts.push(fact.id.clone());
        entry.evidence.extend(fact.evidence.clone());
    }

    for edge in &bundle.edges {
        let Some(slice_kind) = edge_slice_kind(&edge.kind) else {
            continue;
        };
        let entry = slice_entry(&mut grouped, slice_kind);
        entry.edges.push(edge.id.clone());
        entry.evidence.extend(edge.evidence.clone());
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

fn slice_entry(grouped: &mut BTreeMap<String, Slice>, slice_kind: SliceKind) -> &mut Slice {
    let key = slice_kind_key(&slice_kind).to_string();
    grouped.entry(key.clone()).or_insert_with(|| Slice {
        schema: "reir.slice.v0.1".to_string(),
        id: format!("slice.{}", key),
        kind: slice_kind,
        facts: Vec::new(),
        edges: Vec::new(),
        reconciliations: Vec::new(),
        evidence: Vec::new(),
    })
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

fn fact_slice_kind(fact: &crate::Fact) -> Option<SliceKind> {
    match fact.kind {
        FactKind::PackageRisk | FactKind::DependencyRisk | FactKind::SupplyChain => {
            Some(SliceKind::PackageRiskSlice)
        }
        FactKind::PackageFeature => Some(SliceKind::PackageFeatureSlice),
        FactKind::ProviderImplementation => Some(SliceKind::ProviderImplementationSlice),
        FactKind::NativeBoundary
        | FactKind::UnsafeBoundary
        | FactKind::NativeModuleDeclaration
        | FactKind::NativeCargoFeature => Some(SliceKind::NativeUnsafeSlice),
        FactKind::BuildTimeExecution => Some(SliceKind::BuildTimeSlice),
        FactKind::NetworkExposure => Some(SliceKind::NetworkSlice),
        FactKind::AsyncBoundary => Some(SliceKind::AsyncSlice),
        FactKind::Diagnostic => Some(SliceKind::DiagnosticSlice),
        FactKind::SecretAccess => Some(SliceKind::SecretSlice),
        FactKind::StorageAccess => Some(SliceKind::StorageSlice),
        FactKind::Identity => Some(SliceKind::IdentitySlice),
        FactKind::Authorization => Some(SliceKind::RbacSlice),
        FactKind::Unknown => Some(SliceKind::UnknownSlice),
        FactKind::Capability => fact
            .capability
            .as_ref()
            .and_then(|capability| capability_slice_kind(&capability.category)),
        _ => None,
    }
}

fn capability_slice_kind(category: &CapabilityCategory) -> Option<SliceKind> {
    match category {
        CapabilityCategory::RuntimeNative | CapabilityCategory::RuntimeUnsafe => {
            Some(SliceKind::NativeUnsafeSlice)
        }
        CapabilityCategory::BuildExecute | CapabilityCategory::BuildNetwork => {
            Some(SliceKind::BuildTimeSlice)
        }
        CapabilityCategory::NetworkClient
        | CapabilityCategory::NetworkServer
        | CapabilityCategory::NetworkEgress => Some(SliceKind::NetworkSlice),
        CapabilityCategory::NetworkPublicIngress => Some(SliceKind::PublicIngressSlice),
        CapabilityCategory::FilesystemRead | CapabilityCategory::FilesystemWrite => {
            Some(SliceKind::FilesystemSlice)
        }
        CapabilityCategory::ObjectStorageRead
        | CapabilityCategory::ObjectStorageWrite
        | CapabilityCategory::ObjectStorageDelete => Some(SliceKind::ObjectStorageSlice),
        CapabilityCategory::DatabaseRead | CapabilityCategory::DatabaseWrite => {
            Some(SliceKind::DatabaseSlice)
        }
        CapabilityCategory::SecretRead | CapabilityCategory::SecretWrite => {
            Some(SliceKind::SecretSlice)
        }
        CapabilityCategory::EnvRead | CapabilityCategory::EnvWrite => Some(SliceKind::EnvSlice),
        CapabilityCategory::TimeRead => Some(SliceKind::TimeSlice),
        CapabilityCategory::RandomRead => Some(SliceKind::RandomnessSlice),
        CapabilityCategory::ComputeHash | CapabilityCategory::ComputeRegex => {
            Some(SliceKind::ComputeSlice)
        }
        CapabilityCategory::TelemetryEmit => Some(SliceKind::TelemetrySlice),
        CapabilityCategory::ProcessArgs | CapabilityCategory::ProcessSpawn => {
            Some(SliceKind::ProcessSlice)
        }
        CapabilityCategory::IdentityAssume | CapabilityCategory::IdentityGrant => {
            Some(SliceKind::IdentitySlice)
        }
        CapabilityCategory::K8sRbacGrant => Some(SliceKind::RbacSlice),
        CapabilityCategory::StoragePersistentRead | CapabilityCategory::StoragePersistentWrite => {
            Some(SliceKind::StorageSlice)
        }
        CapabilityCategory::Unknown | CapabilityCategory::Extension(_) => {
            Some(SliceKind::UnknownSlice)
        }
        _ => None,
    }
}

fn edge_slice_kind(kind: &EdgeKind) -> Option<SliceKind> {
    match kind {
        EdgeKind::CrossesNative | EdgeKind::CrossesUnsafe | EdgeKind::NormalizesToNativeFn => {
            Some(SliceKind::NativeUnsafeSlice)
        }
        EdgeKind::MountsSecret => Some(SliceKind::SecretSlice),
        EdgeKind::MountsVolume => Some(SliceKind::StorageSlice),
        EdgeKind::HasRbacBinding | EdgeKind::HasPolicyAttachment => Some(SliceKind::RbacSlice),
        EdgeKind::UnknownEdge => Some(SliceKind::UnknownSlice),
        _ => None,
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
        SliceKind::EnvSlice => "env",
        SliceKind::TimeSlice => "time",
        SliceKind::RandomnessSlice => "randomness",
        SliceKind::ComputeSlice => "compute",
        SliceKind::TelemetrySlice => "telemetry",
        SliceKind::ProcessSlice => "process",
        SliceKind::AsyncSlice => "async",
        SliceKind::DiagnosticSlice => "diagnostic",
        SliceKind::PackageFeatureSlice => "package_feature",
        SliceKind::ProviderImplementationSlice => "provider_implementation",
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
