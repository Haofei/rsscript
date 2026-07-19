use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $value:literal
            ),* $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(into = "String", try_from = "String")]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )*
            Extension(String),
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                match value {
                    $(
                        $name::$variant => $value.to_owned(),
                    )*
                    $name::Extension(value) => value,
                }
            }
        }

        impl TryFrom<String> for $name {
            type Error = std::convert::Infallible;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Ok(match value.as_str() {
                    $(
                        $value => Self::$variant,
                    )*
                    _ => Self::Extension(value),
                })
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_arn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

string_enum! {
    /// Evidence sources recognized by REIR.
    pub enum EvidenceKind {
        #[serde(rename = "source_span")]
        SourceSpan => "source_span",
        #[serde(rename = "interface_span")]
        InterfaceSpan => "interface_span",
        #[serde(rename = "manifest_pointer")]
        ManifestPointer => "manifest_pointer",
        #[serde(rename = "source_template_pointer")]
        SourceTemplatePointer => "source_template_pointer",
        #[serde(rename = "rendered_manifest_pointer")]
        RenderedManifestPointer => "rendered_manifest_pointer",
        #[serde(rename = "terraform_plan_pointer")]
        TerraformPlanPointer => "terraform_plan_pointer",
        #[serde(rename = "terraform_state_pointer")]
        TerraformStatePointer => "terraform_state_pointer",
        #[serde(rename = "cloud_policy_pointer")]
        CloudPolicyPointer => "cloud_policy_pointer",
        #[serde(rename = "lockfile_entry")]
        LockfileEntry => "lockfile_entry",
        #[serde(rename = "dependency_path")]
        DependencyPath => "dependency_path",
        #[serde(rename = "binding_manifest")]
        BindingManifest => "binding_manifest",
        #[serde(rename = "cargo_metadata")]
        CargoMetadata => "cargo_metadata",
        #[serde(rename = "package_metadata")]
        PackageMetadata => "package_metadata",
        #[serde(rename = "registry_metadata")]
        RegistryMetadata => "registry_metadata",
        #[serde(rename = "image_digest")]
        ImageDigest => "image_digest",
        #[serde(rename = "provenance_attestation")]
        ProvenanceAttestation => "provenance_attestation",
        #[serde(rename = "sbom_entry")]
        SbomEntry => "sbom_entry",
        #[serde(rename = "runtime_event")]
        RuntimeEvent => "runtime_event",
        #[serde(rename = "cloud_audit_event")]
        CloudAuditEvent => "cloud_audit_event",
        #[serde(rename = "k8s_audit_event")]
        K8sAuditEvent => "k8s_audit_event",
        #[serde(rename = "service_mesh_trace")]
        ServiceMeshTrace => "service_mesh_trace",
        #[serde(rename = "ebpf_observation")]
        EbpfObservation => "ebpf_observation",
        #[serde(rename = "sandbox_log")]
        SandboxLog => "sandbox_log",
        #[serde(rename = "ai_inference_trace")]
        AiInferenceTrace => "ai_inference_trace",
        #[serde(rename = "manual_attestation")]
        ManualAttestation => "manual_attestation",
        #[serde(rename = "unknown_reason")]
        UnknownReason => "unknown_reason"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confidence {
    pub level: ConfidenceLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

string_enum! {
    pub enum ConfidenceLevel {
        #[serde(rename = "authoritative")]
        Authoritative => "authoritative",
        #[serde(rename = "computed")]
        Computed => "computed",
        #[serde(rename = "declared")]
        Declared => "declared",
        #[serde(rename = "scanned")]
        Scanned => "scanned",
        #[serde(rename = "inferred")]
        Inferred => "inferred",
        #[serde(rename = "observed")]
        Observed => "observed",
        #[serde(rename = "partial")]
        Partial => "partial",
        #[serde(rename = "unknown")]
        Unknown => "unknown",
        #[serde(rename = "not_run")]
        NotRun => "not_run"
    }
}

string_enum! {
    pub enum AcquisitionMode {
        #[serde(rename = "compiler_contract")]
        CompilerContract => "compiler_contract",
        #[serde(rename = "normalized_interface")]
        NormalizedInterface => "normalized_interface",
        #[serde(rename = "package_metadata")]
        PackageMetadata => "package_metadata",
        #[serde(rename = "lockfile")]
        Lockfile => "lockfile",
        #[serde(rename = "binding_manifest")]
        BindingManifest => "binding_manifest",
        #[serde(rename = "rendered_manifest")]
        RenderedManifest => "rendered_manifest",
        #[serde(rename = "deployment_manifest")]
        DeploymentManifest => "deployment_manifest",
        #[serde(rename = "terraform_plan")]
        TerraformPlan => "terraform_plan",
        #[serde(rename = "terraform_state")]
        TerraformState => "terraform_state",
        #[serde(rename = "cloud_policy")]
        CloudPolicy => "cloud_policy",
        #[serde(rename = "cloud_authorization_analysis")]
        CloudAuthorizationAnalysis => "cloud_authorization_analysis",
        #[serde(rename = "runtime_observation")]
        RuntimeObservation => "runtime_observation",
        #[serde(rename = "source_scan")]
        SourceScan => "source_scan",
        #[serde(rename = "binary_scan")]
        BinaryScan => "binary_scan",
        #[serde(rename = "provenance_attestation")]
        ProvenanceAttestation => "provenance_attestation",
        #[serde(rename = "sbom")]
        Sbom => "sbom",
        #[serde(rename = "manual_declaration")]
        ManualDeclaration => "manual_declaration",
        #[serde(rename = "manual_exception")]
        ManualException => "manual_exception",
        #[serde(rename = "ai_inference")]
        AiInference => "ai_inference",
        #[serde(rename = "unknown")]
        Unknown => "unknown",
        #[serde(rename = "not_run")]
        NotRun => "not_run"
    }
}

string_enum! {
    pub enum Precision {
        #[serde(rename = "exact")]
        Exact => "exact",
        #[serde(rename = "resource_scoped")]
        ResourceScoped => "resource_scoped",
        #[serde(rename = "symbolic")]
        Symbolic => "symbolic",
        #[serde(rename = "category")]
        Category => "category",
        #[serde(rename = "presence")]
        Presence => "presence",
        #[serde(rename = "unknown")]
        Unknown => "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_kind_serializes_as_string() {
        assert_eq!(
            serde_json::to_string(&EvidenceKind::SourceSpan).unwrap(),
            "\"source_span\""
        );
    }

    #[test]
    fn confidence_level_unknown_deserializes_to_extension() {
        assert_eq!(
            serde_json::from_str::<ConfidenceLevel>("\"adapter_specific\"").unwrap(),
            ConfidenceLevel::Extension("adapter_specific".to_owned())
        );
    }
}
