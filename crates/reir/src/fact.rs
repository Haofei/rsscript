use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    AcquisitionMode, Capability, Confidence, ConfidenceLevel, Evidence, Precision, Subject,
    SubjectKind,
};

/// A review-relevant assertion about a subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub schema: String,
    pub id: String,
    pub kind: FactKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<FactRole>,
    pub subject: Subject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<Capability>,
    pub value: FactValue,
    pub confidence: Confidence,
    pub acquisition_mode: AcquisitionMode,
    pub precision: Precision,
    pub evidence: Vec<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateFactDomain {
    Requirement,
    DeploymentGrant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateInputError {
    pub fact_id: String,
    pub reason: String,
}

impl Fact {
    /// Validate a capability assertion before it is allowed to affect a gate.
    /// Unknown/partial values remain valid inputs, but cannot prove coverage.
    pub fn validate_for_gate(&self, role: FactRole, domain: GateFactDomain) -> Vec<GateInputError> {
        let mut errors = Vec::new();
        let mut reject = |reason: &str| {
            errors.push(GateInputError {
                fact_id: self.id.clone(),
                reason: reason.to_owned(),
            });
        };

        if self.kind != FactKind::Capability {
            reject("gate capability input must have kind `capability`");
        }
        if self.role != Some(role) {
            reject("gate capability input has the wrong or missing role");
        }
        if self.capability.is_none() {
            reject("gate capability input is missing `capability`");
        }
        if self.id.trim().is_empty() || self.subject.id.trim().is_empty() {
            reject("gate capability input has an empty fact or subject id");
        }
        if !subject_in_gate_domain(&self.subject.kind, domain) {
            reject(match domain {
                GateFactDomain::Requirement => {
                    "required capability subject is outside the requirement domain"
                }
                GateFactDomain::DeploymentGrant => {
                    "granted capability subject is not a deployment identity or environment"
                }
            });
        }

        let unknown = self.value.is_unknown_for_gate()
            || matches!(
                self.confidence.level,
                ConfidenceLevel::Unknown | ConfidenceLevel::NotRun | ConfidenceLevel::Partial
            );
        if unknown && self.unknown_reason.as_deref().is_none_or(str::is_empty) {
            reject("unknown or partial gate capability is missing `unknown_reason`");
        }
        if !unknown {
            if self.confidence.source.as_deref().is_none_or(str::is_empty) {
                reject("asserted gate capability is missing confidence source provenance");
            }
            if self.evidence.is_empty() {
                reject("asserted gate capability is missing evidence provenance");
            }
            if matches!(
                self.acquisition_mode,
                AcquisitionMode::Unknown | AcquisitionMode::NotRun | AcquisitionMode::Extension(_)
            ) {
                reject("asserted gate capability has an unusable acquisition mode");
            }
            if matches!(self.confidence.level, ConfidenceLevel::Extension(_)) {
                reject("asserted gate capability has an unsupported confidence level");
            }
        }

        errors
    }

    pub fn is_unknown_for_gate(&self) -> bool {
        self.value.is_unknown_for_gate()
            || self.unknown_reason.is_some()
            || matches!(
                self.confidence.level,
                ConfidenceLevel::Unknown
                    | ConfidenceLevel::NotRun
                    | ConfidenceLevel::Partial
                    | ConfidenceLevel::Extension(_)
            )
    }

    pub fn matches_gate_principal(&self, principal: &str) -> bool {
        self.subject.id == principal
            || self
                .evidence
                .iter()
                .any(|evidence| evidence.principal.as_deref() == Some(principal))
    }
}

impl FactValue {
    pub fn is_unknown_for_gate(&self) -> bool {
        matches!(self, Self::Unknown | Self::NotRun | Self::Partial)
    }
}

fn subject_in_gate_domain(kind: &SubjectKind, domain: GateFactDomain) -> bool {
    match domain {
        GateFactDomain::Requirement => matches!(
            kind,
            SubjectKind::Application
                | SubjectKind::Service
                | SubjectKind::Entrypoint
                | SubjectKind::CodeFile
                | SubjectKind::CodeRegion
                | SubjectKind::CodeModule
                | SubjectKind::CodeFunction
                | SubjectKind::CodePublicApi
                | SubjectKind::CodeInterfaceSymbol
                | SubjectKind::CodeType
                | SubjectKind::CodeProtocol
                | SubjectKind::CodeProtocolMethod
                | SubjectKind::CodeProtocolImpl
                | SubjectKind::Package
                | SubjectKind::PackageVersion
                | SubjectKind::PackageFeature
                | SubjectKind::Dependency
                | SubjectKind::DependencyEdge
                | SubjectKind::BuildArtifact
                | SubjectKind::BuildStep
                | SubjectKind::ContainerImage
                | SubjectKind::K8sDeployment
                | SubjectKind::RuntimeWorkload
                | SubjectKind::NativeBoundary
                | SubjectKind::NativeModule
                | SubjectKind::UnsafeBoundary
        ),
        GateFactDomain::DeploymentGrant => matches!(
            kind,
            SubjectKind::Service
                | SubjectKind::ContainerImage
                | SubjectKind::K8sCluster
                | SubjectKind::K8sNamespace
                | SubjectKind::K8sResource
                | SubjectKind::K8sDeployment
                | SubjectKind::K8sPodTemplate
                | SubjectKind::K8sService
                | SubjectKind::K8sServiceAccount
                | SubjectKind::TerraformWorkspace
                | SubjectKind::TerraformResource
                | SubjectKind::CloudAccount
                | SubjectKind::CloudProject
                | SubjectKind::CloudIdentity
                | SubjectKind::CloudRole
                | SubjectKind::CloudPolicy
                | SubjectKind::CloudPermission
                | SubjectKind::RuntimeProcess
                | SubjectKind::RuntimeWorkload
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactRole {
    Required,
    Granted,
    Observed,
    Denied,
    Allowed,
    Expected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum FactKind {
    Capability,
    Resource,
    Retention,
    Mutation,
    NativeBoundary,
    UnsafeBoundary,
    ProtocolDeclaration,
    ProtocolMethodContract,
    ProtocolImpl,
    ProtocolStaticCall,
    AsyncBoundary,
    Diagnostic,
    ModuleDeclaration,
    UseDeclaration,
    NativeModuleDeclaration,
    NativeCargoFeature,
    PublicContract,
    PackageFeature,
    ProviderImplementation,
    PackageRisk,
    DependencyRisk,
    BuildTimeExecution,
    SupplyChain,
    Identity,
    Authorization,
    NetworkExposure,
    SecretAccess,
    StorageAccess,
    RuntimeObservation,
    ProfileRule,
    PolicyResult,
    SubjectChain,
    Flow,
    Unknown,
    Extension(String),
}

impl From<String> for FactKind {
    fn from(value: String) -> Self {
        match value.as_str() {
            "capability" => Self::Capability,
            "resource" => Self::Resource,
            "retention" => Self::Retention,
            "mutation" => Self::Mutation,
            "native_boundary" => Self::NativeBoundary,
            "unsafe_boundary" => Self::UnsafeBoundary,
            "protocol_declaration" => Self::ProtocolDeclaration,
            "protocol_method_contract" => Self::ProtocolMethodContract,
            "protocol_impl" => Self::ProtocolImpl,
            "protocol_static_call" => Self::ProtocolStaticCall,
            "async_boundary" => Self::AsyncBoundary,
            "diagnostic" => Self::Diagnostic,
            "module_declaration" => Self::ModuleDeclaration,
            "use_declaration" => Self::UseDeclaration,
            "native_module_declaration" => Self::NativeModuleDeclaration,
            "native_cargo_feature" => Self::NativeCargoFeature,
            "public_contract" => Self::PublicContract,
            "package_feature" => Self::PackageFeature,
            "provider_implementation" => Self::ProviderImplementation,
            "package_risk" => Self::PackageRisk,
            "dependency_risk" => Self::DependencyRisk,
            "build_time_execution" => Self::BuildTimeExecution,
            "supply_chain" => Self::SupplyChain,
            "identity" => Self::Identity,
            "authorization" => Self::Authorization,
            "network_exposure" => Self::NetworkExposure,
            "secret_access" => Self::SecretAccess,
            "storage_access" => Self::StorageAccess,
            "runtime_observation" => Self::RuntimeObservation,
            "profile_rule" => Self::ProfileRule,
            "policy_result" => Self::PolicyResult,
            "subject_chain" => Self::SubjectChain,
            "flow" => Self::Flow,
            "unknown" => Self::Unknown,
            other => Self::Extension(other.to_owned()),
        }
    }
}

impl From<FactKind> for String {
    fn from(value: FactKind) -> Self {
        match value {
            FactKind::Capability => "capability".to_owned(),
            FactKind::Resource => "resource".to_owned(),
            FactKind::Retention => "retention".to_owned(),
            FactKind::Mutation => "mutation".to_owned(),
            FactKind::NativeBoundary => "native_boundary".to_owned(),
            FactKind::UnsafeBoundary => "unsafe_boundary".to_owned(),
            FactKind::ProtocolDeclaration => "protocol_declaration".to_owned(),
            FactKind::ProtocolMethodContract => "protocol_method_contract".to_owned(),
            FactKind::ProtocolImpl => "protocol_impl".to_owned(),
            FactKind::ProtocolStaticCall => "protocol_static_call".to_owned(),
            FactKind::AsyncBoundary => "async_boundary".to_owned(),
            FactKind::Diagnostic => "diagnostic".to_owned(),
            FactKind::ModuleDeclaration => "module_declaration".to_owned(),
            FactKind::UseDeclaration => "use_declaration".to_owned(),
            FactKind::NativeModuleDeclaration => "native_module_declaration".to_owned(),
            FactKind::NativeCargoFeature => "native_cargo_feature".to_owned(),
            FactKind::PublicContract => "public_contract".to_owned(),
            FactKind::PackageFeature => "package_feature".to_owned(),
            FactKind::ProviderImplementation => "provider_implementation".to_owned(),
            FactKind::PackageRisk => "package_risk".to_owned(),
            FactKind::DependencyRisk => "dependency_risk".to_owned(),
            FactKind::BuildTimeExecution => "build_time_execution".to_owned(),
            FactKind::SupplyChain => "supply_chain".to_owned(),
            FactKind::Identity => "identity".to_owned(),
            FactKind::Authorization => "authorization".to_owned(),
            FactKind::NetworkExposure => "network_exposure".to_owned(),
            FactKind::SecretAccess => "secret_access".to_owned(),
            FactKind::StorageAccess => "storage_access".to_owned(),
            FactKind::RuntimeObservation => "runtime_observation".to_owned(),
            FactKind::ProfileRule => "profile_rule".to_owned(),
            FactKind::PolicyResult => "policy_result".to_owned(),
            FactKind::SubjectChain => "subject_chain".to_owned(),
            FactKind::Flow => "flow".to_owned(),
            FactKind::Unknown => "unknown".to_owned(),
            FactKind::Extension(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactValue {
    True,
    False,
    Unknown,
    NotRun,
    Partial,
}

impl Serialize for FactValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::True => serializer.serialize_bool(true),
            Self::False => serializer.serialize_bool(false),
            Self::Unknown => serializer.serialize_str("unknown"),
            Self::NotRun => serializer.serialize_str("not_run"),
            Self::Partial => serializer.serialize_str("partial"),
        }
    }
}

impl<'de> Deserialize<'de> for FactValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FactValueVisitor;

        impl<'de> de::Visitor<'de> for FactValueVisitor {
            type Value = FactValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a boolean or recognized fact value string")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(if value {
                    FactValue::True
                } else {
                    FactValue::False
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "true" => Ok(FactValue::True),
                    "false" => Ok(FactValue::False),
                    "unknown" => Ok(FactValue::Unknown),
                    "not_run" => Ok(FactValue::NotRun),
                    "partial" => Ok(FactValue::Partial),
                    other => Err(E::unknown_variant(
                        other,
                        &["true", "false", "unknown", "not_run", "partial"],
                    )),
                }
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(FactValueVisitor)
    }
}
