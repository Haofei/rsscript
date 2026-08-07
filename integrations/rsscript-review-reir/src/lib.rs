#![forbid(unsafe_code)]

//! Optional, one-way REIR adapter for neutral compiler analysis.
//!
//! This integration consumes `rsscript.package_analysis.v1`. Provider and
//! deployment metadata may be merged by REIR later; neither REIR nor review
//! policy participates in RSScript compilation.

/// Convert one serialized neutral package analysis artifact into REIR JSON.
pub fn analysis_bundle_json(analysis_json: &str) -> Result<String, serde_json::Error> {
    let bundle = reir::adapters::rsscript::rsscript_analysis_json_to_bundle(analysis_json)?;
    serde_json::to_string(&bundle)
}

/// Compute a REIR diff between two neutral package analysis artifacts.
pub fn analysis_diff_json(
    baseline_json: &str,
    current_json: &str,
) -> Result<String, serde_json::Error> {
    let baseline = reir::adapters::rsscript::rsscript_analysis_json_to_bundle(baseline_json)?;
    let current = reir::adapters::rsscript::rsscript_analysis_json_to_bundle(current_json)?;
    serde_json::to_string(&reir::compute_diff(&baseline, &current))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis(module_digest: &str) -> String {
        format!(
            r#"{{
                "$schema": "rsscript.package_analysis.v1",
                "language_version": "2026",
                "interface_catalog_digest": "sha256:interfaces",
                "snapshot_digest": "sha256:snapshot",
                "module_digest": "{module_digest}",
                "package": {{ "name": "demo", "version": "0.1.0" }},
                "exports": [],
                "external_imports": [],
                "await_sites": [],
                "diagnostics": []
            }}"#
        )
    }

    #[test]
    fn adapter_accepts_only_neutral_package_analysis() {
        let json = analysis_bundle_json(&analysis("sha256:module"))
            .expect("package analysis should convert");
        assert!(json.contains("rsscript_package_analysis"));
        assert!(json.contains("sha256:snapshot"));

        let error = analysis_bundle_json(r#"{"$schema":"rsscript.package_review.v1"}"#)
            .expect_err("review artifacts must not enter the neutral adapter");
        assert!(error.to_string().contains("missing field"));
    }

    #[test]
    fn adapter_diffs_analysis_artifacts() {
        let diff = analysis_diff_json(&analysis("sha256:module-a"), &analysis("sha256:module-b"))
            .expect("analysis diff should serialize");
        assert!(diff.contains("sha256:module"));
    }
}
