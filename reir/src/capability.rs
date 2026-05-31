use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
pub struct Capability {
    pub category: CapabilityCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub constraints: HashMap<String, String>,
}

string_enum! {
    /// A capability category defined by the REIR ontology.
    pub enum CapabilityCategory {
        #[serde(rename = "network.client")]
        NetworkClient => "network.client",
        #[serde(rename = "network.server")]
        NetworkServer => "network.server",
        #[serde(rename = "network.public_ingress")]
        NetworkPublicIngress => "network.public_ingress",
        #[serde(rename = "network.egress")]
        NetworkEgress => "network.egress",
        #[serde(rename = "filesystem.read")]
        FilesystemRead => "filesystem.read",
        #[serde(rename = "filesystem.write")]
        FilesystemWrite => "filesystem.write",
        #[serde(rename = "object_storage.read")]
        ObjectStorageRead => "object_storage.read",
        #[serde(rename = "object_storage.write")]
        ObjectStorageWrite => "object_storage.write",
        #[serde(rename = "database.read")]
        DatabaseRead => "database.read",
        #[serde(rename = "database.write")]
        DatabaseWrite => "database.write",
        #[serde(rename = "queue.publish")]
        QueuePublish => "queue.publish",
        #[serde(rename = "queue.consume")]
        QueueConsume => "queue.consume",
        #[serde(rename = "secret.read")]
        SecretRead => "secret.read",
        #[serde(rename = "secret.write")]
        SecretWrite => "secret.write",
        #[serde(rename = "env.read")]
        EnvRead => "env.read",
        #[serde(rename = "process.spawn")]
        ProcessSpawn => "process.spawn",
        #[serde(rename = "identity.assume")]
        IdentityAssume => "identity.assume",
        #[serde(rename = "identity.grant")]
        IdentityGrant => "identity.grant",
        #[serde(rename = "runtime.native")]
        RuntimeNative => "runtime.native",
        #[serde(rename = "runtime.unsafe")]
        RuntimeUnsafe => "runtime.unsafe",
        #[serde(rename = "build.execute")]
        BuildExecute => "build.execute",
        #[serde(rename = "build.network")]
        BuildNetwork => "build.network",
        #[serde(rename = "container.privileged")]
        ContainerPrivileged => "container.privileged",
        #[serde(rename = "container.host_access")]
        ContainerHostAccess => "container.host_access",
        #[serde(rename = "k8s.rbac.grant")]
        K8sRbacGrant => "k8s.rbac.grant",
        #[serde(rename = "storage.persistent_read")]
        StoragePersistentRead => "storage.persistent_read",
        #[serde(rename = "storage.persistent_write")]
        StoragePersistentWrite => "storage.persistent_write",
        #[serde(rename = "telemetry.emit")]
        TelemetryEmit => "telemetry.emit",
        #[serde(rename = "unknown")]
        Unknown => "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_category_serializes_as_string() {
        assert_eq!(
            serde_json::to_string(&CapabilityCategory::NetworkClient).unwrap(),
            "\"network.client\""
        );
    }

    #[test]
    fn capability_category_unknown_deserializes_to_extension() {
        assert_eq!(
            serde_json::from_str::<CapabilityCategory>("\"provider.custom\"").unwrap(),
            CapabilityCategory::Extension("provider.custom".to_owned())
        );
    }
}
