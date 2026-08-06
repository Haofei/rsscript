#![allow(unused_imports, dead_code)]
mod common;
pub(crate) use rsscript::{
    analyze_package_dir, check_package_dir, diff_package_dirs, diff_package_locks,
    format_package_analysis_json, format_package_lock_toml, lock_package_dir,
    lower_sources_to_rust_package_with_options, package_lowering_input, package_metadata,
    package_metadata_verify, package_tree, review_package_dir,
};
pub(crate) use rsscript_review_reir::{
    check_bundle_json as format_package_check_reir_json,
    lock_bundle_json as format_package_lock_reir_json,
    lock_diff_bundle_json as format_package_lock_diff_reir_json,
    review_bundle_json as format_package_review_reir_json,
    review_diff_json as format_package_review_reir_diff_json,
    tree_bundle_json as format_package_tree_reir_json,
};
pub(crate) use serde_json::Value;
pub(crate) use std::collections::HashMap;
pub(crate) use std::fs;

fn mock_iam_grant(action: &str) -> reir::Fact {
    reir::Fact {
        schema: "reir.fact.v0.1".to_string(),
        id: format!("fact.mock_iam.grant.{}", action.replace(':', "_")),
        kind: reir::FactKind::Capability,
        role: Some(reir::FactRole::Granted),
        subject: reir::Subject {
            kind: reir::SubjectKind::CloudRole,
            id: "arn:aws:iam::123456789012:role/report-uploader".to_string(),
            name: Some("report-uploader".to_string()),
            package: None,
        },
        capability: Some(reir::Capability {
            category: reir::CapabilityCategory::ObjectStorageWrite,
            provider: Some("aws".to_string()),
            service: Some("s3".to_string()),
            action: Some(action.to_string()),
            resource: Some("arn:aws:s3:::reports-prod/*".to_string()),
            constraints: HashMap::new(),
        }),
        value: reir::FactValue::True,
        confidence: reir::Confidence {
            level: reir::ConfidenceLevel::Authoritative,
            source: Some("mock_iam".to_string()),
        },
        acquisition_mode: reir::AcquisitionMode::CloudPolicy,
        precision: reir::Precision::ResourceScoped,
        evidence: Vec::new(),
        unknown_reason: None,
    }
}

#[path = "checker_package/check.rs"]
mod check;
#[path = "checker_package/dependencies.rs"]
mod dependencies;
