// Serialized compiler and package-manager inputs accepted by the adapter.

/// Input from RSScript review-map (mirrors what the compiler produces).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptReviewMapInput {
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<RsScriptModuleInput>,
    pub regions: Vec<RsScriptRegionInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptModuleInput {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<RsScriptUseInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptUseInput {
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptRegionInput {
    pub file: String,
    pub function_name: String,
    pub classification: RsScriptClassification,
    pub line: usize,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RsScriptClassification {
    Foldable,
    ReviewRequired,
    Unknown,
}

/// Input from RSScript package review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageReviewInput {
    pub package_name: String,
    pub version: String,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<RsScriptProviderImplementation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<RsScriptPackageDependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<RsScriptPackageExport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<RsScriptPackageCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub await_sites: Vec<RsScriptPackageAwaitSite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<RsScriptDiagnosticInput>,
    pub public_apis: usize,
    pub mutating_apis: usize,
    pub retaining_apis: usize,
    pub resource_apis: usize,
    pub native_apis: usize,
    pub unsafe_apis: usize,
    pub unknown_apis: usize,
    pub native_boundaries: Vec<RsScriptNativeBoundary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_cargo_features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_author_declaration: Option<RsScriptNativeAuthorDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_source_scan: Option<RsScriptNativeSourceScan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageCapability {
    pub function: String,
    pub binding_symbol: String,
    pub category: CapabilityCategory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<RsScriptDiagnosticSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageDependency {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default)]
    pub dependency_kind: String,
    #[serde(default)]
    pub compile_only: bool,
    #[serde(default)]
    pub test_only: bool,
    #[serde(default)]
    pub platform_provided: bool,
}

/// Input from RSScript package check output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageCheckInput {
    pub package: RsScriptPackageIdentityInput,
    #[serde(default)]
    pub package_dir: String,
    #[serde(default)]
    pub ok: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub summary: RsScriptPackageCheckSummary,
    pub graph: RsScriptPackageGraphCheckInput,
    pub lock: RsScriptPackageCheckLockInput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<RsScriptProviderImplementation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_rust: Option<RsScriptPackageNativeRustCheckInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<RsScriptDiagnosticInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RsScriptPackageCheckSummary {
    #[serde(default)]
    pub diagnostics: usize,
    #[serde(default)]
    pub errors: usize,
    #[serde(default)]
    pub dependencies: usize,
    #[serde(default)]
    pub native_apis: usize,
    #[serde(default)]
    pub unsafe_apis: usize,
    #[serde(default)]
    pub unknown_apis: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageGraphCheckInput {
    #[serde(default)]
    pub ok: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageCheckLockInput {
    pub path: String,
    #[serde(default)]
    pub present: bool,
    #[serde(default)]
    pub matches: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_changes: Vec<RsScriptPackageLockPackageChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageNativeRustCheckInput {
    pub path: String,
    #[serde(default)]
    pub cargo_toml_present: bool,
    #[serde(default)]
    pub cargo_metadata_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_kinds: Vec<String>,
    #[serde(default)]
    pub unsafe_detected: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_libraries: Vec<String>,
    #[serde(default)]
    pub build_env_detected: bool,
    #[serde(default)]
    pub build_download_detected: bool,
    #[serde(default)]
    pub file_count: usize,
    #[serde(default)]
    pub ok: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// Input from RSScript semantic package lockfiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageLockInput {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile_path: Option<String>,
    #[serde(rename = "package")]
    pub packages: Vec<RsScriptPackageLockPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageLockPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
    pub interface_hash: String,
    pub review_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// Input from RSScript semantic lockfile update review output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageLockDiffInput {
    pub old_lock_path: String,
    pub new_lock_path: String,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub old_packages: usize,
    #[serde(default)]
    pub new_packages: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_changes: Vec<RsScriptPackageLockPackageChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageLockPackageChange {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_version: Option<String>,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<RsScriptPackageLockFieldChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageLockFieldChange {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub risk: RsScriptPackageRisk,
}

/// Input from RSScript package dependency tree output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageTreeInput {
    pub root: RsScriptPackageTreeNode,
    #[serde(default)]
    pub summary: RsScriptPackageTreeSummary,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RsScriptPackageTreeSummary {
    #[serde(default)]
    pub packages: usize,
    #[serde(default)]
    pub path_dependencies: usize,
    #[serde(default)]
    pub unresolved_dependencies: usize,
    #[serde(default)]
    pub native_packages: usize,
    #[serde(default)]
    pub build_execution_packages: usize,
    #[serde(default)]
    pub high_risk_packages: usize,
    #[serde(default)]
    pub unknown_risk_packages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageTreeNode {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement: Option<String>,
    #[serde(default)]
    pub source: String,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default)]
    pub native: bool,
    #[serde(default)]
    pub interface_only: bool,
    #[serde(default)]
    pub compile_only: bool,
    #[serde(default)]
    pub test_only: bool,
    #[serde(default)]
    pub platform_provided: bool,
    #[serde(default)]
    pub interface_effective_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<RsScriptProviderImplementation>,
    #[serde(default)]
    pub dependency_kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<RsScriptPackageTreeNode>,
}

/// Input from RSScript package metadata write/verify output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageMetadataInput {
    pub package: RsScriptPackageIdentityInput,
    #[serde(default)]
    pub package_dir: String,
    pub metadata_path: String,
    #[serde(default)]
    pub reir_path: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub written: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub ok: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mismatches: Vec<RsScriptPackageMetadataMismatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageMetadataMismatch {
    #[serde(default)]
    pub artifact: String,
    pub path: String,
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub expected_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageIdentityInput {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptProviderImplementation {
    pub interface_package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interface_features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_effective_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageExport {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub classification: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub normalized_effects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageAwaitSite {
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee: Option<String>,
    #[serde(default)]
    pub boundary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_across_await: Vec<String>,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub column: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptDiagnosticInput {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<RsScriptDiagnosticSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptDiagnosticSpan {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub column: usize,
    #[serde(default)]
    pub length: usize,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RsScriptPackageRisk {
    Low,
    Elevated,
    High,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptNativeBoundary {
    pub module_name: String,
    pub functions: Vec<String>,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptNativeAuthorDeclaration {
    #[serde(default)]
    pub worker_thread_parallelism: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_parallel_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptNativeSourceScan {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub selected_graph: String,
    #[serde(default)]
    pub worker_thread_parallelism_detected: bool,
    #[serde(default)]
    pub unsafe_detected: bool,
    #[serde(default)]
    pub ffi_detected: bool,
    #[serde(default)]
    pub filesystem_detected: bool,
    #[serde(default)]
    pub network_detected: bool,
    #[serde(default)]
    pub build_script_present: bool,
}
