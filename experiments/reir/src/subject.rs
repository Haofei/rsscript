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

string_enum! {
    /// A subject classification defined by the REIR spec.
    pub enum SubjectKind {
        #[serde(rename = "application")]
        Application => "application",
        #[serde(rename = "service")]
        Service => "service",
        #[serde(rename = "entrypoint")]
        Entrypoint => "entrypoint",
        #[serde(rename = "code.file")]
        CodeFile => "code.file",
        #[serde(rename = "code.region")]
        CodeRegion => "code.region",
        #[serde(rename = "code.module")]
        CodeModule => "code.module",
        #[serde(rename = "code.function")]
        CodeFunction => "code.function",
        #[serde(rename = "code.public_api")]
        CodePublicApi => "code.public_api",
        #[serde(rename = "code.interface_symbol")]
        CodeInterfaceSymbol => "code.interface_symbol",
        #[serde(rename = "code.type")]
        CodeType => "code.type",
        #[serde(rename = "code.protocol")]
        CodeProtocol => "code.protocol",
        #[serde(rename = "code.protocol_method")]
        CodeProtocolMethod => "code.protocol_method",
        #[serde(rename = "code.protocol_impl")]
        CodeProtocolImpl => "code.protocol_impl",
        #[serde(rename = "package")]
        Package => "package",
        #[serde(rename = "package.version")]
        PackageVersion => "package.version",
        #[serde(rename = "package.feature")]
        PackageFeature => "package.feature",
        #[serde(rename = "dependency")]
        Dependency => "dependency",
        #[serde(rename = "dependency.edge")]
        DependencyEdge => "dependency.edge",
        #[serde(rename = "build.artifact")]
        BuildArtifact => "build.artifact",
        #[serde(rename = "build.step")]
        BuildStep => "build.step",
        #[serde(rename = "container.image")]
        ContainerImage => "container.image",
        #[serde(rename = "k8s.cluster")]
        K8sCluster => "k8s.cluster",
        #[serde(rename = "k8s.namespace")]
        K8sNamespace => "k8s.namespace",
        #[serde(rename = "k8s.resource")]
        K8sResource => "k8s.resource",
        #[serde(rename = "k8s.deployment")]
        K8sDeployment => "k8s.deployment",
        #[serde(rename = "k8s.pod_template")]
        K8sPodTemplate => "k8s.pod_template",
        #[serde(rename = "k8s.service")]
        K8sService => "k8s.service",
        #[serde(rename = "k8s.ingress")]
        K8sIngress => "k8s.ingress",
        #[serde(rename = "k8s.service_account")]
        K8sServiceAccount => "k8s.service_account",
        #[serde(rename = "k8s.secret")]
        K8sSecret => "k8s.secret",
        #[serde(rename = "k8s.configmap")]
        K8sConfigmap => "k8s.configmap",
        #[serde(rename = "k8s.volume")]
        K8sVolume => "k8s.volume",
        #[serde(rename = "terraform.workspace")]
        TerraformWorkspace => "terraform.workspace",
        #[serde(rename = "terraform.resource")]
        TerraformResource => "terraform.resource",
        #[serde(rename = "cloud.account")]
        CloudAccount => "cloud.account",
        #[serde(rename = "cloud.project")]
        CloudProject => "cloud.project",
        #[serde(rename = "cloud.identity")]
        CloudIdentity => "cloud.identity",
        #[serde(rename = "cloud.role")]
        CloudRole => "cloud.role",
        #[serde(rename = "cloud.policy")]
        CloudPolicy => "cloud.policy",
        #[serde(rename = "cloud.permission")]
        CloudPermission => "cloud.permission",
        #[serde(rename = "runtime.process")]
        RuntimeProcess => "runtime.process",
        #[serde(rename = "runtime.workload")]
        RuntimeWorkload => "runtime.workload",
        #[serde(rename = "runtime.event")]
        RuntimeEvent => "runtime.event",
        #[serde(rename = "capability")]
        Capability => "capability",
        #[serde(rename = "native.boundary")]
        NativeBoundary => "native.boundary",
        #[serde(rename = "native.module")]
        NativeModule => "native.module",
        #[serde(rename = "unsafe.boundary")]
        UnsafeBoundary => "unsafe.boundary",
        #[serde(rename = "resource")]
        Resource => "resource",
        #[serde(rename = "profile")]
        Profile => "profile",
        #[serde(rename = "policy")]
        Policy => "policy",
        #[serde(rename = "exception")]
        Exception => "exception",
        #[serde(rename = "unknown")]
        Unknown => "unknown"
    }
}

string_enum! {
    /// A relationship inside a subject chain.
    pub enum ChainEdgeKind {
        #[serde(rename = "built_from")]
        BuiltFrom => "built_from",
        #[serde(rename = "built_as")]
        BuiltAs => "built_as",
        #[serde(rename = "packaged_as")]
        PackagedAs => "packaged_as",
        #[serde(rename = "published_as")]
        PublishedAs => "published_as",
        #[serde(rename = "deployed_as")]
        DeployedAs => "deployed_as",
        #[serde(rename = "runs_as")]
        RunsAs => "runs_as",
        #[serde(rename = "routes_to")]
        RoutesTo => "routes_to",
        #[serde(rename = "mounts")]
        Mounts => "mounts",
        #[serde(rename = "reads_secret")]
        ReadsSecret => "reads_secret",
        #[serde(rename = "uses_service_account")]
        UsesServiceAccount => "uses_service_account",
        #[serde(rename = "assumes_role")]
        AssumesRole => "assumes_role",
        #[serde(rename = "bound_to_role")]
        BoundToRole => "bound_to_role",
        #[serde(rename = "grants_permission")]
        GrantsPermission => "grants_permission",
        #[serde(rename = "observed_as")]
        ObservedAs => "observed_as",
        #[serde(rename = "unknown_link")]
        UnknownLink => "unknown_link"
    }
}

/// A Subject identifies the thing a fact or edge is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub kind: SubjectKind,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectChain {
    pub schema: String,
    pub id: String,
    pub kind: String,
    pub nodes: Vec<Subject>,
    pub edges: Vec<ChainEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainEdge {
    pub kind: ChainEdgeKind,
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Producer {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_kind_serializes_as_string() {
        assert_eq!(
            serde_json::to_string(&SubjectKind::CodeFunction).unwrap(),
            "\"code.function\""
        );
    }

    #[test]
    fn subject_kind_unknown_deserializes_to_extension() {
        assert_eq!(
            serde_json::from_str::<SubjectKind>("\"vendor.custom_subject\"").unwrap(),
            SubjectKind::Extension("vendor.custom_subject".to_owned())
        );
    }
}
