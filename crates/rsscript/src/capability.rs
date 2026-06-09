//! Canonical capability taxonomy.
//!
//! Every `[[review.capability_bindings]]` `category` should be one of these
//! `domain.action` names. A fixed taxonomy is what makes capability review
//! meaningful and reproducible: reviewers (and the GitHub Action) can reason
//! about a stable set of powers (filesystem / network / database / secret …)
//! and assign each a default risk, instead of free-form strings that drift.
//!
//! These names mirror REIR's authoritative `reir::CapabilityCategory` so a
//! category that passes review here is recognized by the REIR policy layer
//! rather than degrading to an unrecognized `Extension`. The `taxonomy_is_subset_of_reir`
//! test enforces that alignment.

/// Risk a capability category carries by default, used to rank review effort and
/// drive target policy (e.g. denying high-risk powers in a `prod` target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityRisk {
    Low,
    Medium,
    High,
}

/// One canonical capability category.
#[derive(Debug, Clone, Copy)]
pub struct CapabilityCategory {
    /// `domain.action` name, e.g. `database.write`.
    pub name: &'static str,
    pub risk: CapabilityRisk,
    pub description: &'static str,
}

impl CapabilityCategory {
    /// Coarse domain (`network`, `filesystem`, …) for grouping.
    pub fn domain(&self) -> &'static str {
        self.name.split('.').next().unwrap_or(self.name)
    }
}

use CapabilityRisk::{High, Low, Medium};

/// The canonical capability categories — a risk-annotated mirror of REIR's
/// `CapabilityCategory`. Add new powers to REIR first, then here.
pub const CAPABILITY_CATEGORIES: &[CapabilityCategory] = &[
    cat("network.client", High, "Make outbound network connections"),
    cat("network.server", High, "Serve requests on a bound address"),
    cat("network.public_ingress", High, "Accept traffic from a public ingress"),
    cat("network.egress", High, "Send traffic to an external destination"),
    cat("filesystem.read", Medium, "Read files or directory contents"),
    cat("filesystem.write", High, "Create, modify, or delete files"),
    cat("object_storage.read", Medium, "Read from object storage"),
    cat("object_storage.write", High, "Write to object storage"),
    cat("object_storage.delete", High, "Delete from object storage"),
    cat("database.read", Medium, "Query a database"),
    cat("database.write", High, "Mutate database state"),
    cat("queue.publish", Medium, "Publish to a message queue"),
    cat("queue.consume", Medium, "Consume from a message queue"),
    cat("secret.read", High, "Read secrets or credentials"),
    cat("secret.write", High, "Write secrets or credentials"),
    cat("env.read", Medium, "Read environment variables"),
    cat("env.write", High, "Set environment variables"),
    cat("time.read", Low, "Read the wall clock"),
    cat("random.read", Low, "Draw randomness"),
    cat("compute.hash", Low, "Compute a (non-secret) hash"),
    cat("compute.regex", Low, "Evaluate a regular expression"),
    cat("process.args", Low, "Read process arguments"),
    cat("process.spawn", High, "Spawn or exec subprocesses"),
    cat("identity.assume", High, "Assume an identity / capability role"),
    cat("identity.grant", High, "Grant an identity / capability role"),
    cat("runtime.native", High, "Cross an unclassified native boundary"),
    cat("runtime.unsafe", High, "Execute unsafe runtime code"),
    cat("build.execute", High, "Execute code at build time"),
    cat("build.network", High, "Access the network at build time"),
    cat("container.privileged", High, "Run a privileged container"),
    cat("container.host_access", High, "Access the container host"),
    cat("k8s.rbac.grant", High, "Grant Kubernetes RBAC permissions"),
    cat("storage.persistent_read", Medium, "Read persistent storage"),
    cat("storage.persistent_write", High, "Write persistent storage"),
    cat("telemetry.emit", Low, "Emit telemetry"),
];

const fn cat(
    name: &'static str,
    risk: CapabilityRisk,
    description: &'static str,
) -> CapabilityCategory {
    CapabilityCategory {
        name,
        risk,
        description,
    }
}

/// Look up a canonical category by `domain.action` name.
pub fn capability_category(name: &str) -> Option<&'static CapabilityCategory> {
    CAPABILITY_CATEGORIES
        .iter()
        .find(|category| category.name == name)
}

/// Whether `name` is a recognized canonical capability category.
pub fn is_known_capability_category(name: &str) -> bool {
    capability_category(name).is_some()
}

/// Default risk for a category, or `High` for an unrecognized one (unknown powers
/// are treated as risky).
pub fn capability_risk(name: &str) -> CapabilityRisk {
    capability_category(name).map_or(CapabilityRisk::High, |category| category.risk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_categories_resolve_and_carry_risk() {
        assert!(is_known_capability_category("database.write"));
        assert_eq!(capability_risk("database.write"), CapabilityRisk::High);
        assert_eq!(capability_risk("compute.hash"), CapabilityRisk::Low);
        assert!(!is_known_capability_category("totally.madeup"));
        // Unknown powers default to High.
        assert_eq!(capability_risk("totally.madeup"), CapabilityRisk::High);
    }

    #[test]
    fn taxonomy_names_are_unique_dotted_and_match_domain() {
        let mut seen = std::collections::BTreeSet::new();
        for category in CAPABILITY_CATEGORIES {
            assert!(
                seen.insert(category.name),
                "duplicate capability category {}",
                category.name
            );
            assert!(
                category.name.contains('.'),
                "category {} must be `domain.action`",
                category.name
            );
            assert!(category.name.starts_with(category.domain()));
        }
    }

    /// Every canonical category must be recognized by REIR's authoritative
    /// `CapabilityCategory` (i.e. not fall back to `Extension`), so a category
    /// that passes review here is classified correctly by the REIR policy layer.
    #[test]
    fn taxonomy_is_subset_of_reir() {
        for category in CAPABILITY_CATEGORIES {
            let reir_category = reir::CapabilityCategory::try_from(category.name.to_string())
                .expect("conversion is infallible");
            assert!(
                !matches!(reir_category, reir::CapabilityCategory::Extension(_)),
                "capability category `{}` is not a canonical REIR category \
                 (it would degrade to Extension); align it with reir::CapabilityCategory",
                category.name
            );
        }
    }
}
