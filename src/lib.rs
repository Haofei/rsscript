mod analyzer;
mod checks;
mod diagnostic;
mod hir;
mod interfaces;
mod lexer;
mod lint;
mod package;
mod review;
mod rust_lower;
pub mod syntax;

pub use analyzer::{
    analyze_source, analyze_source_with_core, analyze_source_with_interfaces, core_interfaces,
};
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Severity, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
};
pub use lint::lint_source;
pub use package::{
    PackageCheck, PackageCheckLock, PackageDependencyKind, PackageDiff, PackageIdentity,
    PackageInterfaceChange, PackageInterfaceChangeKind, PackageLock, PackageLockDiff,
    PackageLockFieldChange, PackageLockMetadata, PackageLockPackage, PackageLockPackageChange,
    PackageManifestChange, PackageMetadataReport, PackageNativeRustCheck, PackageNativeRustReview,
    PackagePublishCheck, PackagePublishDryRun, PackageReview, PackageReviewFile,
    PackageReviewFileKind, PackageReviewMetadata, PackageReviewSummary, PackageRisk, PackageTree,
    PackageTreeNode, PackageTreeSummary, PackageVendorEntry, PackageVendorReport,
    PackageVendorUnresolved, check_package_dir, diff_package_dirs, diff_package_locks,
    format_package_check_human, format_package_check_json, format_package_diff_human,
    format_package_diff_json, format_package_lock_diff_human, format_package_lock_diff_json,
    format_package_lock_json, format_package_lock_toml, format_package_metadata_human,
    format_package_metadata_json, format_package_publish_human, format_package_publish_json,
    format_package_review_human, format_package_review_json, format_package_tree_human,
    format_package_tree_json, format_package_vendor_human, format_package_vendor_json,
    lock_package_dir, package_metadata, package_tree, publish_package_dry_run, review_package_dir,
    vendor_package_dir,
};
pub use review::{
    ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary, ReviewMapClassification,
    ReviewMapFile, ReviewMapFileRisk, ReviewMapRegion, ReviewMapSummary, ReviewRisk,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    review_map_sources, review_sources,
};
pub use rust_lower::{
    GeneratedRustPackage, LoweredRust, RemappedRustcDiagnostic, RustBackendCheckResult,
    RustSourceMapEntry, check_generated_rust_package, lower_program_to_rust,
    lower_program_to_rust_with_map, lower_source_to_rust, lower_source_to_rust_package,
    lower_source_to_rust_with_map, parse_runtime_diagnostics, parse_source_map_json,
    remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines, write_generated_rust_package,
};
