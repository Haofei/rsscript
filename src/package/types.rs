use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::review::{ReviewFinding, ReviewMap};
use crate::rust_lower::NativeRustDependency;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReview {
    pub package: PackageIdentity,
    pub manifest_path: String,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub features: Vec<String>,
    pub implements: Vec<PackageProviderImplementation>,
    pub summary: PackageReviewSummary,
    pub files: Vec<PackageReviewFile>,
    pub exports: Vec<PackageReviewExport>,
    pub native_rust: Option<PackageNativeRustReview>,
    pub review_map: ReviewMap,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLoweringInput {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub source_path: String,
    pub source_relative_path: String,
    pub source: String,
    pub sources: Vec<(String, String)>,
    pub interfaces: Vec<(String, String)>,
    pub native_dependencies: Vec<NativeRustDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageMetadataReport {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub metadata_path: String,
    pub dry_run: bool,
    pub written: bool,
    pub ok: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub metadata: PackageReviewMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewMetadata {
    pub schema: String,
    pub package: PackageIdentity,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub features: Vec<String>,
    pub implements: Vec<PackageProviderImplementation>,
    pub summary: PackageReviewSummary,
    pub files: Vec<PackageReviewFile>,
    pub exports: Vec<PackageReviewExport>,
    pub native_rust: Option<PackageNativeRustReview>,
    pub review_map: ReviewMap,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageDiff {
    pub old_package: PackageIdentity,
    pub new_package: PackageIdentity,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub manifest_changes: Vec<PackageManifestChange>,
    pub interface_changes: Vec<PackageInterfaceChange>,
    pub old_review: PackageReviewSummary,
    pub new_review: PackageReviewSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageCheck {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub ok: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub implements: Vec<PackageProviderImplementation>,
    pub summary: PackageReviewSummary,
    pub graph: PackageGraphCheck,
    pub lock: PackageCheckLock,
    pub native_rust: Option<PackageNativeRustCheck>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageTree {
    pub root: PackageTreeNode,
    pub summary: PackageTreeSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageGraphCheck {
    pub ok: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackagePublishDryRun {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub ready: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub registry_index: PackageRegistryIndexEntry,
    pub registry_target: Option<PackageRegistryPublishTarget>,
    pub archive_format: String,
    pub archive_hash: String,
    pub archive_files: Vec<PackageArchiveFile>,
    pub review: PackageReviewSummary,
    pub dependency_summary: PackageTreeSummary,
    pub checks: Vec<PackagePublishCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageRegistryPublishTarget {
    pub registry_dir: String,
    pub index_path: String,
    pub archive_manifest_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageRegistryIndexEntry {
    pub schema: String,
    pub name: String,
    pub version: String,
    pub checksum: String,
    pub interface_hash: String,
    pub review_hash: String,
    pub native_hash: Option<String>,
    pub risk: PackageRisk,
    pub native: bool,
    #[serde(rename = "unsafe")]
    pub unsafe_boundary: bool,
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageArchiveFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackagePublishCheck {
    pub name: String,
    pub ok: bool,
    pub risk: PackageRisk,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageVendorReport {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub vendor_dir: String,
    pub dry_run: bool,
    pub ok: bool,
    pub risk: PackageRisk,
    pub entries: Vec<PackageVendorEntry>,
    pub unresolved: Vec<PackageVendorUnresolved>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageVendorEntry {
    pub name: String,
    pub version: String,
    pub source_path: String,
    pub vendor_path: String,
    pub checksum: String,
    pub native: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageVendorUnresolved {
    pub name: String,
    pub requirement: Option<String>,
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PackageTreeSummary {
    pub packages: usize,
    pub path_dependencies: usize,
    pub unresolved_dependencies: usize,
    pub native_packages: usize,
    pub build_execution_packages: usize,
    pub high_risk_packages: usize,
    pub unknown_risk_packages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageTreeNode {
    pub name: String,
    pub version: Option<String>,
    pub requirement: Option<String>,
    pub source: String,
    pub risk: PackageRisk,
    pub features: Vec<String>,
    pub native: bool,
    pub dependency_kind: PackageDependencyKind,
    pub reasons: Vec<String>,
    pub dependencies: Vec<PackageTreeNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageDependencyKind {
    Root,
    Normal,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageCheckLock {
    pub path: String,
    pub present: bool,
    pub matches: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub package_changes: Vec<PackageLockPackageChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustCheck {
    pub path: String,
    pub cargo_toml_present: bool,
    pub cargo_metadata_ok: bool,
    pub cargo_package_name: Option<String>,
    pub target_kinds: Vec<String>,
    pub unsafe_detected: bool,
    pub linked_libraries: Vec<String>,
    pub build_env_detected: bool,
    pub build_download_detected: bool,
    pub file_count: usize,
    pub ok: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    pub version: u32,
    #[serde(rename = "package")]
    pub packages: Vec<PackageLockPackage>,
    pub metadata: PackageLockMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLockPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
    pub interface_hash: String,
    pub review_hash: String,
    pub native_hash: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLockMetadata {
    #[serde(rename = "rss_version")]
    pub rsscript_version: String,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageLockDiff {
    pub old_lock_path: String,
    pub new_lock_path: String,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub old_packages: usize,
    pub new_packages: usize,
    pub package_changes: Vec<PackageLockPackageChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageLockPackageChange {
    pub name: String,
    pub before_version: Option<String>,
    pub after_version: Option<String>,
    pub risk: PackageRisk,
    pub changes: Vec<PackageLockFieldChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageLockFieldChange {
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub risk: PackageRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageManifestChange {
    pub kind: String,
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub risk: PackageRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageInterfaceChange {
    pub file: String,
    pub change: PackageInterfaceChangeKind,
    pub risk: PackageRisk,
    pub findings: Vec<ReviewFinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageInterfaceChangeKind {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageIdentity {
    pub name: String,
    pub version: String,
    pub edition: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageRisk {
    Low,
    Elevated,
    High,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PackageReviewSummary {
    pub interface_files: usize,
    pub source_files: usize,
    pub diagnostics: usize,
    pub errors: usize,
    pub dependencies: usize,
    pub dev_dependencies: usize,
    pub package_features: usize,
    pub public_types: usize,
    pub public_functions: usize,
    pub public_apis: usize,
    pub mutating_apis: usize,
    pub retaining_apis: usize,
    pub resource_apis: usize,
    pub fresh_returning_apis: usize,
    pub native_apis: usize,
    pub parallel_apis: usize,
    pub unsafe_apis: usize,
    pub unknown_apis: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewFile {
    pub path: String,
    pub kind: PackageReviewFileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewExport {
    pub name: String,
    pub kind: String,
    pub classification: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageProviderImplementation {
    pub interface_package: String,
    pub version: Option<String>,
    pub interface_features: Vec<String>,
    pub interface_effective_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageReviewFileKind {
    Interface,
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustReview {
    pub path: String,
    pub crate_name: Option<String>,
    pub build_scripts: Option<String>,
    pub proc_macros: Option<String>,
    pub unsafe_policy: Option<String>,
    pub native_links_policy: Option<String>,
    pub ffi_policy: Option<String>,
    pub links: Vec<String>,
    pub cargo_features: Vec<String>,
    pub semantic: PackageNativeRustSemanticReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustSemanticReview {
    pub author_declaration: PackageNativeRustAuthorDeclaration,
    pub source_scan_best_effort: PackageNativeRustSourceScan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustAuthorDeclaration {
    pub worker_thread_parallelism: bool,
    pub native_parallel_backend: Option<String>,
    pub risk_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustSourceScan {
    pub tool: String,
    pub selected_graph: String,
    pub worker_thread_parallelism_detected: bool,
    pub native_parallel_backends: Vec<String>,
    pub unsafe_detected: bool,
    pub ffi_detected: bool,
    pub filesystem_detected: bool,
    pub network_detected: bool,
    pub build_script_present: bool,
}
