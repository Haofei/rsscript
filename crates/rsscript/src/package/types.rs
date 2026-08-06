use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, Span};
use crate::review::{ReviewFinding, ReviewMap};
use crate::rust_lower::NativeRustDependency;

/// Schema id for the platform-neutral package analysis artifact.
pub const PACKAGE_ANALYSIS_SCHEMA: &str = "rsscript.package_analysis.v1";
/// Schema id for optional review output derived from package analysis and
/// implementation metadata.
pub const PACKAGE_REVIEW_SCHEMA: &str = "rsscript.package_review.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageAnalysisProducer {
    pub name: String,
    pub version: String,
    pub source_revision: String,
    pub ruleset_digest: String,
}

impl PackageAnalysisProducer {
    pub fn current() -> Self {
        Self {
            name: "rsscript".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            source_revision: env!("RSSCRIPT_SOURCE_REVISION").to_string(),
            ruleset_digest: env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT").to_string(),
        }
    }
}

/// Provider- and review-neutral semantic facts for one immutable package
/// snapshot. Host selection, risk classification, native implementation details,
/// and deployment evidence deliberately live outside this artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageAnalysis {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub producer: PackageAnalysisProducer,
    pub package: PackageIdentity,
    pub files: Vec<PackageReviewFile>,
    pub summary: PackageAnalysisSummary,
    pub exports: Vec<PackageAnalysisExport>,
    pub external_imports: Vec<PackageAnalysisExternalImport>,
    pub await_sites: Vec<PackageAnalysisAwaitSite>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PackageAnalysisSummary {
    pub interface_files: usize,
    pub source_files: usize,
    pub public_types: usize,
    pub public_sum_types: usize,
    pub public_type_aliases: usize,
    pub public_consts: usize,
    pub public_functions: usize,
    pub mutating_apis: usize,
    pub retaining_apis: usize,
    pub resource_apis: usize,
    pub fresh_returning_apis: usize,
    pub async_apis: usize,
    pub await_sites: usize,
    pub diagnostics: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageAnalysisExport {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retained_params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageAnalysisExternalImport {
    pub function: String,
    pub symbol: String,
    pub call_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageAnalysisAwaitSite {
    pub function: String,
    pub callee: Option<String>,
    pub live_across_await: Vec<String>,
    pub span: Span,
}

impl From<&PackageReview> for PackageAnalysis {
    fn from(review: &PackageReview) -> Self {
        let summary = &review.summary;
        Self {
            schema: PACKAGE_ANALYSIS_SCHEMA.to_string(),
            producer: PackageAnalysisProducer::current(),
            package: review.package.clone(),
            files: review.files.clone(),
            summary: PackageAnalysisSummary {
                interface_files: summary.interface_files,
                source_files: summary.source_files,
                public_types: summary.public_types,
                public_sum_types: summary.public_sum_types,
                public_type_aliases: summary.public_type_aliases,
                public_consts: summary.public_consts,
                public_functions: summary.public_functions,
                mutating_apis: summary.mutating_apis,
                retaining_apis: summary.retaining_apis,
                resource_apis: summary.resource_apis,
                fresh_returning_apis: summary.fresh_returning_apis,
                async_apis: summary.async_apis,
                await_sites: summary.await_sites,
                diagnostics: summary.diagnostics,
                errors: summary.errors,
            },
            exports: review
                .exports
                .iter()
                .map(|export| PackageAnalysisExport {
                    name: export.name.clone(),
                    kind: export.kind.clone(),
                    function_kind: export.function_kind.clone(),
                    retained_params: export.retained_params.clone(),
                })
                .collect(),
            external_imports: review
                .external_bindings
                .iter()
                .map(|binding| PackageAnalysisExternalImport {
                    function: binding.function.clone(),
                    symbol: binding.binding_symbol.clone(),
                    call_chain: binding.call_chain.clone(),
                    span: binding.span.clone(),
                })
                .collect(),
            await_sites: review
                .await_sites
                .iter()
                .map(|site| PackageAnalysisAwaitSite {
                    function: site.function.clone(),
                    callee: site.callee.clone(),
                    live_across_await: site.live_across_await.clone(),
                    span: site.span.clone(),
                })
                .collect(),
            diagnostics: review.diagnostics.clone(),
        }
    }
}

/// The tool + version that produced an artifact, so consumers can reason about
/// schema compatibility instead of guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactProducer {
    pub name: String,
    pub version: String,
    pub source_revision: String,
    pub build_id: String,
    pub rustc_version: String,
    pub target: String,
    pub enabled_features: Vec<String>,
    pub ruleset_digest: String,
}

impl ArtifactProducer {
    pub fn current() -> Self {
        Self {
            name: "rsscript".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            source_revision: env!("RSSCRIPT_SOURCE_REVISION").to_string(),
            build_id: env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT").to_string(),
            rustc_version: env!("RSSCRIPT_RUSTC_VERSION").to_string(),
            target: env!("RSSCRIPT_BUILD_TARGET").to_string(),
            enabled_features: if cfg!(feature = "native-jit") {
                vec!["native-jit".to_string()]
            } else {
                Vec::new()
            },
            ruleset_digest: env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT").to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReview {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub producer: ArtifactProducer,
    pub package: PackageIdentity,
    pub manifest_path: String,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub virtual_package: Option<PackageVirtual>,
    pub implements: Vec<PackageProviderImplementation>,
    pub dependencies: Vec<PackageReviewDependency>,
    pub summary: PackageReviewSummary,
    pub files: Vec<PackageReviewFile>,
    pub exports: Vec<PackageReviewExport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub external_bindings: Vec<PackageExternalBinding>,
    pub await_sites: Vec<PackageReviewAwaitSite>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativePluginBuildDependency {
    pub crate_name: String,
    pub path: String,
    pub cargo_features: Vec<String>,
    pub default_features: bool,
    pub bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageMetadataReport {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub metadata_path: String,
    pub reir_path: String,
    pub dry_run: bool,
    pub written: bool,
    pub verified: bool,
    pub ok: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub mismatches: Vec<PackageMetadataMismatch>,
    pub metadata: PackageReviewMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageMetadataMismatch {
    pub artifact: String,
    pub path: String,
    pub kind: String,
    pub message: String,
    pub expected_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewMetadata {
    pub schema: String,
    pub package: PackageIdentity,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub virtual_package: Option<PackageVirtual>,
    pub implements: Vec<PackageProviderImplementation>,
    pub dependencies: Vec<PackageReviewDependency>,
    pub summary: PackageReviewSummary,
    pub files: Vec<PackageReviewFile>,
    pub exports: Vec<PackageReviewExport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub external_bindings: Vec<PackageExternalBinding>,
    pub await_sites: Vec<PackageReviewAwaitSite>,
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
    /// Distinct external_bindings added/removed between the two versions — the
    /// "what powers did this change introduce" view.
    pub external_binding_changes: Vec<PackageExternalBindingChange>,
    pub old_review: PackageReviewSummary,
    pub new_review: PackageReviewSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageExternalBindingChangeKind {
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageExternalBindingChange {
    pub change: PackageExternalBindingChangeKind,
    pub category: String,
    pub binding_symbol: String,
    pub risk: crate::package::types::PackageRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageCheck {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub ok: bool,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub virtual_package: Option<PackageVirtual>,
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
pub struct PackageReviewDependency {
    pub name: String,
    pub requirement: Option<String>,
    pub source: String,
    pub features: Vec<String>,
    pub dependency_kind: PackageDependencyKind,
    pub compile_only: bool,
    pub test_only: bool,
    pub platform_provided: bool,
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
    pub virtual_package: Option<PackageVirtual>,
    pub interface_only: bool,
    pub compile_only: bool,
    pub test_only: bool,
    pub platform_provided: bool,
    pub interface_effective_hash: String,
    pub implements: Vec<PackageProviderImplementation>,
    pub dependency_kind: PackageDependencyKind,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub dependencies: Vec<PackageTreeNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageDependencyKind {
    Root,
    Normal,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageVirtual {
    pub has_default: bool,
    pub provider: Option<String>,
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
    pub public_sum_types: usize,
    pub public_type_aliases: usize,
    pub public_consts: usize,
    pub public_functions: usize,
    pub public_apis: usize,
    pub mutating_apis: usize,
    pub retaining_apis: usize,
    pub resource_apis: usize,
    pub fresh_returning_apis: usize,
    pub guarantee_apis: usize,
    pub native_guarantee_apis: usize,
    pub native_apis: usize,
    pub async_apis: usize,
    pub await_sites: usize,
    pub parallel_apis: usize,
    pub unsafe_apis: usize,
    pub unknown_apis: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewFile {
    pub path: String,
    pub kind: PackageReviewFileKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceFile {
    pub path: String,
    pub relative_path: String,
    pub contents: String,
    pub kind: PackageReviewFileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewExport {
    pub name: String,
    pub kind: String,
    pub classification: String,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retained_params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageExternalBinding {
    pub function: String,
    pub binding_symbol: String,
    pub category: String,
    /// Default risk of `category` from the canonical taxonomy (unknown -> high).
    pub risk: crate::package::types::PackageRisk,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub call_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewAwaitSite {
    pub function: String,
    pub callee: Option<String>,
    pub boundary: PackageReviewAwaitBoundary,
    pub live_across_await: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageReviewAwaitBoundary {
    RuntimePending,
    NativePending,
    RssCall,
    Unknown,
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
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustReview {
    pub path: String,
    pub crate_name: Option<String>,
    pub build_scripts: Option<String>,
    pub proc_macros: Option<String>,
    /// Legacy aggregate retained for report compatibility. Enforcement uses
    /// `unsafe_policies`, which preserves all three independent boundaries.
    pub unsafe_policy: Option<String>,
    pub unsafe_policies: PackageNativeRustUnsafePolicies,
    pub native_links_policy: Option<String>,
    pub ffi_policy: Option<String>,
    pub links: Vec<String>,
    pub cargo_features: Vec<String>,
    pub semantic: PackageNativeRustSemanticReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageNativeRustUnsafePolicies {
    pub rss_unsafe_apis: Option<String>,
    pub wrapper_unsafe_blocks: Option<String>,
    pub transitive_unsafe_blocks: Option<String>,
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
    pub complete: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}
