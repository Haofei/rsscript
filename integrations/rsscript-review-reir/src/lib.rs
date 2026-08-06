#![forbid(unsafe_code)]

//! Optional, one-way REIR adapter for neutral compiler/package evidence.

use std::path::Path;

use rsscript::{
    PackageCheck, PackageLock, PackageLockDiff, PackageMetadataReport, PackageReview, PackageTree,
    format_package_check_json, format_package_lock_diff_json, format_package_metadata_json,
    format_package_review_json, format_package_tree_json,
};

pub fn review_bundle_json(review: &PackageReview) -> String {
    let json = format_package_review_json(review);
    let bundle = reir::adapters::rsscript::rsscript_json_to_bundle(None, Some(&json), None)
        .expect("package review JSON should convert to REIR");
    serde_json::to_string(&bundle).expect("REIR review bundle should serialize")
}

pub fn review_diff_json(baseline: &PackageReview, current: &PackageReview) -> String {
    let baseline = reir::adapters::rsscript::rsscript_json_to_bundle(
        None,
        Some(&format_package_review_json(baseline)),
        None,
    )
    .expect("baseline review should convert to REIR");
    let current = reir::adapters::rsscript::rsscript_json_to_bundle(
        None,
        Some(&format_package_review_json(current)),
        None,
    )
    .expect("current review should convert to REIR");
    serde_json::to_string(&reir::compute_diff(&baseline, &current))
        .expect("REIR review diff should serialize")
}

pub fn metadata_bundle_json(metadata: &PackageMetadataReport) -> String {
    let bundle = reir::adapters::rsscript::rsscript_metadata_json_to_bundle(
        &format_package_metadata_json(metadata),
    )
    .expect("package metadata should convert to REIR");
    serde_json::to_string(&bundle).expect("REIR metadata bundle should serialize")
}

pub fn check_bundle_json(check: &PackageCheck) -> String {
    let bundle =
        reir::adapters::rsscript::rsscript_check_json_to_bundle(&format_package_check_json(check))
            .expect("package check should convert to REIR");
    serde_json::to_string(&bundle).expect("REIR check bundle should serialize")
}

pub fn tree_bundle_json(tree: &PackageTree) -> String {
    let bundle =
        reir::adapters::rsscript::rsscript_tree_json_to_bundle(&format_package_tree_json(tree))
            .expect("package tree should convert to REIR");
    serde_json::to_string(&bundle).expect("REIR tree bundle should serialize")
}

pub fn lock_bundle_json(lock: &PackageLock, lockfile_path: Option<&Path>) -> String {
    let mut value = serde_json::to_value(lock).expect("package lock should serialize");
    if let (Some(path), Some(object)) = (lockfile_path, value.as_object_mut()) {
        object.insert(
            "lockfile_path".to_string(),
            path.display().to_string().into(),
        );
    }
    let json = serde_json::to_string(&value).expect("package lock should serialize");
    let bundle = reir::adapters::rsscript::rsscript_lock_json_to_bundle(&json)
        .expect("package lock should convert to REIR");
    serde_json::to_string(&bundle).expect("REIR lock bundle should serialize")
}

pub fn lock_diff_bundle_json(diff: &PackageLockDiff) -> String {
    let bundle = reir::adapters::rsscript::rsscript_lock_diff_json_to_bundle(
        &format_package_lock_diff_json(diff),
    )
    .expect("package lock diff should convert to REIR");
    serde_json::to_string(&bundle).expect("REIR lock diff bundle should serialize")
}
