//! Canonical capability taxonomy.
//!
//! Every `[[review.capability_bindings]]` `category` should be one of these
//! `domain.action` names. A fixed taxonomy is what makes capability review
//! meaningful and reproducible: reviewers (and the GitHub Action) can reason
//! about a stable set of powers (fs / network / database / process / secret …)
//! and assign each a default risk, instead of free-form strings that drift.

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
    /// Coarse domain (`network`, `fs`, …) for grouping.
    pub domain: &'static str,
    pub risk: CapabilityRisk,
    pub description: &'static str,
}

use CapabilityRisk::{High, Low, Medium};

/// The canonical capability categories. Add new powers here (and a fixture that
/// uses them) rather than inventing free-form category strings.
pub const CAPABILITY_CATEGORIES: &[CapabilityCategory] = &[
    cat("fs.read", "fs", Medium, "Read files or directory contents"),
    cat("fs.write", "fs", High, "Create, modify, or delete files"),
    cat("network.client", "network", High, "Make outbound network connections"),
    cat("network.connect", "network", High, "Open an outbound socket"),
    cat("network.listen", "network", High, "Bind and accept inbound connections"),
    cat("network.server", "network", High, "Serve requests on a bound address"),
    cat("http.client", "network", High, "Make outbound HTTP requests"),
    cat("database.read", "database", Medium, "Query a database"),
    cat("database.write", "database", High, "Mutate database state"),
    cat("env.read", "env", Medium, "Read environment variables"),
    cat("env.write", "env", High, "Set environment variables"),
    cat("process.args", "process", Low, "Read process arguments"),
    cat("process.spawn", "process", High, "Spawn or exec subprocesses"),
    cat("compute.hash", "compute", Low, "Compute a (non-secret) hash"),
    cat("crypto.sign", "crypto", High, "Produce cryptographic signatures/MACs"),
    cat("crypto.verify", "crypto", Medium, "Verify cryptographic signatures/MACs"),
    cat("secret.read", "secret", High, "Read secrets or credentials"),
    cat("identity.assume", "identity", High, "Assume an identity / capability role"),
    cat("time.now", "time", Low, "Read the wall clock"),
    cat("random.secure", "random", Low, "Draw cryptographically secure randomness"),
    cat("runtime.native", "runtime", High, "Cross an unclassified native boundary"),
];

const fn cat(
    name: &'static str,
    domain: &'static str,
    risk: CapabilityRisk,
    description: &'static str,
) -> CapabilityCategory {
    CapabilityCategory {
        name,
        domain,
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
    fn taxonomy_names_are_unique_and_dotted() {
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
        }
    }
}
