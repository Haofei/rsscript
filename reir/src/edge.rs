use serde::{Deserialize, Serialize};

use crate::{AcquisitionMode, Confidence, Evidence, Precision, Subject};

/// A relation between two subjects or between a subject and a capability/fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub schema: String,
    pub id: String,
    pub kind: EdgeKind,
    pub from: Subject,
    pub to: Subject,
    pub confidence: Confidence,
    pub acquisition_mode: AcquisitionMode,
    pub precision: Precision,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum EdgeKind {
    Calls,
    MayCall,
    ProtocolStaticCall,
    ImplementsProtocol,
    NormalizesToNativeFn,
    RequiresCapability,
    GrantsCapability,
    ObservesCapability,
    DeniesCapability,
    UsesResource,
    OpensResource,
    Retains,
    Mutates,
    CrossesNative,
    CrossesUnsafe,
    DependsOn,
    BuiltFrom,
    BuiltAs,
    DeployedAs,
    RunsAs,
    AssumesRole,
    RoutesTo,
    MountsSecret,
    MountsVolume,
    HasRbacBinding,
    HasPolicyAttachment,
    EmitsEvent,
    UnknownEdge,
    Extension(String),
}

impl From<String> for EdgeKind {
    fn from(value: String) -> Self {
        match value.as_str() {
            "calls" => Self::Calls,
            "may_call" => Self::MayCall,
            "protocol_static_call" => Self::ProtocolStaticCall,
            "implements_protocol" => Self::ImplementsProtocol,
            "normalizes_to_native_fn" => Self::NormalizesToNativeFn,
            "requires_capability" => Self::RequiresCapability,
            "grants_capability" => Self::GrantsCapability,
            "observes_capability" => Self::ObservesCapability,
            "denies_capability" => Self::DeniesCapability,
            "uses_resource" => Self::UsesResource,
            "opens_resource" => Self::OpensResource,
            "retains" => Self::Retains,
            "mutates" => Self::Mutates,
            "crosses_native" => Self::CrossesNative,
            "crosses_unsafe" => Self::CrossesUnsafe,
            "depends_on" => Self::DependsOn,
            "built_from" => Self::BuiltFrom,
            "built_as" => Self::BuiltAs,
            "deployed_as" => Self::DeployedAs,
            "runs_as" => Self::RunsAs,
            "assumes_role" => Self::AssumesRole,
            "routes_to" => Self::RoutesTo,
            "mounts_secret" => Self::MountsSecret,
            "mounts_volume" => Self::MountsVolume,
            "has_rbac_binding" => Self::HasRbacBinding,
            "has_policy_attachment" => Self::HasPolicyAttachment,
            "emits_event" => Self::EmitsEvent,
            "unknown_edge" => Self::UnknownEdge,
            other => Self::Extension(other.to_owned()),
        }
    }
}

impl From<EdgeKind> for String {
    fn from(value: EdgeKind) -> Self {
        match value {
            EdgeKind::Calls => "calls".to_owned(),
            EdgeKind::MayCall => "may_call".to_owned(),
            EdgeKind::ProtocolStaticCall => "protocol_static_call".to_owned(),
            EdgeKind::ImplementsProtocol => "implements_protocol".to_owned(),
            EdgeKind::NormalizesToNativeFn => "normalizes_to_native_fn".to_owned(),
            EdgeKind::RequiresCapability => "requires_capability".to_owned(),
            EdgeKind::GrantsCapability => "grants_capability".to_owned(),
            EdgeKind::ObservesCapability => "observes_capability".to_owned(),
            EdgeKind::DeniesCapability => "denies_capability".to_owned(),
            EdgeKind::UsesResource => "uses_resource".to_owned(),
            EdgeKind::OpensResource => "opens_resource".to_owned(),
            EdgeKind::Retains => "retains".to_owned(),
            EdgeKind::Mutates => "mutates".to_owned(),
            EdgeKind::CrossesNative => "crosses_native".to_owned(),
            EdgeKind::CrossesUnsafe => "crosses_unsafe".to_owned(),
            EdgeKind::DependsOn => "depends_on".to_owned(),
            EdgeKind::BuiltFrom => "built_from".to_owned(),
            EdgeKind::BuiltAs => "built_as".to_owned(),
            EdgeKind::DeployedAs => "deployed_as".to_owned(),
            EdgeKind::RunsAs => "runs_as".to_owned(),
            EdgeKind::AssumesRole => "assumes_role".to_owned(),
            EdgeKind::RoutesTo => "routes_to".to_owned(),
            EdgeKind::MountsSecret => "mounts_secret".to_owned(),
            EdgeKind::MountsVolume => "mounts_volume".to_owned(),
            EdgeKind::HasRbacBinding => "has_rbac_binding".to_owned(),
            EdgeKind::HasPolicyAttachment => "has_policy_attachment".to_owned(),
            EdgeKind::EmitsEvent => "emits_event".to_owned(),
            EdgeKind::UnknownEdge => "unknown_edge".to_owned(),
            EdgeKind::Extension(value) => value,
        }
    }
}
