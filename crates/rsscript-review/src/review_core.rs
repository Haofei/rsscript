//! Pure interpretation of package-review evidence.
//!
//! This crate deliberately knows nothing about source files, manifests,
//! providers, artifact persistence, or authorization.  Producers collect
//! typed facts in [`PackageRiskEvidence`]; optional review consumers decide
//! how to present the resulting [`PackageRisk`].  It must not be used as an
//! execution permission or policy mechanism.

use serde::Serialize;

/// A neutral severity classification for review evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageRisk {
    Low,
    Elevated,
    High,
    Unknown,
}

/// The manifest-declared interpretation for native API evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NativeApiRiskPolicy {
    Elevated,
    High,
}

/// Facts collected while analysing a package for optional review output.
///
/// Each field is descriptive rather than authoritative: hosts and deployment
/// systems must make their own authorization decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageRiskEvidence {
    pub native_source_scan_complete: bool,
    pub expected_unknown: bool,
    pub unknown_review_functions: usize,
    pub unknown_external_bindings: usize,
    pub has_error_diagnostics: bool,
    pub has_boundary_changing_features: bool,
    pub has_unreviewed_native_behavior: bool,
    pub native_api_count: usize,
    pub native_api_risk: Option<NativeApiRiskPolicy>,
    pub has_native_package: bool,
    pub review_required_functions: usize,
}

impl Default for PackageRiskEvidence {
    fn default() -> Self {
        Self {
            native_source_scan_complete: true,
            expected_unknown: false,
            unknown_review_functions: 0,
            unknown_external_bindings: 0,
            has_error_diagnostics: false,
            has_boundary_changing_features: false,
            has_unreviewed_native_behavior: false,
            native_api_count: 0,
            native_api_risk: None,
            has_native_package: false,
            review_required_functions: 0,
        }
    }
}

/// Derives an optional review severity from neutral package facts.
///
/// The precedence is intentionally explicit: incomplete information wins over
/// every other signal, followed by hard boundary/error evidence, then native
/// and review-required evidence.
pub fn package_risk(evidence: &PackageRiskEvidence) -> PackageRisk {
    if !evidence.native_source_scan_complete
        || evidence.expected_unknown
        || evidence.unknown_review_functions > 0
        || evidence.unknown_external_bindings > 0
    {
        return PackageRisk::Unknown;
    }
    if evidence.has_error_diagnostics
        || evidence.has_boundary_changing_features
        || evidence.has_unreviewed_native_behavior
    {
        return PackageRisk::High;
    }
    if evidence.native_api_count > 0 {
        return evidence
            .native_api_risk
            .map_or(PackageRisk::High, |policy| match policy {
                NativeApiRiskPolicy::Elevated => PackageRisk::Elevated,
                NativeApiRiskPolicy::High => PackageRisk::High,
            });
    }
    if evidence.has_native_package || evidence.review_required_functions > 0 {
        PackageRisk::Elevated
    } else {
        PackageRisk::Low
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeApiRiskPolicy, PackageRisk, PackageRiskEvidence, package_risk};

    #[test]
    fn incomplete_evidence_takes_precedence() {
        let evidence = PackageRiskEvidence {
            native_source_scan_complete: false,
            has_error_diagnostics: true,
            ..PackageRiskEvidence::default()
        };
        assert_eq!(package_risk(&evidence), PackageRisk::Unknown);
    }

    #[test]
    fn boundary_evidence_is_high() {
        let evidence = PackageRiskEvidence {
            has_boundary_changing_features: true,
            ..PackageRiskEvidence::default()
        };
        assert_eq!(package_risk(&evidence), PackageRisk::High);
    }

    #[test]
    fn native_api_policy_is_explicit() {
        let evidence = PackageRiskEvidence {
            native_api_count: 1,
            native_api_risk: Some(NativeApiRiskPolicy::Elevated),
            ..PackageRiskEvidence::default()
        };
        assert_eq!(package_risk(&evidence), PackageRisk::Elevated);
    }

    #[test]
    fn native_api_defaults_to_high() {
        let evidence = PackageRiskEvidence {
            native_api_count: 1,
            ..PackageRiskEvidence::default()
        };
        assert_eq!(package_risk(&evidence), PackageRisk::High);
    }
}
