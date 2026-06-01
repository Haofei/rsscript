//! RSScript language and package producer adapter for REIR.
//! Converts RSScript compiler/package review output into REIR facts.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::*;

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const EDGE_SCHEMA: &str = "reir.edge.v0.1";
const PRODUCER_VERSION: &str = "0.5.0";
const ADAPTER_VERSION: &str = "0.1";
const PRODUCER_SOURCE: &str = "compiler_contract";
const REVIEW_MAP_SOURCE: &str = "rsscript_review_map";
const PACKAGE_REVIEW_SOURCE: &str = "rsscript_package_review";
const PACKAGE_CHECK_SOURCE: &str = "rsscript_package_check";
const LOCKFILE_SOURCE: &str = "rsscript_lockfile";
const PACKAGE_METADATA_SOURCE: &str = "rsscript_package_metadata";
const PUBLISH_SOURCE: &str = "rsscript_publish_dry_run";
const TREE_SOURCE: &str = "rsscript_tree";
const VENDOR_SOURCE: &str = "rsscript_vendor";
const REVIEW_REQUIRED_KIND: &str = "review_required";

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

/// Input from RSScript package publish dry-run output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackagePublishInput {
    pub package: RsScriptPackageIdentityInput,
    #[serde(default)]
    pub ready: bool,
    pub risk: RsScriptPackageRisk,
    pub registry_index: RsScriptRegistryIndexInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_target: Option<RsScriptRegistryTargetInput>,
    #[serde(default)]
    pub archive_format: String,
    pub archive_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<RsScriptPublishCheckInput>,
}

/// Input from RSScript package metadata write/verify output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageMetadataInput {
    pub package: RsScriptPackageIdentityInput,
    #[serde(default)]
    pub package_dir: String,
    pub metadata_path: String,
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

/// Input from RSScript package vendor output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageVendorInput {
    pub package: RsScriptPackageIdentityInput,
    #[serde(default)]
    pub package_dir: String,
    pub vendor_dir: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub ok: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<RsScriptPackageVendorEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<RsScriptPackageVendorUnresolved>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageVendorEntry {
    pub name: String,
    pub version: String,
    pub source_path: String,
    pub vendor_path: String,
    pub checksum: String,
    #[serde(default)]
    pub native: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageVendorUnresolved {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement: Option<String>,
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageIdentityInput {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptRegistryIndexInput {
    pub schema: String,
    pub name: String,
    pub version: String,
    pub checksum: String,
    pub interface_hash: String,
    #[serde(default)]
    pub effective_interface_hash_default: String,
    pub review_hash: String,
    #[serde(default)]
    pub review_schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_hash: Option<String>,
    pub risk: RsScriptPackageRisk,
    #[serde(default)]
    pub native: bool,
    #[serde(default, rename = "unsafe", alias = "unsafe_apis")]
    pub unsafe_boundary: bool,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub footprint_default: RsScriptRegistryFootprintInput,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RsScriptRegistryFootprintInput {
    #[serde(default)]
    pub direct_dependencies: usize,
    #[serde(default)]
    pub total_packages: usize,
    #[serde(default)]
    pub path_dependencies: usize,
    #[serde(default)]
    pub unresolved_dependencies: usize,
    #[serde(default)]
    pub native: bool,
    #[serde(default)]
    pub native_packages: usize,
    #[serde(default)]
    pub build_time_execution: bool,
    #[serde(default)]
    pub build_execution_packages: usize,
    #[serde(default)]
    pub high_risk_packages: usize,
    #[serde(default)]
    pub unknown_facts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptRegistryTargetInput {
    pub registry_dir: String,
    pub index_path: String,
    pub archive_manifest_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPublishCheckInput {
    pub name: String,
    #[serde(default)]
    pub ok: bool,
    pub risk: RsScriptPackageRisk,
    #[serde(default)]
    pub detail: String,
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

/// Convert RSScript review-map output into REIR facts.
pub fn review_map_to_facts(input: &RsScriptReviewMapInput) -> Vec<Fact> {
    let mut facts = module_use_facts(input);
    facts.extend(
        input
            .regions
            .iter()
            .filter_map(|region| match region.classification {
                RsScriptClassification::Foldable => None,
                RsScriptClassification::Unknown => Some(Fact {
                    schema: FACT_SCHEMA.to_owned(),
                    id: format!(
                        "fact.review_map.{}.unknown",
                        normalized_id(&function_subject_id(
                            &input.package_name,
                            &region.function_name
                        ))
                    ),
                    kind: FactKind::Unknown,
                    role: None,
                    subject: function_subject(&input.package_name, &region.function_name),
                    capability: None,
                    value: FactValue::Unknown,
                    confidence: confidence(ConfidenceLevel::Unknown, REVIEW_MAP_SOURCE),
                    acquisition_mode: AcquisitionMode::CompilerContract,
                    precision: Precision::Exact,
                    evidence: vec![source_span(
                        &region.file,
                        region.line,
                        &region.function_name,
                        joined_reason(&region.reasons),
                        REVIEW_MAP_SOURCE,
                    )],
                    unknown_reason: joined_reason(&region.reasons),
                }),
                RsScriptClassification::ReviewRequired => {
                    let kind = classify_review_required(&region.reasons);
                    let kind_name: String = kind.clone().into();
                    Some(Fact {
                        schema: FACT_SCHEMA.to_owned(),
                        id: format!(
                            "fact.review_map.{}.{}",
                            normalized_id(&function_subject_id(
                                &input.package_name,
                                &region.function_name
                            )),
                            normalized_id(&kind_name)
                        ),
                        kind,
                        role: None,
                        subject: function_subject(&input.package_name, &region.function_name),
                        capability: None,
                        value: FactValue::True,
                        confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
                        acquisition_mode: AcquisitionMode::CompilerContract,
                        precision: Precision::Exact,
                        evidence: vec![source_span(
                            &region.file,
                            region.line,
                            &region.function_name,
                            joined_reason(&region.reasons),
                            REVIEW_MAP_SOURCE,
                        )],
                        unknown_reason: None,
                    })
                }
            }),
    );
    facts
}

/// Convert RSScript package review into REIR facts.
pub fn package_review_to_facts(input: &RsScriptPackageReviewInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    let package_subject = package_subject(&input.package_name, &input.version);
    let package_slug = normalized_id(&package_subject.id);
    let package_summary = package_review_summary(input);

    facts.push(Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package.{}.risk", package_slug),
        kind: FactKind::PackageRisk,
        role: None,
        subject: package_subject.clone(),
        capability: None,
        value: package_risk_value(&input.risk),
        confidence: confidence(package_risk_confidence(&input.risk), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Category,
        evidence: vec![package_metadata(
            &input.package_name,
            &input.version,
            Some(input.risk.as_str().to_owned()),
            Some(package_summary.clone()),
        )],
        unknown_reason: matches!(input.risk, RsScriptPackageRisk::Unknown)
            .then(|| "package risk could not be determined".to_owned()),
    });

    if input.native_apis > 0 {
        facts.push(capability_fact(
            format!("fact.package.{}.capability.runtime_native", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeNative,
            input.native_apis,
            &input.package_name,
            &input.version,
            package_summary.clone(),
        ));
    }

    if input.unsafe_apis > 0 {
        facts.push(capability_fact(
            format!("fact.package.{}.capability.runtime_unsafe", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeUnsafe,
            input.unsafe_apis,
            &input.package_name,
            &input.version,
            package_summary.clone(),
        ));
    }
    if let Some(scan) = &input.native_source_scan {
        facts.extend(native_source_scan_facts(
            input,
            scan,
            &package_subject,
            &package_slug,
        ));
    }
    facts.extend(native_author_declaration_facts(
        input,
        &package_subject,
        &package_slug,
    ));
    facts.extend(native_cargo_feature_facts(input, &package_slug));
    facts.extend(await_site_facts(input, &package_slug));
    facts.extend(diagnostic_facts(input, &package_subject, &package_slug));
    facts.extend(package_feature_facts(input, &package_slug));
    facts.extend(provider_implementation_facts(input, &package_slug));
    facts.extend(package_capability_facts(input, &package_slug));

    for dependency in &input.dependencies {
        let dependency_subject = dependency_subject(dependency);
        let dependency_slug = normalized_id(&dependency_subject.id);
        let value = dependency_risk_value(dependency);
        let unknown = matches!(value, FactValue::Unknown);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.dependency.{}.risk", dependency_slug),
            kind: FactKind::DependencyRisk,
            role: None,
            subject: dependency_subject,
            capability: None,
            value,
            confidence: confidence(dependency_confidence(dependency), PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                dependency.requirement.clone(),
                Some(dependency_summary(dependency)),
            )],
            unknown_reason: unknown
                .then(|| "dependency source is unresolved by package review".to_owned()),
        });
    }

    for export in &input.exports {
        let export_subject = public_contract_subject(&input.package_name, export);
        let export_value = public_contract_value(export);
        let export_unknown = matches!(export_value, FactValue::Unknown);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.public_contract.{}.{}",
                package_slug,
                normalized_id(&export_subject.id)
            ),
            kind: FactKind::PublicContract,
            role: None,
            subject: export_subject.clone(),
            capability: None,
            value: export_value,
            confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some(export.kind.clone()),
                Some(package_export_summary(export)),
            )],
            unknown_reason: export_unknown.then(|| public_contract_unknown_reason(export)),
        });
        if let Some(protocol_fact) = protocol_impl_fact(
            export,
            &export_subject,
            &input.package_name,
            &input.version,
            &package_slug,
        ) {
            facts.push(protocol_fact);
        }
        if let Some(protocol_fact) = protocol_declaration_fact(
            export,
            &export_subject,
            &input.package_name,
            &input.version,
            &package_slug,
        ) {
            facts.push(protocol_fact);
        }
        for category in standard_library_export_capabilities(export) {
            facts.push(export_capability_fact(
                format!(
                    "fact.public_contract.{}.{}.capability.{}",
                    package_slug,
                    normalized_id(&export_subject.id),
                    normalized_id(&String::from(category.clone()))
                ),
                export_subject.clone(),
                export,
                category,
                &input.package_name,
                &input.version,
            ));
        }
    }
    facts.extend(protocol_method_contract_facts(
        input,
        &package_slug,
        &input
            .exports
            .iter()
            .filter(|export| export.kind == "protocol")
            .map(|export| export.name.clone())
            .collect::<Vec<_>>(),
    ));

    for boundary in &input.native_boundaries {
        let boundary_subject = native_boundary_subject(&input.package_name, &boundary.module_name);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.native_module_declaration.{}",
                normalized_id(&boundary_subject.id)
            ),
            kind: FactKind::NativeModuleDeclaration,
            role: None,
            subject: boundary_subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![source_span(
                &boundary.file,
                boundary.line,
                &boundary.module_name,
                Some(native_module_declaration_reason(boundary)),
                PACKAGE_REVIEW_SOURCE,
            )],
            unknown_reason: None,
        });
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.native_boundary.{}",
                normalized_id(&boundary_subject.id)
            ),
            kind: FactKind::NativeBoundary,
            role: None,
            subject: boundary_subject,
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![source_span(
                &boundary.file,
                boundary.line,
                &boundary.module_name,
                Some(native_boundary_reason(boundary)),
                PACKAGE_REVIEW_SOURCE,
            )],
            unknown_reason: None,
        });
    }

    facts
}

/// Convert RSScript semantic lockfile entries into supply-chain REIR facts.
pub fn package_lock_to_facts(input: &RsScriptPackageLockInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    let lockfile_path = input.lockfile_path.as_deref().unwrap_or("rsspkg.lock");
    for (index, package) in input.packages.iter().enumerate() {
        let subject = package_subject(&package.name, &package.version);
        let package_slug = normalized_id(&subject.id);
        facts.push(lockfile_supply_chain_fact(
            &package_slug,
            "checksum",
            "checksum",
            &subject,
            package,
            index,
            &package.checksum,
            lockfile_path,
            format!(
                "package checksum for {}@{} source={}",
                package.name, package.version, package.source
            ),
        ));
        facts.push(lockfile_supply_chain_fact(
            &package_slug,
            "effective_interface_hash",
            "interface_hash",
            &subject,
            package,
            index,
            &package.interface_hash,
            lockfile_path,
            format!(
                "effective_interface_hash={} features={}",
                package.interface_hash,
                lockfile_features_summary(&package.features)
            ),
        ));
        facts.push(lockfile_supply_chain_fact(
            &package_slug,
            "review_hash",
            "review_hash",
            &subject,
            package,
            index,
            &package.review_hash,
            lockfile_path,
            "review metadata hash".to_owned(),
        ));
        if let Some(native_hash) = &package.native_hash {
            facts.push(lockfile_supply_chain_fact(
                &package_slug,
                "native_hash",
                "native_hash",
                &subject,
                package,
                index,
                native_hash,
                lockfile_path,
                "native wrapper source hash".to_owned(),
            ));
        }
    }
    facts
}

/// Convert RSScript package check output into REIR policy facts.
pub fn package_check_to_facts(input: &RsScriptPackageCheckInput) -> Vec<Fact> {
    let subject = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.status", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if input.ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_check_evidence(
            input,
            None,
            Some(package_check_summary(input)),
            Some("/ok"),
        )],
        unknown_reason: (!input.ok).then(|| "package check failed".to_owned()),
    }];

    facts.push(package_check_policy_fact(
        &package_slug,
        "graph",
        &subject,
        input,
        input.graph.ok,
        &input.graph.risk,
        check_reasons_summary("graph", &input.graph.reasons),
        "/graph",
    ));
    facts.push(package_check_lock_policy_fact(
        &package_slug,
        &subject,
        input,
        input.lock.matches,
        &input.lock.risk,
        check_reasons_summary("lock", &input.lock.reasons),
    ));

    for (index, change) in input.lock.package_changes.iter().enumerate() {
        let package_subject = lock_diff_package_subject(change);
        let change_slug = normalized_id(&change.name);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.package_check.{}.lock_change.{}",
                package_slug, change_slug
            ),
            kind: FactKind::DependencyRisk,
            role: None,
            subject: package_subject,
            capability: None,
            value: package_risk_value(&change.risk),
            confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
            acquisition_mode: AcquisitionMode::Lockfile,
            precision: Precision::Category,
            evidence: vec![package_check_lock_evidence(
                input,
                Some(change.risk.as_str().to_owned()),
                Some(lock_diff_package_summary(change)),
                &format!("/lock/package_changes/{index}"),
            )],
            unknown_reason: matches!(change.risk, RsScriptPackageRisk::Unknown)
                .then(|| format!("lock change for package `{}` is unknown", change.name)),
        });
        for (field_index, field) in change.changes.iter().enumerate() {
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package_check.{}.lock_change.{}.field.{}",
                    package_slug,
                    change_slug,
                    normalized_id(&field.field)
                ),
                kind: lock_diff_field_fact_kind(&field.field),
                role: None,
                subject: lock_diff_package_subject(change),
                capability: None,
                value: package_risk_value(&field.risk),
                confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
                acquisition_mode: AcquisitionMode::Lockfile,
                precision: Precision::Exact,
                evidence: vec![package_check_lock_evidence(
                    input,
                    field.after.clone().or_else(|| field.before.clone()),
                    Some(lock_diff_field_summary(field)),
                    &format!("/lock/package_changes/{index}/changes/{field_index}"),
                )],
                unknown_reason: matches!(field.risk, RsScriptPackageRisk::Unknown).then(|| {
                    format!(
                        "stale lock field `{}` for package `{}` is unknown",
                        field.field, change.name
                    )
                }),
            });
        }
    }

    facts.extend(package_check_provider_implementation_facts(
        input,
        &package_slug,
    ));

    if let Some(native) = &input.native_rust {
        facts.push(package_check_policy_fact(
            &package_slug,
            "native",
            &subject,
            input,
            native.ok,
            &native.risk,
            package_check_native_summary(native),
            "/native_rust",
        ));
        if native.unsafe_detected {
            facts.push(package_check_boundary_fact(
                &package_slug,
                "unsafe",
                &subject,
                input,
                FactKind::UnsafeBoundary,
                "native Rust unsafe detected",
                "/native_rust/unsafe_detected",
            ));
        }
        if native.build_env_detected || native.build_download_detected {
            facts.push(package_check_boundary_fact(
                &package_slug,
                "build_time",
                &subject,
                input,
                FactKind::BuildTimeExecution,
                "native Rust build script risk detected",
                "/native_rust",
            ));
        }
    }

    for (index, diagnostic) in input.diagnostics.iter().enumerate() {
        let span = diagnostic.spans.first();
        let diagnostic_subject = span
            .filter(|span| !span.file.is_empty())
            .map(|span| Subject {
                kind: SubjectKind::CodeFile,
                id: format!("{}::{}", input.package.name, span.file),
                name: Some(span.file.clone()),
                package: Some(input.package.name.clone()),
            })
            .unwrap_or_else(|| subject.clone());
        let diagnostic_unknown = matches!(diagnostic_fact_value(diagnostic), FactValue::Unknown);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.package_check.{}.diagnostic.{}.{}",
                package_slug,
                index,
                normalized_id(&diagnostic.code)
            ),
            kind: FactKind::Diagnostic,
            role: None,
            subject: diagnostic_subject,
            capability: None,
            value: if diagnostic_unknown {
                FactValue::Unknown
            } else {
                FactValue::True
            },
            confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_CHECK_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![package_check_diagnostic_evidence(input, diagnostic, span)],
            unknown_reason: diagnostic_unknown.then(|| diagnostic.summary.clone()),
        });
    }

    facts
}

/// Convert RSScript semantic lockfile update reviews into REIR facts.
pub fn package_lock_diff_to_facts(input: &RsScriptPackageLockDiffInput) -> Vec<Fact> {
    let subject = lock_diff_subject(input);
    let diff_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.lock_update.{}.risk", diff_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: package_risk_value(&input.risk),
        confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
        acquisition_mode: AcquisitionMode::Lockfile,
        precision: Precision::Category,
        evidence: vec![lock_diff_evidence(
            input,
            input.new_lock_path.clone(),
            Some(input.risk.as_str().to_owned()),
            Some(lock_diff_summary(input)),
            "/risk",
        )],
        unknown_reason: matches!(input.risk, RsScriptPackageRisk::Unknown)
            .then(|| "lock update risk is unknown".to_owned()),
    }];

    for (package_index, package) in input.package_changes.iter().enumerate() {
        let package_subject = lock_diff_package_subject(package);
        let package_slug = normalized_id(&format!("{}::{}", subject.id, package.name));
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.lock_update.{}.package.{}.risk",
                diff_slug, package_slug
            ),
            kind: FactKind::DependencyRisk,
            role: None,
            subject: package_subject.clone(),
            capability: None,
            value: package_risk_value(&package.risk),
            confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
            acquisition_mode: AcquisitionMode::Lockfile,
            precision: Precision::Category,
            evidence: vec![lock_diff_evidence(
                input,
                lock_diff_package_evidence_file(input, package),
                Some(package.risk.as_str().to_owned()),
                Some(lock_diff_package_summary(package)),
                &format!("/package_changes/{package_index}"),
            )],
            unknown_reason: matches!(package.risk, RsScriptPackageRisk::Unknown)
                .then(|| format!("lock update package `{}` risk is unknown", package.name)),
        });
        for (field_index, field) in package.changes.iter().enumerate() {
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.lock_update.{}.package.{}.field.{}",
                    diff_slug,
                    package_slug,
                    normalized_id(&field.field)
                ),
                kind: lock_diff_field_fact_kind(&field.field),
                role: None,
                subject: package_subject.clone(),
                capability: None,
                value: package_risk_value(&field.risk),
                confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
                acquisition_mode: AcquisitionMode::Lockfile,
                precision: Precision::Exact,
                evidence: vec![lock_diff_evidence(
                    input,
                    lock_diff_field_evidence_file(input, field),
                    field.after.clone().or_else(|| field.before.clone()),
                    Some(lock_diff_field_summary(field)),
                    &format!("/package_changes/{package_index}/changes/{field_index}"),
                )],
                unknown_reason: matches!(field.risk, RsScriptPackageRisk::Unknown).then(|| {
                    format!(
                        "lock update field `{}` for package `{}` is unknown",
                        field.field, package.name
                    )
                }),
            });
        }
    }
    facts
}

/// Convert RSScript dependency tree nodes into graph-risk REIR facts.
pub fn package_tree_to_facts(input: &RsScriptPackageTreeInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    collect_package_tree_facts(&input.root, "root", &mut facts);
    facts
}

/// Convert RSScript dependency tree relationships into REIR dependency edges.
pub fn package_tree_to_edges(input: &RsScriptPackageTreeInput) -> Vec<Edge> {
    let mut edges = Vec::new();
    collect_package_tree_edges(&input.root, "root", &mut edges);
    edges
}

/// Convert RSScript publish dry-run output into registry/archive REIR facts.
pub fn package_publish_to_facts(input: &RsScriptPackagePublishInput) -> Vec<Fact> {
    let subject = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.publish.{}.readiness", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if input.ready {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PUBLISH_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![registry_metadata_evidence(
            input,
            Some(input.ready.to_string()),
            Some(package_publish_summary(input)),
            "/ready",
        )],
        unknown_reason: (!input.ready).then(|| "publish dry-run is not ready".to_owned()),
    }];

    facts.push(publish_supply_chain_fact(
        &package_slug,
        "archive_checksum",
        &subject,
        input,
        &input.archive_hash,
        "package archive checksum",
        "/archive_hash",
    ));
    facts.push(publish_supply_chain_fact(
        &package_slug,
        "registry_checksum",
        &subject,
        input,
        &input.registry_index.checksum,
        "registry index checksum",
        "/registry_index/checksum",
    ));
    let (interface_hash, interface_hash_pointer) = if input
        .registry_index
        .effective_interface_hash_default
        .is_empty()
    {
        (
            input.registry_index.interface_hash.as_str(),
            "/registry_index/interface_hash",
        )
    } else {
        (
            input
                .registry_index
                .effective_interface_hash_default
                .as_str(),
            "/registry_index/effective_interface_hash_default",
        )
    };
    facts.push(publish_supply_chain_fact(
        &package_slug,
        "effective_interface_hash",
        &subject,
        input,
        interface_hash,
        "registry index effective interface hash",
        interface_hash_pointer,
    ));
    facts.push(publish_supply_chain_fact(
        &package_slug,
        "review_hash",
        &subject,
        input,
        &input.registry_index.review_hash,
        "registry index review metadata hash",
        "/registry_index/review_hash",
    ));
    if let Some(native_hash) = &input.registry_index.native_hash {
        facts.push(publish_supply_chain_fact(
            &package_slug,
            "native_hash",
            &subject,
            input,
            native_hash,
            "registry index native wrapper hash",
            "/registry_index/native_hash",
        ));
    }
    if !input.registry_index.review_schema.is_empty() {
        facts.push(publish_supply_chain_fact(
            &package_slug,
            "review_schema",
            &subject,
            input,
            &input.registry_index.review_schema,
            "registry index package review metadata schema",
            "/registry_index/review_schema",
        ));
    }
    if let Some(default_features) = input.registry_index.features.get("default") {
        facts.push(publish_supply_chain_fact(
            &package_slug,
            "default_features",
            &subject,
            input,
            &registry_feature_summary(default_features),
            "registry index default selected package features",
            "/registry_index/features/default",
        ));
    }
    facts.push(publish_registry_boundary_fact(
        &package_slug,
        "registry_native",
        FactKind::NativeBoundary,
        &subject,
        input,
        input.registry_index.native,
        "registry index native boundary signal",
        "/registry_index/native",
    ));
    facts.push(publish_registry_boundary_fact(
        &package_slug,
        "registry_unsafe_apis",
        FactKind::UnsafeBoundary,
        &subject,
        input,
        input.registry_index.unsafe_boundary,
        "registry index unsafe API signal",
        "/registry_index/unsafe_apis",
    ));
    if registry_footprint_present(&input.registry_index.footprint_default) {
        facts.push(registry_footprint_fact(
            &package_slug,
            &subject,
            input,
            &input.registry_index.footprint_default,
        ));
    }
    for (index, check) in input.checks.iter().enumerate() {
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.publish.{}.check.{}",
                package_slug,
                normalized_id(&check.name)
            ),
            kind: FactKind::PolicyResult,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: if check.ok {
                FactValue::True
            } else {
                FactValue::Unknown
            },
            confidence: confidence(ConfidenceLevel::Computed, PUBLISH_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![registry_metadata_evidence(
                input,
                Some(check.risk.as_str().to_owned()),
                Some(format!(
                    "publish check {} ok={} risk={} detail={}",
                    check.name,
                    check.ok,
                    check.risk.as_str(),
                    check.detail
                )),
                &format!("/checks/{index}"),
            )],
            unknown_reason: (!check.ok).then(|| format!("publish check `{}` failed", check.name)),
        });
    }
    facts
}

/// Convert RSScript package metadata write/verify output into REIR facts.
pub fn package_metadata_report_to_facts(input: &RsScriptPackageMetadataInput) -> Vec<Fact> {
    let subject = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.metadata.{}.status", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if input.ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_METADATA_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![metadata_artifact_evidence(
            input,
            Some(input.package_dir.clone()),
            None,
            Some(package_metadata_report_summary(input)),
            Some("/ok"),
        )],
        unknown_reason: (!input.ok)
            .then(|| "package metadata write/verify result is not ok".to_owned()),
    }];

    facts.push(metadata_artifact_fact(
        &package_slug,
        "package_review_artifact",
        &subject,
        input,
        &input.metadata_path,
        "package review metadata artifact",
        "/metadata_path",
    ));
    facts.push(metadata_artifact_fact(
        &package_slug,
        "reir_artifact",
        &subject,
        input,
        &input.reir_path,
        "package REIR bundle artifact",
        "/reir_path",
    ));

    for (index, mismatch) in input.mismatches.iter().enumerate() {
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.metadata.{}.mismatch.{}.{}",
                package_slug,
                index,
                normalized_id(&mismatch.path)
            ),
            kind: FactKind::PolicyResult,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::Unknown,
            confidence: confidence(ConfidenceLevel::Computed, PACKAGE_METADATA_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: vec![metadata_artifact_evidence(
                input,
                Some(mismatch.path.clone()),
                Some(metadata_mismatch_value(mismatch)),
                Some(metadata_mismatch_reason(mismatch)),
                Some(&format!("/mismatches/{index}")),
            )],
            unknown_reason: Some(format!(
                "metadata artifact `{}` is {}{}",
                mismatch.path,
                mismatch.kind,
                metadata_mismatch_hash_suffix(mismatch)
            )),
        });
    }

    facts
}

fn metadata_mismatch_value(mismatch: &RsScriptPackageMetadataMismatch) -> String {
    if mismatch.expected_sha256.is_empty() {
        mismatch.path.clone()
    } else if let Some(actual_sha256) = &mismatch.actual_sha256 {
        format!(
            "{} expected={} actual={}",
            mismatch.path, mismatch.expected_sha256, actual_sha256
        )
    } else {
        format!("{} expected={}", mismatch.path, mismatch.expected_sha256)
    }
}

fn metadata_mismatch_reason(mismatch: &RsScriptPackageMetadataMismatch) -> String {
    let artifact = if mismatch.artifact.is_empty() {
        "artifact"
    } else {
        mismatch.artifact.as_str()
    };
    format!(
        "metadata {artifact} {}: {}{}",
        mismatch.kind,
        mismatch.message,
        metadata_mismatch_hash_suffix(mismatch)
    )
}

fn metadata_mismatch_hash_suffix(mismatch: &RsScriptPackageMetadataMismatch) -> String {
    if mismatch.expected_sha256.is_empty() {
        String::new()
    } else if let Some(actual_sha256) = &mismatch.actual_sha256 {
        format!(
            " (expected {}, actual {})",
            mismatch.expected_sha256, actual_sha256
        )
    } else {
        format!(" (expected {})", mismatch.expected_sha256)
    }
}

/// Convert RSScript package vendor output into REIR facts.
pub fn package_vendor_to_facts(input: &RsScriptPackageVendorInput) -> Vec<Fact> {
    let subject = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.vendor.{}.status", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if input.ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, VENDOR_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![vendor_evidence(
            input,
            input.vendor_dir.clone(),
            None,
            Some(package_vendor_summary(input)),
            Some("/ok"),
        )],
        unknown_reason: (!input.ok)
            .then(|| "vendor operation has unresolved dependencies".to_owned()),
    }];

    for (index, entry) in input.entries.iter().enumerate() {
        let entry_subject = package_subject(&entry.name, &entry.version);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.vendor.{}.entry.{}.checksum",
                package_slug,
                normalized_id(&entry_subject.id)
            ),
            kind: FactKind::SupplyChain,
            role: None,
            subject: entry_subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Computed, VENDOR_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: vec![vendor_evidence(
                input,
                entry.vendor_path.clone(),
                Some(entry.checksum.clone()),
                Some(format!(
                    "vendored {}@{} source={} vendor_path={} native={}",
                    entry.name, entry.version, entry.source_path, entry.vendor_path, entry.native
                )),
                Some(&format!("/entries/{index}")),
            )],
            unknown_reason: None,
        });
    }

    for (index, unresolved) in input.unresolved.iter().enumerate() {
        let dependency = RsScriptPackageDependency {
            name: unresolved.name.clone(),
            requirement: unresolved.requirement.clone(),
            source: unresolved.source.clone(),
            features: Vec::new(),
            dependency_kind: "normal".to_owned(),
            compile_only: false,
            test_only: false,
            platform_provided: false,
        };
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.vendor.{}.unresolved.{}",
                package_slug,
                normalized_id(&unresolved.name)
            ),
            kind: FactKind::DependencyRisk,
            role: None,
            subject: dependency_subject(&dependency),
            capability: None,
            value: FactValue::Unknown,
            confidence: confidence(ConfidenceLevel::Unknown, VENDOR_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![vendor_evidence(
                input,
                input.vendor_dir.clone(),
                unresolved
                    .requirement
                    .clone()
                    .or_else(|| Some(unresolved.source.clone())),
                Some(format!(
                    "unresolved vendor dependency {} source={} reason={}",
                    unresolved.name, unresolved.source, unresolved.reason
                )),
                Some(&format!("/unresolved/{index}")),
            )],
            unknown_reason: Some(unresolved.reason.clone()),
        });
    }

    facts
}

/// Convert RSScript package review relationships into REIR edges.
pub fn native_boundaries_to_edges(input: &RsScriptPackageReviewInput) -> Vec<Edge> {
    let mut edges = Vec::new();
    let package_subject = package_subject(&input.package_name, &input.version);

    for dependency in &input.dependencies {
        let dependency_subject = dependency_subject(dependency);
        let dependency_slug = normalized_id(&dependency_subject.id);
        edges.push(Edge {
            schema: EDGE_SCHEMA.to_owned(),
            id: format!(
                "edge.package.{}.depends_on.{}",
                normalized_id(&package_subject.id),
                dependency_slug
            ),
            kind: EdgeKind::DependsOn,
            from: package_subject.clone(),
            to: dependency_subject,
            confidence: confidence(dependency_confidence(dependency), PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                dependency.requirement.clone(),
                Some(dependency_summary(dependency)),
            )],
        });
    }

    for boundary in &input.native_boundaries {
        let to = native_boundary_subject(&input.package_name, &boundary.module_name);

        if boundary.functions.is_empty() {
            edges.push(native_edge(
                format!(
                    "edge.crosses_native.{}.{}",
                    normalized_id(&package_subject.id),
                    normalized_id(&to.id)
                ),
                package_subject.clone(),
                to.clone(),
                &boundary.file,
                boundary.line,
                &boundary.module_name,
            ));
            continue;
        }

        for function_name in &boundary.functions {
            let from = function_subject(&input.package_name, function_name);
            edges.push(Edge {
                schema: EDGE_SCHEMA.to_owned(),
                id: format!(
                    "edge.normalizes_to_native_fn.{}.{}",
                    normalized_id(&to.id),
                    normalized_id(&from.id)
                ),
                kind: EdgeKind::NormalizesToNativeFn,
                from: to.clone(),
                to: from.clone(),
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![source_span(
                    &boundary.file,
                    boundary.line,
                    function_name,
                    Some(format!(
                        "native module {} normalizes to native function {}",
                        boundary.module_name, function_name
                    )),
                    PACKAGE_REVIEW_SOURCE,
                )],
            });
            edges.push(native_edge(
                format!(
                    "edge.crosses_native.{}.{}",
                    normalized_id(&from.id),
                    normalized_id(&to.id)
                ),
                from,
                to.clone(),
                &boundary.file,
                boundary.line,
                function_name,
            ));
        }
    }

    for export in input
        .exports
        .iter()
        .filter(|export| export.kind == "protocol_impl")
    {
        let Some((protocol, _type_name)) = protocol_impl_parts(export) else {
            continue;
        };
        let from = public_contract_subject(&input.package_name, export);
        let to = protocol_subject(&input.package_name, &protocol);
        edges.push(Edge {
            schema: EDGE_SCHEMA.to_owned(),
            id: format!(
                "edge.protocol_impl.{}.implements.{}",
                normalized_id(&from.id),
                normalized_id(&to.id)
            ),
            kind: EdgeKind::ImplementsProtocol,
            from,
            to,
            confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some(export.kind.clone()),
                Some(package_export_summary(export)),
            )],
        });
    }

    edges
}

/// Build a complete REIR bundle from RSScript compiler output.
pub fn rsscript_to_bundle(
    review_map: &RsScriptReviewMapInput,
    package_review: &RsScriptPackageReviewInput,
) -> Bundle {
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-language".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(PRODUCER_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(review_map_to_facts(review_map));
    bundle.facts.extend(package_review_to_facts(package_review));
    bundle
        .edges
        .extend(native_boundaries_to_edges(package_review));
    finalize_bundle(&mut bundle);
    bundle
}

/// Build a REIR bundle from the JSON emitted by `rss review --map --json` and
/// `rss pkg review --json`.
pub fn rsscript_json_to_bundle(
    review_map_json: Option<&str>,
    package_review_json: Option<&str>,
    package_name: Option<&str>,
) -> Result<Bundle, serde_json::Error> {
    let package_review = match package_review_json {
        Some(json) => Some(package_review_input_from_json(json)?),
        None => None,
    };
    let fallback_package = package_review
        .as_ref()
        .map(|review| review.package_name.as_str())
        .or(package_name)
        .unwrap_or("rsscript");
    let review_map = match review_map_json {
        Some(json) => Some(review_map_input_from_json(fallback_package, json)?),
        None => embedded_review_map_value(package_review_json)?
            .map(|value| review_map_input_from_value(fallback_package, value)),
    };

    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-language".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(PRODUCER_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    if let Some(review_map) = &review_map {
        bundle.facts.extend(review_map_to_facts(review_map));
    }
    if let Some(package_review) = &package_review {
        bundle.facts.extend(package_review_to_facts(package_review));
        bundle
            .edges
            .extend(native_boundaries_to_edges(package_review));
    }
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg lock --json`.
pub fn rsscript_lock_json_to_bundle(lock_json: &str) -> Result<Bundle, serde_json::Error> {
    let lock = package_lock_input_from_json(lock_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-lockfile".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(LOCKFILE_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(package_lock_to_facts(&lock));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg check --json`.
pub fn rsscript_check_json_to_bundle(check_json: &str) -> Result<Bundle, serde_json::Error> {
    let check = package_check_input_from_json(check_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-package-check".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(PACKAGE_CHECK_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(package_check_to_facts(&check));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg review update --json`.
pub fn rsscript_lock_diff_json_to_bundle(
    lock_diff_json: &str,
) -> Result<Bundle, serde_json::Error> {
    let diff = package_lock_diff_input_from_json(lock_diff_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-lock-diff".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(LOCKFILE_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(package_lock_diff_to_facts(&diff));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg tree --json`.
pub fn rsscript_tree_json_to_bundle(tree_json: &str) -> Result<Bundle, serde_json::Error> {
    let tree = package_tree_input_from_json(tree_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-package-tree".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(TREE_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(package_tree_to_facts(&tree));
    bundle.edges.extend(package_tree_to_edges(&tree));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg publish --dry-run --json`.
pub fn rsscript_publish_json_to_bundle(publish_json: &str) -> Result<Bundle, serde_json::Error> {
    let publish = package_publish_input_from_json(publish_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-publish".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(PUBLISH_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(package_publish_to_facts(&publish));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg metadata --json`.
pub fn rsscript_metadata_json_to_bundle(metadata_json: &str) -> Result<Bundle, serde_json::Error> {
    let metadata = package_metadata_input_from_json(metadata_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-metadata".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(PACKAGE_METADATA_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle
        .facts
        .extend(package_metadata_report_to_facts(&metadata));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

/// Build a REIR bundle from the JSON emitted by `rss pkg vendor --json`.
pub fn rsscript_vendor_json_to_bundle(vendor_json: &str) -> Result<Bundle, serde_json::Error> {
    let vendor = package_vendor_input_from_json(vendor_json)?;
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-vendor".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(VENDOR_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(package_vendor_to_facts(&vendor));
    finalize_bundle(&mut bundle);
    Ok(bundle)
}

pub fn review_map_input_from_json(
    package_name: &str,
    json: &str,
) -> Result<RsScriptReviewMapInput, serde_json::Error> {
    let value = serde_json::from_str(json)?;
    Ok(review_map_input_from_value(package_name, value))
}

pub fn package_review_input_from_json(
    json: &str,
) -> Result<RsScriptPackageReviewInput, serde_json::Error> {
    let value: Value = serde_json::from_str(json)?;
    Ok(package_review_input_from_value(&value))
}

pub fn package_lock_input_from_json(
    json: &str,
) -> Result<RsScriptPackageLockInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_check_input_from_json(
    json: &str,
) -> Result<RsScriptPackageCheckInput, serde_json::Error> {
    let value: Value = serde_json::from_str(json)?;
    let mut input: RsScriptPackageCheckInput = serde_json::from_value(value.clone())?;
    input.diagnostics = package_diagnostics_from_json(&value);
    Ok(input)
}

pub fn package_lock_diff_input_from_json(
    json: &str,
) -> Result<RsScriptPackageLockDiffInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_tree_input_from_json(
    json: &str,
) -> Result<RsScriptPackageTreeInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_publish_input_from_json(
    json: &str,
) -> Result<RsScriptPackagePublishInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_metadata_input_from_json(
    json: &str,
) -> Result<RsScriptPackageMetadataInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_vendor_input_from_json(
    json: &str,
) -> Result<RsScriptPackageVendorInput, serde_json::Error> {
    serde_json::from_str(json)
}

impl RsScriptPackageRisk {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Elevated => "elevated",
            Self::High => "high",
            Self::Unknown => "unknown",
        }
    }
}

fn embedded_review_map_value(json: Option<&str>) -> Result<Option<Value>, serde_json::Error> {
    let Some(json) = json else {
        return Ok(None);
    };
    let value = serde_json::from_str::<Value>(json)?;
    Ok(value.get("review_map").cloned())
}

fn review_map_input_from_value(package_name: &str, value: Value) -> RsScriptReviewMapInput {
    let modules = value
        .get("modules")
        .and_then(Value::as_array)
        .map(|modules| {
            modules
                .iter()
                .map(|module| RsScriptModuleInput {
                    file: string_field(module, "file").unwrap_or_default(),
                    module_path: string_field(module, "module_path").unwrap_or_default(),
                    line: usize_field(module, "line").unwrap_or(1),
                    uses: module
                        .get("uses")
                        .and_then(Value::as_array)
                        .map(|uses| {
                            uses.iter()
                                .map(|use_decl| RsScriptUseInput {
                                    path: string_field(use_decl, "path").unwrap_or_default(),
                                    line: usize_field(use_decl, "line").unwrap_or(1),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut regions = Vec::new();
    for file in files {
        let file_name = string_field(&file, "file").unwrap_or_default();
        let file_regions = file
            .get("regions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for region in file_regions {
            regions.push(RsScriptRegionInput {
                file: file_name.clone(),
                function_name: string_field(&region, "function").unwrap_or_default(),
                classification: classification_from_json(&region),
                line: usize_field(&region, "line").unwrap_or(1),
                reasons: string_array_field(&region, "reasons"),
            });
        }
    }
    RsScriptReviewMapInput {
        package_name: package_name.to_owned(),
        modules,
        regions,
    }
}

fn package_review_input_from_value(value: &Value) -> RsScriptPackageReviewInput {
    let package = value.get("package").unwrap_or(&Value::Null);
    let summary = value.get("summary").unwrap_or(&Value::Null);
    let package_name = string_field(package, "name").unwrap_or_else(|| "rsscript".to_owned());
    let version = string_field(package, "version").unwrap_or_else(|| "0.0.0".to_owned());
    let exports = value
        .get("exports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    RsScriptPackageReviewInput {
        package_name: package_name.clone(),
        version,
        risk: package_risk_from_json(value),
        features: string_array_field(value, "features"),
        implements: package_implements_from_json(value),
        dependencies: package_dependencies_from_json(value),
        exports: package_exports_from_json(&exports),
        capabilities: package_capabilities_from_json(value),
        await_sites: package_await_sites_from_json(value),
        diagnostics: package_diagnostics_from_json(value),
        public_apis: usize_field(summary, "public_apis").unwrap_or(0),
        mutating_apis: usize_field(summary, "mutating_apis").unwrap_or(0),
        retaining_apis: usize_field(summary, "retaining_apis").unwrap_or(0),
        resource_apis: usize_field(summary, "resource_apis").unwrap_or(0),
        native_apis: usize_field(summary, "native_apis").unwrap_or(0),
        unsafe_apis: usize_field(summary, "unsafe_apis").unwrap_or(0),
        unknown_apis: usize_field(summary, "unknown_apis").unwrap_or(0),
        native_boundaries: native_boundaries_from_exports(&package_name, value, &exports),
        native_cargo_features: native_cargo_features_from_json(value),
        native_author_declaration: native_author_declaration_from_json(value),
        native_source_scan: native_source_scan_from_json(value),
    }
}

fn package_implements_from_json(value: &Value) -> Vec<RsScriptProviderImplementation> {
    value
        .get("implements")
        .and_then(Value::as_array)
        .map(|implements| {
            implements
                .iter()
                .map(|implementation| RsScriptProviderImplementation {
                    interface_package: string_field(implementation, "interface_package")
                        .unwrap_or_else(|| "unknown".to_owned()),
                    version: string_field(implementation, "version"),
                    interface_features: string_array_field(implementation, "interface_features"),
                    interface_effective_hash: string_field(
                        implementation,
                        "interface_effective_hash",
                    ),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn package_capabilities_from_json(value: &Value) -> Vec<RsScriptPackageCapability> {
    value
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|capabilities| {
            capabilities
                .iter()
                .map(|capability| RsScriptPackageCapability {
                    function: string_field(capability, "function")
                        .unwrap_or_else(|| "unknown".to_owned()),
                    binding_symbol: string_field(capability, "binding_symbol")
                        .unwrap_or_else(|| "unknown".to_owned()),
                    category: string_field(capability, "category")
                        .unwrap_or_else(|| "unknown".to_owned())
                        .try_into()
                        .expect("capability category conversion is infallible"),
                    provider: string_field(capability, "provider"),
                    service: string_field(capability, "service"),
                    action: string_field(capability, "action"),
                    resource: string_field(capability, "resource"),
                    call_chain: string_array_field(capability, "call_chain"),
                    span: capability
                        .get("span")
                        .map(|span| diagnostic_span_from_json(span, String::new())),
                    unknown_reason: string_field(capability, "unknown_reason"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn native_cargo_features_from_json(value: &Value) -> Vec<String> {
    value
        .get("native_rust")
        .map(|native| string_array_field(native, "cargo_features"))
        .unwrap_or_default()
}

fn native_author_declaration_from_json(value: &Value) -> Option<RsScriptNativeAuthorDeclaration> {
    let declaration = value
        .get("native_rust")?
        .get("semantic")?
        .get("author_declaration")?;
    Some(RsScriptNativeAuthorDeclaration {
        worker_thread_parallelism: bool_field(declaration, "worker_thread_parallelism")
            .unwrap_or(false),
        native_parallel_backend: string_field(declaration, "native_parallel_backend"),
        risk_reasons: string_array_field(declaration, "risk_reasons"),
    })
}

fn package_diagnostics_from_json(value: &Value) -> Vec<RsScriptDiagnosticInput> {
    value
        .get("diagnostics")
        .and_then(Value::as_array)
        .map(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| RsScriptDiagnosticInput {
                    code: string_field(diagnostic, "code").unwrap_or_else(|| "unknown".to_owned()),
                    severity: string_field(diagnostic, "severity")
                        .unwrap_or_else(|| "error".to_owned()),
                    summary: string_field(diagnostic, "summary").unwrap_or_default(),
                    spans: diagnostic_spans_from_json(diagnostic),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diagnostic_spans_from_json(value: &Value) -> Vec<RsScriptDiagnosticSpan> {
    if let Some(span) = value.get("span") {
        return vec![diagnostic_span_from_json(
            span,
            string_field(value, "label").unwrap_or_default(),
        )];
    }
    value
        .get("spans")
        .and_then(Value::as_array)
        .map(|spans| {
            spans
                .iter()
                .map(|span| {
                    diagnostic_span_from_json(span, string_field(span, "label").unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diagnostic_span_from_json(value: &Value, label: String) -> RsScriptDiagnosticSpan {
    RsScriptDiagnosticSpan {
        file: string_field(value, "file").unwrap_or_default(),
        line: usize_field(value, "line").unwrap_or(1),
        column: usize_field(value, "column").unwrap_or(1),
        length: usize_field(value, "length").unwrap_or(0),
        label,
    }
}

fn package_await_sites_from_json(value: &Value) -> Vec<RsScriptPackageAwaitSite> {
    value
        .get("await_sites")
        .and_then(Value::as_array)
        .map(|sites| {
            sites
                .iter()
                .map(|site| {
                    let span = site.get("span").unwrap_or(&Value::Null);
                    RsScriptPackageAwaitSite {
                        function: string_field(site, "function")
                            .unwrap_or_else(|| "unknown".to_owned()),
                        callee: string_field(site, "callee"),
                        boundary: string_field(site, "boundary")
                            .unwrap_or_else(|| "unknown".to_owned()),
                        live_across_await: string_array_field(site, "live_across_await"),
                        file: string_field(span, "file").unwrap_or_default(),
                        line: usize_field(span, "line").unwrap_or(1),
                        column: usize_field(span, "column").unwrap_or(1),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn native_source_scan_from_json(value: &Value) -> Option<RsScriptNativeSourceScan> {
    let scan = value
        .get("native_rust")?
        .get("semantic")?
        .get("source_scan_best_effort")?;
    Some(RsScriptNativeSourceScan {
        tool: string_field(scan, "tool").unwrap_or_else(|| "rss-native-source-scan".to_owned()),
        selected_graph: string_field(scan, "selected_graph")
            .unwrap_or_else(|| "package-native-rust".to_owned()),
        worker_thread_parallelism_detected: bool_field(scan, "worker_thread_parallelism_detected")
            .unwrap_or(false),
        unsafe_detected: bool_field(scan, "unsafe_detected").unwrap_or(false),
        ffi_detected: bool_field(scan, "ffi_detected").unwrap_or(false),
        filesystem_detected: bool_field(scan, "filesystem_detected").unwrap_or(false),
        network_detected: bool_field(scan, "network_detected").unwrap_or(false),
        build_script_present: bool_field(scan, "build_script_present").unwrap_or(false),
    })
}

fn package_exports_from_json(exports: &[Value]) -> Vec<RsScriptPackageExport> {
    exports
        .iter()
        .map(|export| RsScriptPackageExport {
            name: string_field(export, "name").unwrap_or_else(|| "unknown".to_owned()),
            kind: string_field(export, "kind").unwrap_or_else(|| "unknown".to_owned()),
            classification: string_field(export, "classification").unwrap_or_default(),
            reasons: string_array_field(export, "reasons"),
            normalized_effects: string_array_field(export, "normalized_effects"),
        })
        .collect()
}

fn package_dependencies_from_json(value: &Value) -> Vec<RsScriptPackageDependency> {
    value
        .get("dependencies")
        .and_then(Value::as_array)
        .map(|dependencies| {
            dependencies
                .iter()
                .map(|dependency| RsScriptPackageDependency {
                    name: string_field(dependency, "name").unwrap_or_else(|| "unknown".to_owned()),
                    requirement: string_field(dependency, "requirement"),
                    source: string_field(dependency, "source")
                        .unwrap_or_else(|| "registry".to_owned()),
                    features: string_array_field(dependency, "features"),
                    dependency_kind: string_field(dependency, "dependency_kind")
                        .unwrap_or_else(|| "normal".to_owned()),
                    compile_only: bool_field(dependency, "compile_only").unwrap_or(false),
                    test_only: bool_field(dependency, "test_only").unwrap_or(false),
                    platform_provided: bool_field(dependency, "platform_provided").unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn classification_from_json(value: &Value) -> RsScriptClassification {
    match string_field(value, "classification").as_deref() {
        Some("low_semantic_risk" | "foldable") => RsScriptClassification::Foldable,
        Some("unknown") => RsScriptClassification::Unknown,
        _ => RsScriptClassification::ReviewRequired,
    }
}

fn package_risk_from_json(value: &Value) -> RsScriptPackageRisk {
    match string_field(value, "risk").as_deref() {
        Some("low") => RsScriptPackageRisk::Low,
        Some("elevated") => RsScriptPackageRisk::Elevated,
        Some("unknown") => RsScriptPackageRisk::Unknown,
        _ => RsScriptPackageRisk::High,
    }
}

fn native_boundaries_from_exports(
    package_name: &str,
    value: &Value,
    exports: &[Value],
) -> Vec<RsScriptNativeBoundary> {
    let file = value
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| string_field(file, "path"))
        .unwrap_or_else(|| format!("{package_name}.rssi"));
    let mut boundaries = BTreeMap::<String, RsScriptNativeBoundary>::new();
    for export in exports.iter().filter(|export| {
        string_array_field(export, "normalized_effects")
            .iter()
            .any(|effect| effect == "native")
    }) {
        let function = string_field(export, "name").unwrap_or_default();
        let (file, line) =
            source_span_for_function(value, &function).unwrap_or_else(|| (file.clone(), 1));
        let module_name = function
            .rsplit_once('.')
            .map(|(module, _)| module.to_owned())
            .unwrap_or_else(|| "native".to_owned());
        let boundary =
            boundaries
                .entry(module_name.clone())
                .or_insert_with(|| RsScriptNativeBoundary {
                    module_name,
                    functions: Vec::new(),
                    file,
                    line,
                });
        if !function.is_empty() {
            boundary.functions.push(function);
        }
    }
    for boundary in boundaries.values_mut() {
        boundary.functions.sort();
        boundary.functions.dedup();
    }
    boundaries.into_values().collect()
}

fn source_span_for_function(value: &Value, function_name: &str) -> Option<(String, usize)> {
    value
        .get("review_map")
        .and_then(|review_map| review_map.get("files"))
        .and_then(Value::as_array)
        .and_then(|files| {
            files.iter().find_map(|file| {
                let file_name = string_field(file, "file")?;
                file.get("regions")
                    .and_then(Value::as_array)
                    .and_then(|regions| {
                        regions.iter().find_map(|region| {
                            let region_function = string_field(region, "function")?;
                            (region_function == function_name).then(|| {
                                (file_name.clone(), usize_field(region, "line").unwrap_or(1))
                            })
                        })
                    })
            })
        })
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn usize_field(value: &Value, field: &str) -> Option<usize> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn string_array_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn bool_field(value: &Value, field: &str) -> Option<bool> {
    value.get(field).and_then(Value::as_bool)
}

fn classify_review_required(reasons: &[String]) -> FactKind {
    let lower_reasons = reasons
        .iter()
        .map(|reason| reason.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if lower_reasons
        .iter()
        .any(|reason| matches!(reason.as_str(), "native boundary" | "native bridge"))
    {
        FactKind::NativeBoundary
    } else if lower_reasons.iter().any(|reason| reason.contains("retain")) {
        FactKind::Retention
    } else if lower_reasons.iter().any(|reason| reason.contains("mut")) {
        FactKind::Mutation
    } else {
        FactKind::Extension(REVIEW_REQUIRED_KIND.to_owned())
    }
}

fn function_subject(package_name: &str, function_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeFunction,
        id: function_subject_id(package_name, function_name),
        name: Some(function_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn module_use_facts(input: &RsScriptReviewMapInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    for module in &input.modules {
        let module_subject = module_subject(&input.package_name, &module.module_path);
        let module_slug = normalized_id(&module_subject.id);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.module.{module_slug}.declaration"),
            kind: FactKind::ModuleDeclaration,
            role: None,
            subject: module_subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![source_span(
                &module.file,
                module.line,
                &module.module_path,
                Some("RSScript module declaration".to_owned()),
                REVIEW_MAP_SOURCE,
            )],
            unknown_reason: None,
        });

        for use_decl in &module.uses {
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.module.{}.uses.{}",
                    module_slug,
                    normalized_id(&use_decl.path)
                ),
                kind: FactKind::UseDeclaration,
                role: None,
                subject: module_subject.clone(),
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![source_span(
                    &module.file,
                    use_decl.line,
                    &use_decl.path,
                    Some("RSScript use declaration; no implicit method resolution".to_owned()),
                    REVIEW_MAP_SOURCE,
                )],
                unknown_reason: None,
            });
        }
    }
    facts
}

fn module_subject(package_name: &str, module_path: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeModule,
        id: format!("{package_name}::module::{module_path}"),
        name: Some(module_path.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn function_subject_id(package_name: &str, function_name: &str) -> String {
    format!("{package_name}::{function_name}")
}

fn package_subject(package_name: &str, version: &str) -> Subject {
    Subject {
        kind: SubjectKind::Package,
        id: format!("{package_name}@{version}"),
        name: Some(package_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn dependency_subject(dependency: &RsScriptPackageDependency) -> Subject {
    let identity = dependency
        .requirement
        .as_ref()
        .map(|requirement| format!("{}@{}", dependency.name, requirement))
        .unwrap_or_else(|| format!("{}@{}", dependency.name, dependency.source));
    Subject {
        kind: SubjectKind::Package,
        id: identity,
        name: Some(dependency.name.clone()),
        package: Some(dependency.name.clone()),
    }
}

fn native_boundary_subject(package_name: &str, module_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::NativeBoundary,
        id: format!("{package_name}::native::{module_name}"),
        name: Some(module_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn public_contract_subject(package_name: &str, export: &RsScriptPackageExport) -> Subject {
    let kind = match export.kind.as_str() {
        "type" | "sum_type" | "type_alias" => SubjectKind::CodeType,
        "protocol" => SubjectKind::CodeProtocol,
        "protocol_impl" => SubjectKind::CodeProtocolImpl,
        "function" | "const" => SubjectKind::CodePublicApi,
        _ => SubjectKind::CodeInterfaceSymbol,
    };
    Subject {
        kind,
        id: format!("{}::public::{}::{}", package_name, export.kind, export.name),
        name: Some(export.name.clone()),
        package: Some(package_name.to_owned()),
    }
}

fn package_function_subject(package_name: &str, function: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeFunction,
        id: format!("{package_name}::function::{function}"),
        name: Some(function.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn protocol_declaration_fact(
    export: &RsScriptPackageExport,
    export_subject: &Subject,
    package_name: &str,
    version: &str,
    package_slug: &str,
) -> Option<Fact> {
    (export.kind == "protocol").then(|| Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.protocol_declaration.{}.{}",
            package_slug,
            normalized_id(&export_subject.id)
        ),
        kind: FactKind::ProtocolDeclaration,
        role: None,
        subject: export_subject.clone(),
        capability: None,
        value: public_contract_value(export),
        confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(export.kind.clone()),
            Some(package_export_summary(export)),
        )],
        unknown_reason: matches!(public_contract_value(export), FactValue::Unknown)
            .then(|| public_contract_unknown_reason(export)),
    })
}

fn protocol_impl_fact(
    export: &RsScriptPackageExport,
    export_subject: &Subject,
    package_name: &str,
    version: &str,
    package_slug: &str,
) -> Option<Fact> {
    (export.kind == "protocol_impl").then(|| Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.protocol_impl.{}.{}",
            package_slug,
            normalized_id(&export_subject.id)
        ),
        kind: FactKind::ProtocolImpl,
        role: None,
        subject: export_subject.clone(),
        capability: None,
        value: public_contract_value(export),
        confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(export.kind.clone()),
            Some(package_export_summary(export)),
        )],
        unknown_reason: matches!(public_contract_value(export), FactValue::Unknown)
            .then(|| public_contract_unknown_reason(export)),
    })
}

fn protocol_method_contract_facts(
    input: &RsScriptPackageReviewInput,
    package_slug: &str,
    protocol_names: &[String],
) -> Vec<Fact> {
    input
        .exports
        .iter()
        .filter(|export| export.kind == "function")
        .filter_map(|export| {
            let (namespace, method) = export.name.split_once('.')?;
            protocol_names
                .iter()
                .any(|protocol| protocol == namespace)
                .then(|| {
                    let subject = Subject {
                        kind: SubjectKind::CodeProtocolMethod,
                        id: format!(
                            "{}::protocol::{}::method::{}",
                            input.package_name, namespace, method
                        ),
                        name: Some(export.name.clone()),
                        package: Some(input.package_name.clone()),
                    };
                    Fact {
                        schema: FACT_SCHEMA.to_owned(),
                        id: format!(
                            "fact.protocol_method_contract.{}.{}",
                            package_slug,
                            normalized_id(&subject.id)
                        ),
                        kind: FactKind::ProtocolMethodContract,
                        role: None,
                        subject,
                        capability: None,
                        value: public_contract_value(export),
                        confidence: confidence(
                            public_contract_confidence(export),
                            PACKAGE_REVIEW_SOURCE,
                        ),
                        acquisition_mode: AcquisitionMode::CompilerContract,
                        precision: Precision::Exact,
                        evidence: vec![package_metadata(
                            &input.package_name,
                            &input.version,
                            Some(export.kind.clone()),
                            Some(package_export_summary(export)),
                        )],
                        unknown_reason: matches!(public_contract_value(export), FactValue::Unknown)
                            .then(|| public_contract_unknown_reason(export)),
                    }
                })
        })
        .collect()
}

fn protocol_subject(package_name: &str, protocol: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeProtocol,
        id: format!("{package_name}::public::protocol::{protocol}"),
        name: Some(protocol.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn protocol_impl_parts(export: &RsScriptPackageExport) -> Option<(String, String)> {
    let protocol = export
        .reasons
        .iter()
        .find_map(|reason| backtick_value_after_prefix(reason, "protocol "))
        .or_else(|| {
            export
                .name
                .split_once(" for ")
                .map(|(protocol, _)| protocol.to_owned())
        })?;
    let type_name = export
        .reasons
        .iter()
        .find_map(|reason| backtick_value_after_prefix(reason, "type "))
        .or_else(|| {
            export
                .name
                .split_once(" for ")
                .map(|(_, type_name)| type_name.to_owned())
        })?;
    Some((protocol, type_name))
}

fn backtick_value_after_prefix(reason: &str, prefix: &str) -> Option<String> {
    let rest = reason.strip_prefix(prefix)?;
    let value = rest.strip_prefix('`')?.split_once('`')?.0;
    (!value.is_empty()).then(|| value.to_owned())
}

fn public_contract_value(export: &RsScriptPackageExport) -> FactValue {
    if export.classification == "unknown" || export.kind == "contract_diagnostic" {
        FactValue::Unknown
    } else {
        FactValue::True
    }
}

fn public_contract_confidence(export: &RsScriptPackageExport) -> ConfidenceLevel {
    if matches!(public_contract_value(export), FactValue::Unknown) {
        ConfidenceLevel::Unknown
    } else {
        ConfidenceLevel::Authoritative
    }
}

fn public_contract_unknown_reason(export: &RsScriptPackageExport) -> String {
    let reason = if export.reasons.is_empty() {
        "package public contract could not be classified".to_owned()
    } else {
        export.reasons.join("; ")
    };
    format!(
        "public contract export `{}` is unknown: {}",
        export.name, reason
    )
}

fn package_risk_value(risk: &RsScriptPackageRisk) -> FactValue {
    match risk {
        RsScriptPackageRisk::Unknown => FactValue::Unknown,
        _ => FactValue::True,
    }
}

fn package_risk_confidence(risk: &RsScriptPackageRisk) -> ConfidenceLevel {
    match risk {
        RsScriptPackageRisk::Unknown => ConfidenceLevel::Unknown,
        _ => ConfidenceLevel::Authoritative,
    }
}

fn dependency_risk_value(dependency: &RsScriptPackageDependency) -> FactValue {
    if dependency.source.starts_with("path+") || dependency.platform_provided {
        FactValue::True
    } else {
        FactValue::Unknown
    }
}

fn dependency_confidence(dependency: &RsScriptPackageDependency) -> ConfidenceLevel {
    if dependency.source.starts_with("path+") || dependency.platform_provided {
        ConfidenceLevel::Scanned
    } else {
        ConfidenceLevel::Unknown
    }
}

fn dependency_summary(dependency: &RsScriptPackageDependency) -> String {
    let requirement = dependency.requirement.as_deref().unwrap_or("unspecified");
    let features = if dependency.features.is_empty() {
        "none".to_owned()
    } else {
        dependency.features.join(",")
    };
    format!(
        "dependency {} requirement={} source={} kind={} features={} compile_only={} test_only={} platform_provided={}",
        dependency.name,
        requirement,
        dependency.source,
        dependency.dependency_kind,
        features,
        dependency.compile_only,
        dependency.test_only,
        dependency.platform_provided
    )
}

fn collect_package_tree_facts(node: &RsScriptPackageTreeNode, path: &str, facts: &mut Vec<Fact>) {
    let subject = package_tree_node_subject(node);
    let path_slug = normalized_id(path);
    let unknown = matches!(node.risk, RsScriptPackageRisk::Unknown);
    facts.push(Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_tree.{}.risk", path_slug),
        kind: if node.dependency_kind == "root" {
            FactKind::PackageRisk
        } else {
            FactKind::DependencyRisk
        },
        role: None,
        subject,
        capability: None,
        value: package_risk_value(&node.risk),
        confidence: confidence(package_tree_confidence(node), TREE_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![dependency_path_evidence(
            node,
            path,
            Some(node.risk.as_str().to_owned()),
            Some(package_tree_node_summary(node)),
        )],
        unknown_reason: unknown.then(|| package_tree_unknown_reason(node)),
    });
    if !node.interface_effective_hash.is_empty() {
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.package_tree.{}.effective_interface_hash", path_slug),
            kind: FactKind::SupplyChain,
            role: None,
            subject: package_tree_node_subject(node),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Computed, TREE_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: vec![dependency_path_evidence(
                node,
                path,
                Some(node.interface_effective_hash.clone()),
                Some(format!(
                    "effective_interface_hash={} features={}",
                    node.interface_effective_hash,
                    tree_features_summary(&node.features)
                )),
            )],
            unknown_reason: None,
        });
    }
    for (index, dependency) in node.dependencies.iter().enumerate() {
        collect_package_tree_facts(dependency, &format!("{path}/{}", index + 1), facts);
    }
}

fn collect_package_tree_edges(node: &RsScriptPackageTreeNode, path: &str, edges: &mut Vec<Edge>) {
    let from = package_tree_node_subject(node);
    for (index, dependency) in node.dependencies.iter().enumerate() {
        let child_path = format!("{path}/{}", index + 1);
        edges.push(Edge {
            schema: EDGE_SCHEMA.to_owned(),
            id: format!(
                "edge.package_tree.{}.depends_on.{}",
                normalized_id(path),
                normalized_id(&child_path)
            ),
            kind: EdgeKind::DependsOn,
            from: from.clone(),
            to: package_tree_node_subject(dependency),
            confidence: confidence(package_tree_confidence(dependency), TREE_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![dependency_path_evidence(
                dependency,
                &child_path,
                dependency.requirement.clone(),
                Some(package_tree_node_summary(dependency)),
            )],
        });
        collect_package_tree_edges(dependency, &child_path, edges);
    }
}

fn package_tree_node_subject(node: &RsScriptPackageTreeNode) -> Subject {
    if let Some(version) = &node.version {
        package_subject(&node.name, version)
    } else {
        let identity = node
            .requirement
            .as_ref()
            .map(|requirement| format!("{}@{}", node.name, requirement))
            .unwrap_or_else(|| format!("{}@{}", node.name, node.source));
        Subject {
            kind: SubjectKind::Dependency,
            id: identity,
            name: Some(node.name.clone()),
            package: Some(node.name.clone()),
        }
    }
}

fn package_tree_confidence(node: &RsScriptPackageTreeNode) -> ConfidenceLevel {
    if node.source.starts_with("path+") || node.platform_provided {
        ConfidenceLevel::Scanned
    } else {
        ConfidenceLevel::Unknown
    }
}

fn package_tree_unknown_reason(node: &RsScriptPackageTreeNode) -> String {
    if node.reasons.is_empty() {
        format!(
            "dependency `{}` could not be resolved by package tree",
            node.name
        )
    } else {
        node.reasons.join("; ")
    }
}

fn package_tree_node_summary(node: &RsScriptPackageTreeNode) -> String {
    format!(
        "dependency {} version={} requirement={} source={} kind={} features={} native={} interface_only={} compile_only={} test_only={} platform_provided={} reasons={}",
        node.name,
        node.version.as_deref().unwrap_or("unresolved"),
        node.requirement.as_deref().unwrap_or("unspecified"),
        node.source,
        node.dependency_kind,
        tree_features_summary(&node.features),
        node.native,
        node.interface_only,
        node.compile_only,
        node.test_only,
        node.platform_provided,
        if node.reasons.is_empty() {
            "none".to_owned()
        } else {
            node.reasons.join("; ")
        }
    )
}

fn dependency_path_evidence(
    node: &RsScriptPackageTreeNode,
    path: &str,
    value: Option<String>,
    reason: Option<String>,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::DependencyPath,
        file: package_tree_evidence_file(node),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: Some(format!("/root{}", dependency_path_json_pointer(path))),
        resource: Some(format!(
            "{}@{}",
            node.name,
            node.version
                .as_deref()
                .or(node.requirement.as_deref())
                .unwrap_or("unresolved")
        )),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(TREE_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_tree_evidence_file(node: &RsScriptPackageTreeNode) -> Option<String> {
    node.version.as_ref()?;
    node.source
        .strip_prefix("path+")
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

fn dependency_path_json_pointer(path: &str) -> String {
    path.strip_prefix("root")
        .unwrap_or(path)
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let index = part.parse::<usize>().unwrap_or(1).saturating_sub(1);
            format!("/dependencies/{index}")
        })
        .collect::<String>()
}

fn tree_features_summary(features: &[String]) -> String {
    if features.is_empty() {
        "none".to_owned()
    } else {
        features.join(",")
    }
}

fn publish_registry_boundary_fact(
    package_slug: &str,
    field: &str,
    kind: FactKind,
    subject: &Subject,
    input: &RsScriptPackagePublishInput,
    present: bool,
    reason: &str,
    json_pointer: &str,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.publish.{}.{}", package_slug, field),
        kind,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if present {
            FactValue::True
        } else {
            FactValue::False
        },
        confidence: confidence(ConfidenceLevel::Computed, PUBLISH_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![registry_metadata_evidence(
            input,
            Some(present.to_string()),
            Some(reason.to_owned()),
            json_pointer,
        )],
        unknown_reason: None,
    }
}

fn publish_supply_chain_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackagePublishInput,
    value: &str,
    reason: &str,
    json_pointer: &str,
) -> Fact {
    let missing_value = value.is_empty();
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.publish.{}.{}", package_slug, field),
        kind: FactKind::SupplyChain,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if missing_value {
            FactValue::Unknown
        } else {
            FactValue::True
        },
        confidence: confidence(ConfidenceLevel::Computed, PUBLISH_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Exact,
        evidence: vec![registry_metadata_evidence(
            input,
            (!missing_value).then(|| value.to_owned()),
            Some(reason.to_owned()),
            json_pointer,
        )],
        unknown_reason: missing_value.then(|| format!("{reason} is missing")),
    }
}

fn registry_footprint_fact(
    package_slug: &str,
    subject: &Subject,
    input: &RsScriptPackagePublishInput,
    footprint: &RsScriptRegistryFootprintInput,
) -> Fact {
    let summary = registry_footprint_summary(footprint);
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.publish.{}.registry_footprint", package_slug),
        kind: FactKind::DependencyRisk,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if footprint.unknown_facts == 0 && footprint.unresolved_dependencies == 0 {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PUBLISH_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![registry_metadata_evidence(
            input,
            Some(summary.clone()),
            Some(format!("registry default dependency footprint {summary}")),
            "/registry_index/footprint_default",
        )],
        unknown_reason: (footprint.unknown_facts > 0 || footprint.unresolved_dependencies > 0)
            .then(|| "registry preview footprint contains unknown or unresolved facts".to_owned()),
    }
}

fn registry_footprint_present(footprint: &RsScriptRegistryFootprintInput) -> bool {
    footprint.direct_dependencies > 0
        || footprint.total_packages > 0
        || footprint.path_dependencies > 0
        || footprint.unresolved_dependencies > 0
        || footprint.native
        || footprint.native_packages > 0
        || footprint.build_time_execution
        || footprint.build_execution_packages > 0
        || footprint.high_risk_packages > 0
        || footprint.unknown_facts > 0
}

fn registry_footprint_summary(footprint: &RsScriptRegistryFootprintInput) -> String {
    format!(
        "direct_dependencies={} total_packages={} path_dependencies={} unresolved_dependencies={} native={} native_packages={} build_time_execution={} build_execution_packages={} high_risk_packages={} unknown_facts={}",
        footprint.direct_dependencies,
        footprint.total_packages,
        footprint.path_dependencies,
        footprint.unresolved_dependencies,
        footprint.native,
        footprint.native_packages,
        footprint.build_time_execution,
        footprint.build_execution_packages,
        footprint.high_risk_packages,
        footprint.unknown_facts
    )
}

fn registry_feature_summary(features: &[String]) -> String {
    if features.is_empty() {
        "none".to_owned()
    } else {
        features.join(",")
    }
}

fn package_check_policy_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackageCheckInput,
    ok: bool,
    risk: &RsScriptPackageRisk,
    reason: String,
    json_pointer: &str,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.{}", package_slug, field),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_check_evidence(
            input,
            Some(risk.as_str().to_owned()),
            Some(reason),
            Some(json_pointer),
        )],
        unknown_reason: (!ok).then(|| format!("package check `{field}` failed")),
    }
}

fn package_check_lock_policy_fact(
    package_slug: &str,
    subject: &Subject,
    input: &RsScriptPackageCheckInput,
    ok: bool,
    risk: &RsScriptPackageRisk,
    reason: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.lock", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::Lockfile,
        precision: Precision::Category,
        evidence: vec![package_check_lock_evidence(
            input,
            Some(risk.as_str().to_owned()),
            Some(reason),
            "/lock",
        )],
        unknown_reason: (!ok).then(|| "package check `lock` failed".to_owned()),
    }
}

fn package_check_boundary_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackageCheckInput,
    kind: FactKind,
    reason: &str,
    json_pointer: &str,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.{}", package_slug, field),
        kind,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_check_evidence(
            input,
            None,
            Some(reason.to_owned()),
            Some(json_pointer),
        )],
        unknown_reason: None,
    }
}

fn metadata_artifact_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackageMetadataInput,
    path: &str,
    reason: &str,
    json_pointer: &str,
) -> Fact {
    let path_has_mismatch = input
        .mismatches
        .iter()
        .any(|mismatch| mismatch.path == path);
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.metadata.{}.{}", package_slug, field),
        kind: FactKind::SupplyChain,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if path_has_mismatch {
            FactValue::Unknown
        } else {
            FactValue::True
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_METADATA_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Exact,
        evidence: vec![metadata_artifact_evidence(
            input,
            Some(path.to_owned()),
            Some(path.to_owned()),
            Some(format!("{reason} {path}")),
            Some(json_pointer),
        )],
        unknown_reason: path_has_mismatch
            .then(|| format!("metadata artifact `{path}` is missing, stale, or unreadable")),
    }
}

fn metadata_artifact_evidence(
    input: &RsScriptPackageMetadataInput,
    file: Option<String>,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: Option<&str>,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::PackageMetadata,
        file,
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: json_pointer.map(str::to_owned),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(PACKAGE_METADATA_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_check_evidence(
    input: &RsScriptPackageCheckInput,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: Option<&str>,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::PackageMetadata,
        file: package_check_evidence_file(input, json_pointer),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: json_pointer.map(str::to_owned),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(PACKAGE_CHECK_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_check_evidence_file(
    input: &RsScriptPackageCheckInput,
    json_pointer: Option<&str>,
) -> Option<String> {
    if json_pointer.is_some_and(|pointer| pointer.starts_with("/native_rust")) {
        if let Some(native) = &input.native_rust {
            if !native.path.is_empty() {
                return Some(package_check_native_evidence_file(input, &native.path));
            }
        }
    }
    if json_pointer.is_some_and(|pointer| pointer.starts_with("/implements")) {
        return Some(
            Path::new(&input.package_dir)
                .join("rsspkg.toml")
                .display()
                .to_string(),
        );
    }
    Some(input.package_dir.clone())
}

fn package_check_native_evidence_file(
    input: &RsScriptPackageCheckInput,
    native_path: &str,
) -> String {
    let path = Path::new(native_path);
    if path.is_absolute() {
        native_path.to_owned()
    } else {
        Path::new(&input.package_dir)
            .join(path)
            .display()
            .to_string()
    }
}

fn package_check_lock_evidence(
    input: &RsScriptPackageCheckInput,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: &str,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::LockfileEntry,
        file: Some(input.lock.path.clone()),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: Some(json_pointer.to_owned()),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(PACKAGE_CHECK_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_check_diagnostic_evidence(
    input: &RsScriptPackageCheckInput,
    diagnostic: &RsScriptDiagnosticInput,
    span: Option<&RsScriptDiagnosticSpan>,
) -> Evidence {
    let mut evidence = diagnostic_evidence(diagnostic, span);
    if let Some(file) = evidence.file.as_deref() {
        let path = Path::new(file);
        if !path.is_absolute() {
            evidence.file = Some(
                Path::new(&input.package_dir)
                    .join(path)
                    .display()
                    .to_string(),
            );
        }
    }
    evidence.source = Some(PACKAGE_CHECK_SOURCE.to_owned());
    evidence
}

fn vendor_evidence(
    input: &RsScriptPackageVendorInput,
    file: String,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: Option<&str>,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::PackageMetadata,
        file: Some(file),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: json_pointer.map(str::to_owned),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(VENDOR_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn registry_metadata_evidence(
    input: &RsScriptPackagePublishInput,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: &str,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::RegistryMetadata,
        file: publish_registry_evidence_file(input, json_pointer),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: Some(json_pointer.to_owned()),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(PUBLISH_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn publish_registry_evidence_file(
    input: &RsScriptPackagePublishInput,
    json_pointer: &str,
) -> Option<String> {
    let target = input.registry_target.as_ref()?;
    if json_pointer.starts_with("/registry_index") {
        Some(target.index_path.clone())
    } else if matches!(json_pointer, "/archive_hash" | "/archive_format")
        || json_pointer.starts_with("/archive_files")
    {
        Some(target.archive_manifest_path.clone())
    } else if json_pointer == "/ready" || json_pointer.starts_with("/checks/") {
        Some(target.registry_dir.clone())
    } else {
        None
    }
}

fn package_publish_summary(input: &RsScriptPackagePublishInput) -> String {
    format!(
        "publish ready={} risk={} archive_format={} registry_schema={} review_schema={} default_features={} footprint={} native={} unsafe={} checks={}",
        input.ready,
        input.risk.as_str(),
        input.archive_format,
        input.registry_index.schema,
        if input.registry_index.review_schema.is_empty() {
            "unknown"
        } else {
            input.registry_index.review_schema.as_str()
        },
        input
            .registry_index
            .features
            .get("default")
            .map(|features| registry_feature_summary(features))
            .unwrap_or_else(|| "unknown".to_owned()),
        registry_footprint_summary(&input.registry_index.footprint_default),
        input.registry_index.native,
        input.registry_index.unsafe_boundary,
        input.checks.len()
    )
}

fn capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    count: usize,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    let mut constraints = HashMap::new();
    constraints.insert("count".to_owned(), count.to_string());

    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: None,
            action: None,
            resource: Some(subject.id.clone()),
            constraints,
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(count.to_string()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn native_edge(
    id: String,
    from: Subject,
    to: Subject,
    file: &str,
    line: usize,
    symbol: &str,
) -> Edge {
    Edge {
        schema: EDGE_SCHEMA.to_owned(),
        id,
        kind: EdgeKind::CrossesNative,
        from,
        to,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![source_span(file, line, symbol, None, PACKAGE_REVIEW_SOURCE)],
    }
}

fn export_capability_fact(
    id: String,
    subject: Subject,
    export: &RsScriptPackageExport,
    category: CapabilityCategory,
    package_name: &str,
    version: &str,
) -> Fact {
    let mut constraints = HashMap::new();
    constraints.insert("symbol".to_owned(), export.name.clone());
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category: category.clone(),
            provider: Some("rsscript".to_owned()),
            service: Some("stdlib".to_owned()),
            action: Some(export.name.clone()),
            resource: Some(subject.id.clone()),
            constraints,
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Category,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(String::from(category)),
            Some(package_export_summary(export)),
        )],
        unknown_reason: None,
    }
}

fn package_capability_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .capabilities
        .iter()
        .map(|capability| {
            let subject = package_function_subject(&input.package_name, &capability.function);
            let mut constraints = HashMap::new();
            constraints.insert(
                "binding_symbol".to_owned(),
                capability.binding_symbol.clone(),
            );
            if !capability.call_chain.is_empty() {
                constraints.insert("call_chain".to_owned(), capability.call_chain.join(" -> "));
            }
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package_capability.{}.{}.{}",
                    package_slug,
                    normalized_id(&subject.id),
                    normalized_id(&capability.binding_symbol)
                ),
                kind: FactKind::Capability,
                role: Some(FactRole::Required),
                subject,
                capability: Some(Capability {
                    category: capability.category.clone(),
                    provider: capability
                        .provider
                        .clone()
                        .or_else(|| Some("rsscript".to_owned())),
                    service: capability.service.clone(),
                    action: capability.action.clone(),
                    resource: capability.resource.clone(),
                    constraints,
                }),
                value: if capability.unknown_reason.is_some() {
                    FactValue::Unknown
                } else {
                    FactValue::True
                },
                confidence: confidence(
                    if capability.unknown_reason.is_some() {
                        ConfidenceLevel::Unknown
                    } else {
                        ConfidenceLevel::Inferred
                    },
                    PACKAGE_REVIEW_SOURCE,
                ),
                acquisition_mode: AcquisitionMode::BindingManifest,
                precision: if capability.resource.is_some() {
                    Precision::ResourceScoped
                } else {
                    Precision::Category
                },
                evidence: vec![binding_manifest_evidence(input, capability)],
                unknown_reason: capability.unknown_reason.clone(),
            }
        })
        .collect()
}

fn binding_manifest_evidence(
    input: &RsScriptPackageReviewInput,
    capability: &RsScriptPackageCapability,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::BindingManifest,
        file: capability
            .span
            .as_ref()
            .and_then(|span| (!span.file.is_empty()).then(|| span.file.clone())),
        line: capability.span.as_ref().map(|span| span.line.max(1)),
        column: capability.span.as_ref().map(|span| span.column.max(1)),
        length: capability.span.as_ref().map(|span| span.length),
        symbol: Some(capability.function.clone()),
        reason: Some(format!(
            "{} {} propagated through {}",
            if capability.unknown_reason.is_some() {
                "unknown capability binding"
            } else {
                "capability binding"
            },
            capability.binding_symbol,
            if capability.call_chain.is_empty() {
                capability.function.clone()
            } else {
                capability.call_chain.join(" -> ")
            }
        )),
        json_pointer: None,
        resource: capability
            .resource
            .clone()
            .or_else(|| Some(format!("{}@{}", input.package_name, input.version))),
        provider: capability.provider.clone(),
        value: Some(String::from(capability.category.clone())),
        event_id: None,
        time: None,
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: capability.action.clone(),
    }
}

fn native_source_scan_facts(
    input: &RsScriptPackageReviewInput,
    scan: &RsScriptNativeSourceScan,
    package_subject: &Subject,
    package_slug: &str,
) -> Vec<Fact> {
    let mut facts = Vec::new();
    let scan_summary = native_source_scan_summary(scan);
    if scan.unsafe_detected {
        let subject = Subject {
            kind: SubjectKind::UnsafeBoundary,
            id: format!("{}::unsafe::native_rust", input.package_name),
            name: Some("native_rust_unsafe".to_owned()),
            package: Some(input.package_name.clone()),
        };
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.unsafe_boundary.{}", normalized_id(&subject.id)),
            kind: FactKind::UnsafeBoundary,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::Presence,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some("unsafe_detected".to_owned()),
                Some(scan_summary.clone()),
            )],
            unknown_reason: None,
        });
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.runtime_unsafe", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeUnsafe,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.ffi_detected {
        let subject = Subject {
            kind: SubjectKind::NativeBoundary,
            id: format!("{}::native::ffi", input.package_name),
            name: Some("ffi".to_owned()),
            package: Some(input.package_name.clone()),
        };
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.native_boundary.{}", normalized_id(&subject.id)),
            kind: FactKind::NativeBoundary,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::Presence,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some("ffi_detected".to_owned()),
                Some(scan_summary.clone()),
            )],
            unknown_reason: None,
        });
        facts.push(native_scan_capability_fact(
            format!(
                "fact.package.{}.native_scan.runtime_native_ffi",
                package_slug
            ),
            package_subject.clone(),
            CapabilityCategory::RuntimeNative,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.filesystem_detected {
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.filesystem", package_slug),
            package_subject.clone(),
            CapabilityCategory::FilesystemRead,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.network_detected {
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.network", package_slug),
            package_subject.clone(),
            CapabilityCategory::NetworkClient,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.worker_thread_parallelism_detected {
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.process_spawn", package_slug),
            package_subject.clone(),
            CapabilityCategory::ProcessSpawn,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.build_script_present {
        facts.push(native_build_time_execution_fact(
            input,
            package_slug,
            scan_summary.clone(),
        ));
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.build_execute", package_slug),
            package_subject.clone(),
            CapabilityCategory::BuildExecute,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    facts
}

fn native_build_time_execution_fact(
    input: &RsScriptPackageReviewInput,
    package_slug: &str,
    summary: String,
) -> Fact {
    let subject = Subject {
        kind: SubjectKind::BuildStep,
        id: format!(
            "{}@{}::build::native_rust_build_script",
            input.package_name, input.version
        ),
        name: Some("native_rust_build_script".to_owned()),
        package: Some(input.package_name.clone()),
    };
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.package.{}.build_time_execution.build_script",
            package_slug
        ),
        kind: FactKind::BuildTimeExecution,
        role: None,
        subject,
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::SourceScan,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            &input.package_name,
            &input.version,
            Some("build_script_present".to_owned()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn await_site_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .await_sites
        .iter()
        .enumerate()
        .map(|(index, site)| {
            let subject = function_subject(&input.package_name, &site.function);
            let boundary = if site.boundary.is_empty() {
                "unknown"
            } else {
                site.boundary.as_str()
            };
            let summary = await_site_summary(site, boundary);
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.await_site.{}.{}.{}",
                    package_slug,
                    normalized_id(&subject.id),
                    site.line,
                    index
                ),
                kind: FactKind::AsyncBoundary,
                role: None,
                subject,
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![await_site_evidence(site, summary)],
                unknown_reason: (boundary == "unknown")
                    .then(|| "await boundary target could not be classified".to_owned()),
            }
        })
        .collect()
}

fn await_site_summary(site: &RsScriptPackageAwaitSite, boundary: &str) -> String {
    let callee = site.callee.as_deref().unwrap_or("unknown");
    let live = if site.live_across_await.is_empty() {
        "none".to_owned()
    } else {
        site.live_across_await.join(",")
    };
    format!(
        "await boundary={} callee={} live_across_await={}",
        boundary, callee, live
    )
}

fn await_site_evidence(site: &RsScriptPackageAwaitSite, summary: String) -> Evidence {
    Evidence {
        kind: EvidenceKind::SourceSpan,
        file: (!site.file.is_empty()).then(|| site.file.clone()),
        line: Some(site.line.max(1)),
        column: Some(site.column.max(1)),
        length: None,
        symbol: Some(site.function.clone()),
        reason: Some(summary),
        json_pointer: None,
        resource: None,
        provider: None,
        value: None,
        event_id: None,
        time: None,
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn diagnostic_facts(
    input: &RsScriptPackageReviewInput,
    package_subject: &Subject,
    package_slug: &str,
) -> Vec<Fact> {
    input
        .diagnostics
        .iter()
        .enumerate()
        .map(|(index, diagnostic)| {
            let span = diagnostic.spans.first();
            let subject = span
                .filter(|span| !span.file.is_empty())
                .map(|span| Subject {
                    kind: SubjectKind::CodeFile,
                    id: format!("{}::{}", input.package_name, span.file),
                    name: Some(span.file.clone()),
                    package: Some(input.package_name.clone()),
                })
                .unwrap_or_else(|| package_subject.clone());
            let code = if diagnostic.code.is_empty() {
                "unknown"
            } else {
                diagnostic.code.as_str()
            };
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.diagnostic.{}.{}",
                    package_slug,
                    normalized_id(code),
                    index
                ),
                kind: FactKind::Diagnostic,
                role: None,
                subject,
                capability: None,
                value: diagnostic_fact_value(diagnostic),
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![diagnostic_evidence(diagnostic, span)],
                unknown_reason: (diagnostic.severity == "error")
                    .then(|| diagnostic.summary.clone()),
            }
        })
        .collect()
}

fn diagnostic_fact_value(diagnostic: &RsScriptDiagnosticInput) -> FactValue {
    if diagnostic.severity.eq_ignore_ascii_case("error") {
        FactValue::Unknown
    } else {
        FactValue::True
    }
}

fn diagnostic_evidence(
    diagnostic: &RsScriptDiagnosticInput,
    span: Option<&RsScriptDiagnosticSpan>,
) -> Evidence {
    let reason = if diagnostic.summary.is_empty() {
        format!(
            "diagnostic {} severity={}",
            diagnostic.code, diagnostic.severity
        )
    } else {
        format!(
            "diagnostic {} severity={} summary={}",
            diagnostic.code, diagnostic.severity, diagnostic.summary
        )
    };
    Evidence {
        kind: EvidenceKind::SourceSpan,
        file: span.and_then(|span| (!span.file.is_empty()).then(|| span.file.clone())),
        line: span.map(|span| span.line.max(1)),
        column: span.map(|span| span.column.max(1)),
        length: span.map(|span| span.length),
        symbol: Some(diagnostic.code.clone()),
        reason: Some(reason),
        json_pointer: None,
        resource: None,
        provider: None,
        value: Some(diagnostic.severity.clone()),
        event_id: None,
        time: None,
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_feature_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .features
        .iter()
        .map(|feature| {
            let subject = Subject {
                kind: SubjectKind::PackageFeature,
                id: format!(
                    "{}@{}#feature:{}",
                    input.package_name, input.version, feature
                ),
                name: Some(feature.clone()),
                package: Some(input.package_name.clone()),
            };
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.feature.{}",
                    package_slug,
                    normalized_id(feature)
                ),
                kind: FactKind::PackageFeature,
                role: None,
                subject,
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_metadata(
                    &input.package_name,
                    &input.version,
                    Some(feature.clone()),
                    Some(format!("package feature {} selected", feature)),
                )],
                unknown_reason: None,
            }
        })
        .collect()
}

fn provider_implementation_facts(
    input: &RsScriptPackageReviewInput,
    package_slug: &str,
) -> Vec<Fact> {
    input
        .implements
        .iter()
        .map(|implementation| {
            let subject = Subject {
                kind: SubjectKind::Package,
                id: format!(
                    "{}@{}::implements::{}",
                    input.package_name, input.version, implementation.interface_package
                ),
                name: Some(implementation.interface_package.clone()),
                package: Some(input.package_name.clone()),
            };
            let has_hash = implementation.interface_effective_hash.is_some();
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.provider_implementation.{}",
                    package_slug,
                    normalized_id(&implementation.interface_package)
                ),
                kind: FactKind::ProviderImplementation,
                role: None,
                subject,
                capability: None,
                value: if has_hash {
                    FactValue::True
                } else {
                    FactValue::Unknown
                },
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_metadata(
                    &input.package_name,
                    &input.version,
                    implementation.interface_effective_hash.clone(),
                    Some(provider_implementation_summary(implementation)),
                )],
                unknown_reason: (!has_hash).then(|| {
                    "provider implementation is missing interface_effective_hash".to_owned()
                }),
            }
        })
        .collect()
}

fn provider_implementation_summary(implementation: &RsScriptProviderImplementation) -> String {
    let version = implementation.version.as_deref().unwrap_or("unspecified");
    let features = if implementation.interface_features.is_empty() {
        "none".to_owned()
    } else {
        implementation.interface_features.join(",")
    };
    let hash = implementation
        .interface_effective_hash
        .as_deref()
        .unwrap_or("missing");
    format!(
        "implements {} version={} interface_features={} interface_effective_hash={}",
        implementation.interface_package, version, features, hash
    )
}

fn package_check_provider_implementation_facts(
    input: &RsScriptPackageCheckInput,
    package_slug: &str,
) -> Vec<Fact> {
    input
        .implements
        .iter()
        .enumerate()
        .map(|(index, implementation)| {
            let subject = Subject {
                kind: SubjectKind::Package,
                id: format!(
                    "{}@{}::implements::{}",
                    input.package.name, input.package.version, implementation.interface_package
                ),
                name: Some(implementation.interface_package.clone()),
                package: Some(input.package.name.clone()),
            };
            let has_hash = implementation.interface_effective_hash.is_some();
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package_check.{}.provider_implementation.{}",
                    package_slug,
                    normalized_id(&implementation.interface_package)
                ),
                kind: FactKind::ProviderImplementation,
                role: None,
                subject,
                capability: None,
                value: if has_hash {
                    FactValue::True
                } else {
                    FactValue::Unknown
                },
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_CHECK_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_check_evidence(
                    input,
                    implementation.interface_effective_hash.clone(),
                    Some(provider_implementation_summary(implementation)),
                    Some(&format!("/implements/{index}")),
                )],
                unknown_reason: (!has_hash).then(|| {
                    "provider implementation is missing interface_effective_hash".to_owned()
                }),
            }
        })
        .collect()
}

fn native_author_declaration_facts(
    input: &RsScriptPackageReviewInput,
    package_subject: &Subject,
    package_slug: &str,
) -> Vec<Fact> {
    let Some(declaration) = &input.native_author_declaration else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    if declaration.worker_thread_parallelism {
        let summary = native_author_declaration_summary(declaration);
        facts.push(native_author_capability_fact(
            format!("fact.package.{}.native_author.process_spawn", package_slug),
            package_subject.clone(),
            CapabilityCategory::ProcessSpawn,
            &input.package_name,
            &input.version,
            summary,
        ));
    }
    facts
}

fn native_cargo_feature_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .native_cargo_features
        .iter()
        .map(|feature| {
            let subject = Subject {
                kind: SubjectKind::PackageFeature,
                id: format!(
                    "{}@{}#native-cargo-feature:{}",
                    input.package_name, input.version, feature
                ),
                name: Some(feature.clone()),
                package: Some(input.package_name.clone()),
            };
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.native_cargo_feature.{}",
                    package_slug,
                    normalized_id(feature)
                ),
                kind: FactKind::NativeCargoFeature,
                role: None,
                subject,
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_metadata(
                    &input.package_name,
                    &input.version,
                    Some(feature.clone()),
                    Some(format!("selected native Cargo feature {}", feature)),
                )],
                unknown_reason: None,
            }
        })
        .collect()
}

fn native_author_capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: Some("native_rust_author_declaration".to_owned()),
            action: None,
            resource: Some(subject.id.clone()),
            constraints: HashMap::new(),
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Declared, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::ManualDeclaration,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some("author_declaration".to_owned()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn native_author_declaration_summary(declaration: &RsScriptNativeAuthorDeclaration) -> String {
    let backend = declaration
        .native_parallel_backend
        .as_deref()
        .unwrap_or("unspecified");
    let reasons = if declaration.risk_reasons.is_empty() {
        "none".to_owned()
    } else {
        declaration.risk_reasons.join(";")
    };
    format!(
        "author_declaration worker_thread_parallelism={} native_parallel_backend={} risk_reasons={}",
        declaration.worker_thread_parallelism, backend, reasons
    )
}

fn native_scan_capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: Some("native_rust_source_scan".to_owned()),
            action: None,
            resource: Some(subject.id.clone()),
            constraints: HashMap::new(),
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::SourceScan,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some("source_scan_best_effort".to_owned()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn standard_library_export_capabilities(export: &RsScriptPackageExport) -> Vec<CapabilityCategory> {
    if export.kind != "function" {
        return Vec::new();
    }
    let name = export.name.as_str();
    let mut categories = Vec::new();
    match name {
        "Env.get" | "Env.get_or_default" | "Env.current_dir" | "Env.home_dir" | "Env.temp_dir" => {
            categories.push(CapabilityCategory::EnvRead)
        }
        "Env.set" | "Env.set_current_dir" => categories.push(CapabilityCategory::EnvWrite),
        "Http.get" | "Http.post_json" | "Http.post_form" => {
            categories.push(CapabilityCategory::NetworkClient);
        }
        "Process.run_stdout"
        | "Process.run_stdout_timeout"
        | "Process.run_many_stdout"
        | "Process.run_many_stdout_timeout" => {
            categories.push(CapabilityCategory::ProcessSpawn);
        }
        "Args.count" | "Args.get_or_default" => {
            categories.push(CapabilityCategory::ProcessArgs);
        }
        "Clock.now" | "Clock.system_unix_ms" | "Instant.elapsed" => {
            categories.push(CapabilityCategory::TimeRead);
        }
        "Uuid.new_v4" | "Random.int" | "Random.bool" | "Random.float" | "Random.bytes"
        | "Random.string" => {
            categories.push(CapabilityCategory::RandomRead);
        }
        "Hash.sha256_string" | "Hash.sha256_bytes" => {
            categories.push(CapabilityCategory::ComputeHash);
        }
        "Hash.sha256_file" => {
            categories.push(CapabilityCategory::ComputeHash);
            categories.push(CapabilityCategory::FilesystemRead);
        }
        "DbConnection.open" | "DbConnection.try_open" => {
            categories.push(CapabilityCategory::DatabaseRead);
        }
        "DbConnection.query" => {
            categories.push(CapabilityCategory::DatabaseRead);
            categories.push(CapabilityCategory::DatabaseWrite);
        }
        "Regex.compile" | "Regex.is_match" | "Regex.find" | "Regex.captures"
        | "Regex.replace_all" | "Regex.split" => categories.push(CapabilityCategory::ComputeRegex),
        "Log.write" => categories.push(CapabilityCategory::TelemetryEmit),
        "Directory.exists"
        | "Directory.is_file"
        | "Directory.is_dir"
        | "Directory.list_files"
        | "Directory.metadata"
        | "Directory.read_string"
        | "Config.load"
        | "RuleLoader.load_rules"
        | "Csv.open_read"
        | "Csv.read_into"
        | "Image.load"
        | "Json.parse_file"
        | "Toml.parse_file"
        | "Yaml.parse_file"
        | "File.open_read"
        | "File.read_all"
        | "File.read_all_string"
        | "File.read_into"
        | "TempDir.path" => categories.push(CapabilityCategory::FilesystemRead),
        "Directory.create_all"
        | "Directory.remove_file"
        | "Directory.remove_dir_all"
        | "Directory.write_string"
        | "File.open_write"
        | "File.write"
        | "File.write_string"
        | "File.write_buffer"
        | "Image.save"
        | "TempDir.new"
        | "TempDir.new_in"
        | "TempDir.keep" => categories.push(CapabilityCategory::FilesystemWrite),
        "File.open" => {
            categories.push(CapabilityCategory::FilesystemRead);
            categories.push(CapabilityCategory::FilesystemWrite);
        }
        "Directory.copy_file" | "Directory.rename" => {
            categories.push(CapabilityCategory::FilesystemRead);
            categories.push(CapabilityCategory::FilesystemWrite);
        }
        _ => {}
    }
    categories.sort_by_key(|category| String::from(category.clone()));
    categories.dedup();
    categories
}

fn native_source_scan_summary(scan: &RsScriptNativeSourceScan) -> String {
    format!(
        "native_source_scan tool={} graph={} unsafe_detected={} ffi_detected={} filesystem_detected={} network_detected={} build_script_present={} worker_thread_parallelism_detected={}",
        scan.tool,
        scan.selected_graph,
        scan.unsafe_detected,
        scan.ffi_detected,
        scan.filesystem_detected,
        scan.network_detected,
        scan.build_script_present,
        scan.worker_thread_parallelism_detected
    )
}

fn confidence(level: ConfidenceLevel, source: &str) -> Confidence {
    Confidence {
        level,
        source: Some(source.to_owned()),
    }
}

fn source_span(
    file: &str,
    line: usize,
    symbol: &str,
    reason: Option<String>,
    source: &str,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::SourceSpan,
        file: Some(file.to_owned()),
        line: Some(line),
        column: None,
        length: None,
        symbol: Some(symbol.to_owned()),
        reason,
        json_pointer: None,
        resource: None,
        provider: None,
        value: None,
        event_id: None,
        time: None,
        source: Some(source.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_metadata(
    package_name: &str,
    version: &str,
    value: Option<String>,
    reason: Option<String>,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::PackageMetadata,
        file: None,
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: None,
        resource: Some(format!("{package_name}@{version}")),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn lockfile_supply_chain_fact(
    package_slug: &str,
    field: &str,
    json_field: &str,
    subject: &Subject,
    package: &RsScriptPackageLockPackage,
    package_index: usize,
    hash: &str,
    lockfile_path: &str,
    reason: String,
) -> Fact {
    let missing_hash = hash.is_empty();
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.lockfile.{}.{}", package_slug, field),
        kind: FactKind::SupplyChain,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if missing_hash {
            FactValue::Unknown
        } else {
            FactValue::True
        },
        confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
        acquisition_mode: AcquisitionMode::Lockfile,
        precision: Precision::Exact,
        evidence: vec![lockfile_entry(
            package,
            if missing_hash {
                None
            } else {
                Some(hash.to_owned())
            },
            reason,
            format!("/package/{package_index}/{json_field}"),
            lockfile_path,
        )],
        unknown_reason: missing_hash.then(|| format!("lockfile `{json_field}` is missing")),
    }
}

fn lockfile_entry(
    package: &RsScriptPackageLockPackage,
    value: Option<String>,
    reason: String,
    json_pointer: String,
    lockfile_path: &str,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::LockfileEntry,
        file: Some(lockfile_path.to_owned()),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason: Some(reason),
        json_pointer: Some(json_pointer),
        resource: Some(format!("{}@{}", package.name, package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(LOCKFILE_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn lockfile_features_summary(features: &[String]) -> String {
    if features.is_empty() {
        "default".to_owned()
    } else {
        features.join(",")
    }
}

fn lock_diff_subject(input: &RsScriptPackageLockDiffInput) -> Subject {
    Subject {
        kind: SubjectKind::DependencyEdge,
        id: format!(
            "lock-update:{}->{}",
            input.old_lock_path, input.new_lock_path
        ),
        name: Some("rsspkg.lock update".to_owned()),
        package: None,
    }
}

fn lock_diff_package_subject(package: &RsScriptPackageLockPackageChange) -> Subject {
    let version = package
        .after_version
        .as_ref()
        .or(package.before_version.as_ref())
        .map(String::as_str)
        .unwrap_or("unresolved");
    Subject {
        kind: SubjectKind::Package,
        id: format!("{}@{}", package.name, version),
        name: Some(package.name.clone()),
        package: Some(package.name.clone()),
    }
}

fn lock_diff_field_fact_kind(field: &str) -> FactKind {
    match field {
        "checksum" | "interface_hash" | "review_hash" | "native_hash" | "features" => {
            FactKind::SupplyChain
        }
        "package" | "version" | "source" => FactKind::DependencyRisk,
        _ => FactKind::PolicyResult,
    }
}

fn lock_diff_summary(input: &RsScriptPackageLockDiffInput) -> String {
    format!(
        "lock update old={} new={} old_packages={} new_packages={} changed_packages={} reasons={}",
        input.old_lock_path,
        input.new_lock_path,
        input.old_packages,
        input.new_packages,
        input.package_changes.len(),
        if input.reasons.is_empty() {
            "none".to_owned()
        } else {
            input.reasons.join("; ")
        }
    )
}

fn lock_diff_package_summary(package: &RsScriptPackageLockPackageChange) -> String {
    format!(
        "package {} version {} -> {} risk={} changes={}",
        package.name,
        package.before_version.as_deref().unwrap_or("<none>"),
        package.after_version.as_deref().unwrap_or("<none>"),
        package.risk.as_str(),
        package.changes.len()
    )
}

fn lock_diff_field_summary(field: &RsScriptPackageLockFieldChange) -> String {
    format!(
        "field {} {} -> {} risk={}",
        field.field,
        field.before.as_deref().unwrap_or("<none>"),
        field.after.as_deref().unwrap_or("<none>"),
        field.risk.as_str()
    )
}

fn lock_diff_package_evidence_file(
    input: &RsScriptPackageLockDiffInput,
    package: &RsScriptPackageLockPackageChange,
) -> String {
    if package.after_version.is_none() {
        input.old_lock_path.clone()
    } else {
        input.new_lock_path.clone()
    }
}

fn lock_diff_field_evidence_file(
    input: &RsScriptPackageLockDiffInput,
    field: &RsScriptPackageLockFieldChange,
) -> String {
    if field.after.is_none() {
        input.old_lock_path.clone()
    } else {
        input.new_lock_path.clone()
    }
}

fn lock_diff_evidence(
    input: &RsScriptPackageLockDiffInput,
    file: String,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: &str,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::LockfileEntry,
        file: Some(file),
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: Some(json_pointer.to_owned()),
        resource: Some(format!(
            "{} -> {}",
            input.old_lock_path, input.new_lock_path
        )),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(LOCKFILE_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_review_summary(input: &RsScriptPackageReviewInput) -> String {
    format!(
        "public_apis={}, mutating_apis={}, retaining_apis={}, resource_apis={}, native_apis={}, unsafe_apis={}, unknown_apis={}, native_boundaries={}",
        input.public_apis,
        input.mutating_apis,
        input.retaining_apis,
        input.resource_apis,
        input.native_apis,
        input.unsafe_apis,
        input.unknown_apis,
        input.native_boundaries.len()
    )
}

fn package_check_summary(input: &RsScriptPackageCheckInput) -> String {
    format!(
        "package check ok={} risk={} diagnostics={} errors={} dependencies={} native_apis={} unsafe_apis={} unknown_apis={}",
        input.ok,
        input.risk.as_str(),
        input.summary.diagnostics,
        input.summary.errors,
        input.summary.dependencies,
        input.summary.native_apis,
        input.summary.unsafe_apis,
        input.summary.unknown_apis
    )
}

fn check_reasons_summary(label: &str, reasons: &[String]) -> String {
    if reasons.is_empty() {
        format!("{label} check ok")
    } else {
        format!("{label} check reasons={}", reasons.join("; "))
    }
}

fn package_check_native_summary(native: &RsScriptPackageNativeRustCheckInput) -> String {
    format!(
        "native check ok={} risk={} path={} cargo_toml={} cargo_metadata={} unsafe={} links={} build_env={} build_download={} files={} reasons={}",
        native.ok,
        native.risk.as_str(),
        native.path,
        native.cargo_toml_present,
        native.cargo_metadata_ok,
        native.unsafe_detected,
        if native.linked_libraries.is_empty() {
            "none".to_owned()
        } else {
            native.linked_libraries.join(",")
        },
        native.build_env_detected,
        native.build_download_detected,
        native.file_count,
        if native.reasons.is_empty() {
            "none".to_owned()
        } else {
            native.reasons.join("; ")
        }
    )
}

fn package_metadata_report_summary(input: &RsScriptPackageMetadataInput) -> String {
    format!(
        "package metadata {} package={} version={} risk={} mismatches={}",
        if input.verified {
            "verify"
        } else if input.written {
            "write"
        } else if input.dry_run {
            "dry-run"
        } else {
            "report"
        },
        input.package.name,
        input.package.version,
        input.risk.as_str(),
        input.mismatches.len()
    )
}

fn package_vendor_summary(input: &RsScriptPackageVendorInput) -> String {
    format!(
        "package vendor {} package={} version={} risk={} entries={} unresolved={}",
        if input.dry_run { "dry-run" } else { "write" },
        input.package.name,
        input.package.version,
        input.risk.as_str(),
        input.entries.len(),
        input.unresolved.len()
    )
}

fn package_export_summary(export: &RsScriptPackageExport) -> String {
    let mut parts = vec![
        format!("kind={}", export.kind),
        format!("classification={}", export.classification),
    ];
    if !export.reasons.is_empty() {
        parts.push(format!("reasons={}", export.reasons.join("; ")));
    }
    if !export.normalized_effects.is_empty() {
        parts.push(format!("effects={}", export.normalized_effects.join(", ")));
    }
    parts.join(", ")
}

fn native_boundary_reason(boundary: &RsScriptNativeBoundary) -> String {
    if boundary.functions.is_empty() {
        format!("native boundary in module {}", boundary.module_name)
    } else {
        format!(
            "native boundary in module {} for functions {}",
            boundary.module_name,
            boundary.functions.join(", ")
        )
    }
}

fn native_module_declaration_reason(boundary: &RsScriptNativeBoundary) -> String {
    if boundary.functions.is_empty() {
        format!("native module {} declared", boundary.module_name)
    } else {
        format!(
            "native module {} declares functions {}",
            boundary.module_name,
            boundary.functions.join(", ")
        )
    }
}

fn joined_reason(reasons: &[String]) -> Option<String> {
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

fn normalized_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn finalize_bundle(bundle: &mut Bundle) {
    index_bundle_subjects(bundle);
    bundle.slices = slice_by_kind(bundle);
}

fn index_bundle_subjects(bundle: &mut Bundle) {
    let mut subjects = BTreeMap::<String, Subject>::new();
    for subject in &bundle.subjects {
        subjects.insert(subject.id.clone(), subject.clone());
    }
    for fact in &bundle.facts {
        subjects.insert(fact.subject.id.clone(), fact.subject.clone());
    }
    for edge in &bundle.edges {
        subjects.insert(edge.from.id.clone(), edge.from.clone());
        subjects.insert(edge.to.id.clone(), edge.to.clone());
    }
    for chain in &bundle.subject_chains {
        for subject in &chain.nodes {
            subjects.insert(subject.id.clone(), subject.clone());
        }
    }
    bundle.subjects = subjects.into_values().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_review_map() -> RsScriptReviewMapInput {
        RsScriptReviewMapInput {
            package_name: "demo_pkg".to_owned(),
            modules: vec![RsScriptModuleInput {
                file: "src/package/review.rss".to_owned(),
                module_path: "rss.package.review".to_owned(),
                line: 1,
                uses: vec![
                    RsScriptUseInput {
                        path: "rss.package.contract.PackageContract".to_owned(),
                        line: 3,
                    },
                    RsScriptUseInput {
                        path: "rss.review.ReviewMap".to_owned(),
                        line: 4,
                    },
                ],
            }],
            regions: vec![
                RsScriptRegionInput {
                    file: "src/lib.rs".to_owned(),
                    function_name: "foldable_fn".to_owned(),
                    classification: RsScriptClassification::Foldable,
                    line: 10,
                    reasons: vec!["pure".to_owned()],
                },
                RsScriptRegionInput {
                    file: "src/lib.rs".to_owned(),
                    function_name: "native_fn".to_owned(),
                    classification: RsScriptClassification::ReviewRequired,
                    line: 22,
                    reasons: vec!["native bridge".to_owned()],
                },
                RsScriptRegionInput {
                    file: "src/lib.rs".to_owned(),
                    function_name: "opaque_fn".to_owned(),
                    classification: RsScriptClassification::Unknown,
                    line: 31,
                    reasons: vec!["macro expansion".to_owned()],
                },
            ],
        }
    }

    fn sample_package_review() -> RsScriptPackageReviewInput {
        RsScriptPackageReviewInput {
            package_name: "demo_pkg".to_owned(),
            version: "1.2.3".to_owned(),
            risk: RsScriptPackageRisk::High,
            features: Vec::new(),
            implements: Vec::new(),
            dependencies: vec![RsScriptPackageDependency {
                name: "rss_core".to_owned(),
                requirement: Some("0.5".to_owned()),
                source: "registry".to_owned(),
                features: vec!["json".to_owned()],
                dependency_kind: "normal".to_owned(),
                compile_only: false,
                test_only: false,
                platform_provided: false,
            }],
            exports: vec![RsScriptPackageExport {
                name: "PackageError".to_owned(),
                kind: "sum_type".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public sum type".to_owned(), "variants: 2".to_owned()],
                normalized_effects: Vec::new(),
            }],
            capabilities: Vec::new(),
            await_sites: Vec::new(),
            diagnostics: Vec::new(),
            public_apis: 8,
            mutating_apis: 2,
            retaining_apis: 1,
            resource_apis: 3,
            native_apis: 2,
            unsafe_apis: 1,
            unknown_apis: 0,
            native_boundaries: vec![RsScriptNativeBoundary {
                module_name: "ffi.crypto".to_owned(),
                functions: vec!["native_fn".to_owned()],
                file: "src/ffi.rs".to_owned(),
                line: 44,
            }],
            native_cargo_features: Vec::new(),
            native_author_declaration: None,
            native_source_scan: None,
        }
    }

    #[test]
    fn review_map_to_facts_skips_foldable_regions() {
        let facts = review_map_to_facts(&sample_review_map());

        assert_eq!(facts.len(), 5);
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::ModuleDeclaration
                && fact.subject.kind == SubjectKind::CodeModule
                && fact.subject.id == "demo_pkg::module::rss.package.review"
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.subject.id == "demo_pkg::module::rss.package.review"
                && fact.evidence[0].symbol.as_deref()
                    == Some("rss.package.contract.PackageContract")
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.subject.id == "demo_pkg::module::rss.package.review"
                && fact.evidence[0].symbol.as_deref() == Some("rss.review.ReviewMap")
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native_fn"
                && fact.evidence[0].file.as_deref() == Some("src/lib.rs")
                && fact.evidence[0].line == Some(22)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::Unknown
                && fact.value == FactValue::Unknown
                && fact.confidence.level == ConfidenceLevel::Unknown
                && fact.subject.id == "demo_pkg::opaque_fn"
        }));
    }

    #[test]
    fn package_review_to_facts_emits_risk_boundary_and_capabilities() {
        let facts = package_review_to_facts(&sample_package_review());

        assert_eq!(facts.len(), 7);
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::PackageRisk
                && fact.subject.kind == SubjectKind::Package
                && fact.subject.id == "demo_pkg@1.2.3"
                && fact.evidence[0].value.as_deref() == Some("high")
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.role == Some(FactRole::Required)
                && fact
                    .capability
                    .as_ref()
                    .map(|capability| capability.category == CapabilityCategory::RuntimeNative)
                    .unwrap_or(false)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.role == Some(FactRole::Required)
                && fact
                    .capability
                    .as_ref()
                    .map(|capability| capability.category == CapabilityCategory::RuntimeUnsafe)
                    .unwrap_or(false)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.kind == SubjectKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native::ffi.crypto"
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::NativeModuleDeclaration
                && fact.subject.kind == SubjectKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native::ffi.crypto"
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.kind == SubjectKind::Package
                && fact.subject.id == "rss_core@0.5"
                && fact.value == FactValue::Unknown
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::PublicContract
                && fact.subject.kind == SubjectKind::CodeType
                && fact.subject.id == "demo_pkg::public::sum_type::PackageError"
                && fact.evidence[0].value.as_deref() == Some("sum_type")
        }));
    }

    #[test]
    fn package_review_to_facts_maps_stdlib_facades_to_capabilities() {
        let mut review = sample_package_review();
        review.exports = vec![
            RsScriptPackageExport {
                name: "Env.get".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Directory.write_string".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Http.get".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Process.run_stdout".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Args.get_or_default".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Hash.sha256_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Config.load".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Json.parse_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Toml.parse_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Yaml.parse_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.open".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.read_all_string".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.read_to_string".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.write_buffer".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "DbConnection.query".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Image.load".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Image.save".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Csv.open_read".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Random.bytes".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Uuid.new_v4".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
        ];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let categories = bundle
            .facts
            .iter()
            .filter_map(|fact| fact.capability.as_ref())
            .map(|capability| capability.category.clone())
            .collect::<Vec<_>>();

        assert!(categories.contains(&CapabilityCategory::EnvRead));
        assert!(categories.contains(&CapabilityCategory::FilesystemWrite));
        assert!(categories.contains(&CapabilityCategory::FilesystemRead));
        assert!(categories.contains(&CapabilityCategory::NetworkClient));
        assert!(categories.contains(&CapabilityCategory::ProcessArgs));
        assert!(categories.contains(&CapabilityCategory::ProcessSpawn));
        assert!(categories.contains(&CapabilityCategory::ComputeHash));
        assert!(categories.contains(&CapabilityCategory::RandomRead));
        assert!(categories.contains(&CapabilityCategory::DatabaseRead));
        assert!(categories.contains(&CapabilityCategory::DatabaseWrite));
        assert!(!bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.subject.id == "demo_pkg::public::function::File.read_to_string"
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::EnvSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::FilesystemSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NetworkSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::ProcessSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::RandomnessSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::DatabaseSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_native_source_scan_boundaries() {
        let mut review = sample_package_review();
        review.native_source_scan = Some(RsScriptNativeSourceScan {
            tool: "rss-native-source-scan".to_owned(),
            selected_graph: "package-native-rust".to_owned(),
            worker_thread_parallelism_detected: true,
            unsafe_detected: true,
            ffi_detected: true,
            filesystem_detected: true,
            network_detected: true,
            build_script_present: true,
        });

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let categories = bundle
            .facts
            .iter()
            .filter_map(|fact| fact.capability.as_ref())
            .map(|capability| capability.category.clone())
            .collect::<Vec<_>>();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::UnsafeBoundary
                && fact.subject.kind == SubjectKind::UnsafeBoundary
                && fact.confidence.level == ConfidenceLevel::Scanned
                && fact.acquisition_mode == AcquisitionMode::SourceScan
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native::ffi"
                && fact.confidence.level == ConfidenceLevel::Scanned
        }));
        assert!(categories.contains(&CapabilityCategory::RuntimeUnsafe));
        assert!(categories.contains(&CapabilityCategory::RuntimeNative));
        assert!(categories.contains(&CapabilityCategory::FilesystemRead));
        assert!(categories.contains(&CapabilityCategory::NetworkClient));
        assert!(categories.contains(&CapabilityCategory::ProcessSpawn));
        assert!(categories.contains(&CapabilityCategory::BuildExecute));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::BuildTimeExecution
                && fact.subject.kind == SubjectKind::BuildStep
                && fact.subject.id == "demo_pkg@1.2.3::build::native_rust_build_script"
                && fact.confidence.level == ConfidenceLevel::Scanned
                && fact.acquisition_mode == AcquisitionMode::SourceScan
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NativeUnsafeSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::FilesystemSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NetworkSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::ProcessSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::BuildTimeSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_await_sites_as_async_boundaries() {
        let mut review = sample_package_review();
        review.await_sites = vec![RsScriptPackageAwaitSite {
            function: "Api.run".to_owned(),
            callee: Some("Timer.sleep".to_owned()),
            boundary: "native_pending".to_owned(),
            live_across_await: vec!["client".to_owned()],
            file: "src/api.rss".to_owned(),
            line: 12,
            column: 9,
        }];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let fact = bundle
            .facts
            .iter()
            .find(|fact| fact.kind == FactKind::AsyncBoundary)
            .expect("async boundary fact");

        assert_eq!(fact.subject.id, "demo_pkg::Api.run");
        assert_eq!(fact.confidence.level, ConfidenceLevel::Authoritative);
        assert_eq!(fact.acquisition_mode, AcquisitionMode::CompilerContract);
        assert_eq!(fact.evidence[0].file.as_deref(), Some("src/api.rss"));
        assert_eq!(fact.evidence[0].line, Some(12));
        assert_eq!(fact.evidence[0].column, Some(9));
        assert!(
            fact.evidence[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("boundary=native_pending"))
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::AsyncSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_diagnostics() {
        let mut review = sample_package_review();
        review.diagnostics = vec![RsScriptDiagnosticInput {
            code: "RS0015".to_owned(),
            severity: "error".to_owned(),
            summary: "unsupported syntax".to_owned(),
            spans: vec![RsScriptDiagnosticSpan {
                file: "interface/lib.rssi".to_owned(),
                line: 1,
                column: 4,
                length: 2,
                label: "unsupported".to_owned(),
            }],
        }];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let fact = bundle
            .facts
            .iter()
            .find(|fact| fact.kind == FactKind::Diagnostic)
            .expect("diagnostic fact");

        assert_eq!(fact.subject.kind, SubjectKind::CodeFile);
        assert_eq!(fact.subject.id, "demo_pkg::interface/lib.rssi");
        assert_eq!(fact.value, FactValue::Unknown);
        assert_eq!(fact.evidence[0].symbol.as_deref(), Some("RS0015"));
        assert_eq!(fact.evidence[0].file.as_deref(), Some("interface/lib.rssi"));
        assert_eq!(fact.evidence[0].line, Some(1));
        assert_eq!(fact.evidence[0].column, Some(4));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::DiagnosticSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_features_and_provider_implementations() {
        let mut review = sample_package_review();
        review.features = vec!["native-tls".to_owned()];
        review.implements = vec![RsScriptProviderImplementation {
            interface_package: "platform-env".to_owned(),
            version: Some("0.1".to_owned()),
            interface_features: vec!["posix".to_owned()],
            interface_effective_hash: Some("sha256:abc".to_owned()),
        }];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PackageFeature
                && fact.subject.kind == SubjectKind::PackageFeature
                && fact.subject.id == "demo_pkg@1.2.3#feature:native-tls"
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::ProviderImplementation
                && fact.subject.id == "demo_pkg@1.2.3::implements::platform-env"
                && fact.value == FactValue::True
                && fact.evidence[0]
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("interface_effective_hash=sha256:abc"))
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::PackageFeatureSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::ProviderImplementationSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_native_author_and_cargo_feature_metadata() {
        let mut review = sample_package_review();
        review.native_cargo_features = vec!["base-native".to_owned()];
        review.native_author_declaration = Some(RsScriptNativeAuthorDeclaration {
            worker_thread_parallelism: true,
            native_parallel_backend: Some("rayon".to_owned()),
            risk_reasons: vec!["native API declares parallel worker execution".to_owned()],
        });

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let author_fact = bundle
            .facts
            .iter()
            .find(|fact| {
                fact.capability.as_ref().is_some_and(|capability| {
                    capability.service.as_deref() == Some("native_rust_author_declaration")
                })
            })
            .expect("native author declaration capability fact");

        assert_eq!(author_fact.confidence.level, ConfidenceLevel::Declared);
        assert_eq!(
            author_fact.acquisition_mode,
            AcquisitionMode::ManualDeclaration
        );
        assert_eq!(
            author_fact
                .capability
                .as_ref()
                .map(|capability| capability.category.clone()),
            Some(CapabilityCategory::ProcessSpawn)
        );
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeCargoFeature
                && fact.subject.id == "demo_pkg@1.2.3#native-cargo-feature:base-native"
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NativeUnsafeSlice)
        );
    }

    #[test]
    fn package_lock_to_facts_emits_lockfile_supply_chain_hashes() {
        let lock = RsScriptPackageLockInput {
            version: 1,
            lockfile_path: Some("/tmp/demo/rsspkg.lock".to_owned()),
            packages: vec![RsScriptPackageLockPackage {
                name: "demo_pkg".to_owned(),
                version: "1.2.3".to_owned(),
                source: "path+/tmp/demo".to_owned(),
                checksum: "sha256:pkg".to_owned(),
                interface_hash: "sha256:iface".to_owned(),
                review_hash: "sha256:review".to_owned(),
                native_hash: Some("sha256:native".to_owned()),
                features: vec!["fast".to_owned()],
            }],
        };

        let bundle = {
            let json = serde_json::to_string(&lock).unwrap();
            rsscript_lock_json_to_bundle(&json).unwrap()
        };

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.lockfile.demo_pkg_1_2_3.effective_interface_hash"
                && fact.acquisition_mode == AcquisitionMode::Lockfile
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package/0/interface_hash")
                && fact.evidence[0].value.as_deref() == Some("sha256:iface")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.lockfile.demo_pkg_1_2_3.native_hash"
                && fact.evidence[0].value.as_deref() == Some("sha256:native")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.lockfile.demo_pkg_1_2_3.review_hash".to_owned())
        }));
    }

    #[test]
    fn package_lock_to_facts_marks_missing_hashes_unknown() {
        let lock = RsScriptPackageLockInput {
            version: 1,
            lockfile_path: Some("/tmp/demo/rsspkg.lock".to_owned()),
            packages: vec![RsScriptPackageLockPackage {
                name: "demo_pkg".to_owned(),
                version: "1.2.3".to_owned(),
                source: "path+/tmp/demo".to_owned(),
                checksum: String::new(),
                interface_hash: String::new(),
                review_hash: String::new(),
                native_hash: Some(String::new()),
                features: Vec::new(),
            }],
        };

        let bundle = {
            let json = serde_json::to_string(&lock).unwrap();
            rsscript_lock_json_to_bundle(&json).unwrap()
        };

        for id in [
            "fact.lockfile.demo_pkg_1_2_3.checksum",
            "fact.lockfile.demo_pkg_1_2_3.effective_interface_hash",
            "fact.lockfile.demo_pkg_1_2_3.review_hash",
            "fact.lockfile.demo_pkg_1_2_3.native_hash",
        ] {
            let fact = bundle
                .facts
                .iter()
                .find(|fact| fact.id == id)
                .unwrap_or_else(|| panic!("missing lockfile fact `{id}`"));
            assert_eq!(fact.kind, FactKind::SupplyChain);
            assert_eq!(fact.value, FactValue::Unknown);
            assert!(fact.evidence[0].value.is_none());
            assert!(
                fact.unknown_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("is missing"))
            );
        }
    }

    #[test]
    fn package_check_to_bundle_emits_gate_policy_facts() {
        let check = RsScriptPackageCheckInput {
            package: RsScriptPackageIdentityInput {
                name: "demo".to_owned(),
                version: "0.1.0".to_owned(),
                edition: "2026".to_owned(),
            },
            package_dir: "/tmp/demo".to_owned(),
            ok: false,
            risk: RsScriptPackageRisk::High,
            reasons: vec!["rsspkg.lock missing".to_owned()],
            summary: RsScriptPackageCheckSummary {
                diagnostics: 1,
                errors: 1,
                dependencies: 1,
                native_apis: 1,
                unsafe_apis: 1,
                unknown_apis: 0,
            },
            graph: RsScriptPackageGraphCheckInput {
                ok: true,
                risk: RsScriptPackageRisk::Low,
                reasons: Vec::new(),
            },
            lock: RsScriptPackageCheckLockInput {
                path: "/tmp/demo/rsspkg.lock".to_owned(),
                present: false,
                matches: false,
                risk: RsScriptPackageRisk::Elevated,
                reasons: vec!["rsspkg.lock missing".to_owned()],
                package_changes: Vec::new(),
            },
            implements: vec![RsScriptProviderImplementation {
                interface_package: "platform-env".to_owned(),
                version: Some("0.1".to_owned()),
                interface_features: vec!["posix".to_owned()],
                interface_effective_hash: None,
            }],
            native_rust: Some(RsScriptPackageNativeRustCheckInput {
                path: "/tmp/demo/native".to_owned(),
                cargo_toml_present: true,
                cargo_metadata_ok: true,
                cargo_package_name: Some("demo-native".to_owned()),
                target_kinds: vec!["lib".to_owned()],
                unsafe_detected: true,
                linked_libraries: Vec::new(),
                build_env_detected: true,
                build_download_detected: false,
                file_count: 2,
                ok: false,
                risk: RsScriptPackageRisk::High,
                reasons: vec!["native Rust unsafe usage".to_owned()],
            }),
            diagnostics: vec![RsScriptDiagnosticInput {
                code: "PKG0601".to_owned(),
                severity: "error".to_owned(),
                summary: "native policy violation".to_owned(),
                spans: vec![RsScriptDiagnosticSpan {
                    file: "rsspkg.toml".to_owned(),
                    line: 3,
                    column: 1,
                    length: 4,
                    label: "native".to_owned(),
                }],
            }],
        };

        let json = serde_json::to_string(&check).unwrap();
        let bundle = rsscript_check_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.status"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo")
                && fact.evidence[0].json_pointer.as_deref() == Some("/ok")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.graph"
                && fact.acquisition_mode == AcquisitionMode::PackageMetadata
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo")
                && fact.evidence[0].json_pointer.as_deref() == Some("/graph")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.lock"
                && fact.acquisition_mode == AcquisitionMode::Lockfile
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/lock")
                && fact.evidence[0].value.as_deref() == Some("elevated")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.native"
                && fact.acquisition_mode == AcquisitionMode::PackageMetadata
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/native")
                && fact.evidence[0].json_pointer.as_deref() == Some("/native_rust")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::UnsafeBoundary
                && fact.id == "fact.package_check.demo_0_1_0.unsafe"
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/native")
                && fact.evidence[0].json_pointer.as_deref() == Some("/native_rust/unsafe_detected")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::BuildTimeExecution
                && fact.id == "fact.package_check.demo_0_1_0.build_time"
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/native")
                && fact.evidence[0].json_pointer.as_deref() == Some("/native_rust")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Diagnostic
                && fact.id.contains("PKG0601")
                && fact.evidence[0].source.as_deref() == Some(PACKAGE_CHECK_SOURCE)
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.toml")
                && fact.evidence[0].line == Some(3)
                && fact.evidence[0].column == Some(1)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::ProviderImplementation
                && fact.id == "fact.package_check.demo_0_1_0.provider_implementation.platform_env"
                && fact.subject.id == "demo@0.1.0::implements::platform-env"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].source.as_deref() == Some(PACKAGE_CHECK_SOURCE)
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.toml")
                && fact.evidence[0].json_pointer.as_deref() == Some("/implements/0")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::DiagnosticSlice
                && slice.facts.iter().any(|fact| fact.contains("PKG0601"))
        }));
    }

    #[test]
    fn package_lock_diff_to_bundle_emits_update_review_facts() {
        let diff = RsScriptPackageLockDiffInput {
            old_lock_path: "old.rsspkg.lock".to_owned(),
            new_lock_path: "new.rsspkg.lock".to_owned(),
            risk: RsScriptPackageRisk::High,
            reasons: vec![".rssi interface hash changed".to_owned()],
            old_packages: 1,
            new_packages: 1,
            package_changes: vec![
                RsScriptPackageLockPackageChange {
                    name: "dep".to_owned(),
                    before_version: Some("0.1.0".to_owned()),
                    after_version: Some("0.2.0".to_owned()),
                    risk: RsScriptPackageRisk::High,
                    changes: vec![RsScriptPackageLockFieldChange {
                        field: "interface_hash".to_owned(),
                        before: Some("sha256:old".to_owned()),
                        after: Some("sha256:new".to_owned()),
                        risk: RsScriptPackageRisk::High,
                    }],
                },
                RsScriptPackageLockPackageChange {
                    name: "old-dep".to_owned(),
                    before_version: Some("0.1.0".to_owned()),
                    after_version: None,
                    risk: RsScriptPackageRisk::Elevated,
                    changes: vec![RsScriptPackageLockFieldChange {
                        field: "package".to_owned(),
                        before: Some("removed".to_owned()),
                        after: None,
                        risk: RsScriptPackageRisk::Elevated,
                    }],
                },
            ],
        };

        let json = serde_json::to_string(&diff).unwrap();
        let bundle = rsscript_lock_diff_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id.contains("field.interface_hash")
                && fact.subject.id == "dep@0.2.0"
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("new.rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package_changes/0/changes/0")
                && fact.evidence[0].value.as_deref() == Some("sha256:new")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.id.contains("old_dep")
                && fact.subject.id == "old-dep@0.1.0"
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("old.rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package_changes/1")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.id.contains("field.package")
                && fact.subject.id == "old-dep@0.1.0"
                && fact.evidence[0].file.as_deref() == Some("old.rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package_changes/1/changes/0")
                && fact.evidence[0].value.as_deref() == Some("removed")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id.contains(".risk")
                && fact.evidence[0].json_pointer.as_deref() == Some("/risk")
                && fact.evidence[0].resource.as_deref()
                    == Some("old.rsspkg.lock -> new.rsspkg.lock")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .iter()
                    .any(|fact| fact.contains("interface_hash"))
        }));
    }

    #[test]
    fn package_tree_to_bundle_emits_dependency_path_facts_and_edges() {
        let tree = RsScriptPackageTreeInput {
            root: RsScriptPackageTreeNode {
                name: "app".to_owned(),
                version: Some("0.1.0".to_owned()),
                requirement: None,
                source: "path+/tmp/app".to_owned(),
                risk: RsScriptPackageRisk::Low,
                features: Vec::new(),
                native: false,
                interface_only: false,
                compile_only: false,
                test_only: false,
                platform_provided: false,
                interface_effective_hash: "sha256:app".to_owned(),
                implements: Vec::new(),
                dependency_kind: "root".to_owned(),
                reasons: Vec::new(),
                dependencies: vec![
                    RsScriptPackageTreeNode {
                        name: "dep".to_owned(),
                        version: Some("1.0.0".to_owned()),
                        requirement: Some("^1".to_owned()),
                        source: "path+/tmp/dep".to_owned(),
                        risk: RsScriptPackageRisk::Elevated,
                        features: vec!["fast".to_owned()],
                        native: true,
                        interface_only: false,
                        compile_only: false,
                        test_only: false,
                        platform_provided: false,
                        interface_effective_hash: "sha256:dep".to_owned(),
                        implements: Vec::new(),
                        dependency_kind: "normal".to_owned(),
                        reasons: vec!["native API".to_owned()],
                        dependencies: Vec::new(),
                    },
                    RsScriptPackageTreeNode {
                        name: "registry-dep".to_owned(),
                        version: None,
                        requirement: Some("^2".to_owned()),
                        source: "registry".to_owned(),
                        risk: RsScriptPackageRisk::Unknown,
                        features: Vec::new(),
                        native: false,
                        interface_only: false,
                        compile_only: false,
                        test_only: false,
                        platform_provided: false,
                        interface_effective_hash: String::new(),
                        implements: Vec::new(),
                        dependency_kind: "normal".to_owned(),
                        reasons: vec!["dependency resolver not implemented".to_owned()],
                        dependencies: Vec::new(),
                    },
                    RsScriptPackageTreeNode {
                        name: "missing-path-dep".to_owned(),
                        version: None,
                        requirement: None,
                        source: "path+/tmp/missing-path-dep".to_owned(),
                        risk: RsScriptPackageRisk::Unknown,
                        features: Vec::new(),
                        native: false,
                        interface_only: false,
                        compile_only: false,
                        test_only: false,
                        platform_provided: false,
                        interface_effective_hash: String::new(),
                        implements: Vec::new(),
                        dependency_kind: "normal".to_owned(),
                        reasons: vec!["path dependency manifest missing".to_owned()],
                        dependencies: Vec::new(),
                    },
                ],
            },
            summary: RsScriptPackageTreeSummary::default(),
        };

        let json = serde_json::to_string(&tree).unwrap();
        let bundle = rsscript_tree_json_to_bundle(&json).unwrap();

        assert!(bundle.producers.iter().any(|producer| {
            producer.adapter.as_deref() == Some("rsscript-package-tree")
                && producer.source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "dep@1.0.0"
                && fact.confidence.source.as_deref() == Some(TREE_SOURCE)
                && fact.evidence[0].kind == EvidenceKind::DependencyPath
                && fact.evidence[0].file.as_deref() == Some("/tmp/dep")
                && fact.evidence[0].json_pointer.as_deref() == Some("/root/dependencies/0")
                && fact.evidence[0].source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "registry-dep@^2"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.is_none()
                && fact.evidence[0].json_pointer.as_deref() == Some("/root/dependencies/1")
                && fact.unknown_reason.as_deref() == Some("dependency resolver not implemented")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "missing-path-dep@path+/tmp/missing-path-dep"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.is_none()
                && fact.evidence[0].json_pointer.as_deref() == Some("/root/dependencies/2")
                && fact.unknown_reason.as_deref() == Some("path dependency manifest missing")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.package_tree.root_1.effective_interface_hash"
                && fact.evidence[0].file.as_deref() == Some("/tmp/dep")
                && fact.evidence[0].value.as_deref() == Some("sha256:dep")
                && fact.evidence[0].source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.edges.iter().any(|edge| {
            edge.kind == EdgeKind::DependsOn
                && edge.from.id == "app@0.1.0"
                && edge.to.id == "dep@1.0.0"
                && edge.confidence.source.as_deref() == Some(TREE_SOURCE)
                && edge.evidence[0].kind == EvidenceKind::DependencyPath
                && edge.evidence[0].file.as_deref() == Some("/tmp/dep")
                && edge.evidence[0].source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.package_tree.root_1.risk".to_owned())
        }));
    }

    #[test]
    fn package_publish_to_bundle_emits_registry_supply_chain_facts() {
        let publish = RsScriptPackagePublishInput {
            package: RsScriptPackageIdentityInput {
                name: "demo".to_owned(),
                version: "0.1.0".to_owned(),
                edition: "2026".to_owned(),
            },
            ready: true,
            risk: RsScriptPackageRisk::Low,
            registry_index: RsScriptRegistryIndexInput {
                schema: "rss.registry.index.v1".to_owned(),
                name: "demo".to_owned(),
                version: "0.1.0".to_owned(),
                checksum: "sha256:archive".to_owned(),
                interface_hash: "sha256:interface".to_owned(),
                effective_interface_hash_default: "sha256:interface-default".to_owned(),
                review_hash: "sha256:review".to_owned(),
                review_schema: "rss.review.package.v1".to_owned(),
                native_hash: Some("sha256:native".to_owned()),
                risk: RsScriptPackageRisk::Low,
                native: true,
                unsafe_boundary: false,
                dependencies: BTreeMap::new(),
                features: BTreeMap::from([("default".to_owned(), vec!["streaming".to_owned()])]),
                footprint_default: RsScriptRegistryFootprintInput {
                    direct_dependencies: 1,
                    total_packages: 2,
                    unknown_facts: 1,
                    ..RsScriptRegistryFootprintInput::default()
                },
            },
            registry_target: Some(RsScriptRegistryTargetInput {
                registry_dir: "/tmp/registry".to_owned(),
                index_path: "/tmp/registry/index/demo/0.1.0.json".to_owned(),
                archive_manifest_path: "/tmp/registry/archives/demo/0.1.0/archive-manifest.json"
                    .to_owned(),
            }),
            archive_format: "rss.package.archive.v1".to_owned(),
            archive_hash: "sha256:archive".to_owned(),
            checks: vec![RsScriptPublishCheckInput {
                name: "package archive reproducible".to_owned(),
                ok: true,
                risk: RsScriptPackageRisk::Low,
                detail: "3 files; sha256:archive".to_owned(),
            }],
        };

        let json = serde_json::to_string(&publish).unwrap();
        let bundle = rsscript_publish_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.publish.demo_0_1_0.effective_interface_hash"
                && fact.evidence[0].kind == EvidenceKind::RegistryMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/registry/index/demo/0.1.0.json")
                && fact.evidence[0].json_pointer.as_deref()
                    == Some("/registry_index/effective_interface_hash_default")
                && fact.evidence[0].value.as_deref() == Some("sha256:interface-default")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.publish.demo_0_1_0.archive_checksum"
                && fact.evidence[0].file.as_deref()
                    == Some("/tmp/registry/archives/demo/0.1.0/archive-manifest.json")
                && fact.evidence[0].json_pointer.as_deref() == Some("/archive_hash")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.publish.demo_0_1_0.review_schema"
                && fact.evidence[0].json_pointer.as_deref() == Some("/registry_index/review_schema")
                && fact.evidence[0].value.as_deref() == Some("rss.review.package.v1")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.publish.demo_0_1_0.default_features"
                && fact.evidence[0].json_pointer.as_deref()
                    == Some("/registry_index/features/default")
                && fact.evidence[0].value.as_deref() == Some("streaming")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.id == "fact.publish.demo_0_1_0.registry_footprint"
                && fact.evidence[0].json_pointer.as_deref()
                    == Some("/registry_index/footprint_default")
                && fact
                    .unknown_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("unknown or unresolved"))
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.id == "fact.publish.demo_0_1_0.registry_native"
                && fact.value == FactValue::True
                && fact.evidence[0].json_pointer.as_deref() == Some("/registry_index/native")
                && fact.evidence[0].value.as_deref() == Some("true")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::UnsafeBoundary
                && fact.id == "fact.publish.demo_0_1_0.registry_unsafe_apis"
                && fact.value == FactValue::False
                && fact.evidence[0].json_pointer.as_deref() == Some("/registry_index/unsafe_apis")
                && fact.evidence[0].value.as_deref() == Some("false")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.publish.demo_0_1_0.readiness"
                && fact.value == FactValue::True
                && fact.evidence[0].file.as_deref() == Some("/tmp/registry")
                && fact.evidence[0].json_pointer.as_deref() == Some("/ready")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.publish.demo_0_1_0.check.package_archive_reproducible"
                && fact.evidence[0].file.as_deref() == Some("/tmp/registry")
                && fact.evidence[0].json_pointer.as_deref() == Some("/checks/0")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.publish.demo_0_1_0.archive_checksum".to_owned())
        }));
    }

    #[test]
    fn package_publish_to_bundle_accepts_legacy_registry_unsafe_field() {
        let publish = serde_json::json!({
            "package": { "name": "demo", "version": "0.1.0", "edition": "2026" },
            "ready": true,
            "risk": "low",
            "registry_index": {
                "schema": "rss.registry.index.v1",
                "name": "demo",
                "version": "0.1.0",
                "checksum": "sha256:archive",
                "interface_hash": "sha256:interface",
                "review_hash": "sha256:review",
                "risk": "low",
                "native": false,
                "unsafe": true
            },
            "archive_format": "rss.package.archive.v1",
            "archive_hash": "sha256:archive",
            "checks": []
        });

        let bundle = rsscript_publish_json_to_bundle(&publish.to_string()).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::UnsafeBoundary
                && fact.id == "fact.publish.demo_0_1_0.registry_unsafe_apis"
                && fact.value == FactValue::True
                && fact.evidence[0].value.as_deref() == Some("true")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::NativeUnsafeSlice
                && slice
                    .facts
                    .contains(&"fact.publish.demo_0_1_0.registry_unsafe_apis".to_owned())
        }));
    }

    #[test]
    fn package_publish_to_bundle_marks_missing_hashes_unknown() {
        let publish = serde_json::json!({
            "package": { "name": "demo", "version": "0.1.0", "edition": "2026" },
            "ready": false,
            "risk": "unknown",
            "registry_index": {
                "schema": "rss.registry.index.v1",
                "name": "demo",
                "version": "0.1.0",
                "checksum": "",
                "interface_hash": "",
                "review_hash": "",
                "risk": "unknown",
                "native": false,
                "unsafe_apis": false
            },
            "archive_format": "rss.package.archive.v1",
            "archive_hash": "",
            "checks": []
        });

        let bundle = rsscript_publish_json_to_bundle(&publish.to_string()).unwrap();

        for id in [
            "fact.publish.demo_0_1_0.archive_checksum",
            "fact.publish.demo_0_1_0.registry_checksum",
            "fact.publish.demo_0_1_0.effective_interface_hash",
            "fact.publish.demo_0_1_0.review_hash",
        ] {
            let fact = bundle
                .facts
                .iter()
                .find(|fact| fact.id == id)
                .unwrap_or_else(|| panic!("missing publish fact `{id}`"));
            assert_eq!(fact.kind, FactKind::SupplyChain);
            assert_eq!(fact.value, FactValue::Unknown);
            assert!(fact.evidence[0].value.is_none());
            assert!(
                fact.unknown_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("is missing"))
            );
        }
    }

    #[test]
    fn package_metadata_to_bundle_emits_artifact_status_facts() {
        let metadata = RsScriptPackageMetadataInput {
            package: RsScriptPackageIdentityInput {
                name: "demo".to_owned(),
                version: "0.1.0".to_owned(),
                edition: "2026".to_owned(),
            },
            package_dir: "/tmp/demo".to_owned(),
            metadata_path: "/tmp/demo/review/package-review.json".to_owned(),
            reir_path: "/tmp/demo/review/reir/rsscript.json".to_owned(),
            dry_run: false,
            written: false,
            verified: false,
            ok: false,
            risk: RsScriptPackageRisk::Unknown,
            reasons: vec!["unknown API".to_owned()],
            mismatches: vec![RsScriptPackageMetadataMismatch {
                artifact: "reir_bundle".to_owned(),
                path: "/tmp/demo/review/reir/rsscript.json".to_owned(),
                kind: "stale".to_owned(),
                message: "artifact does not match the current package review result".to_owned(),
                expected_sha256: "sha256:expected".to_owned(),
                actual_sha256: Some("sha256:actual".to_owned()),
            }],
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let bundle = rsscript_metadata_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.metadata.demo_0_1_0.status"
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo")
                && fact.evidence[0].json_pointer.as_deref() == Some("/ok")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.metadata.demo_0_1_0.reir_artifact"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].json_pointer.as_deref() == Some("/reir_path")
                && fact.evidence[0].value.as_deref() == Some("/tmp/demo/review/reir/rsscript.json")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id.contains(".mismatch.")
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/review/reir/rsscript.json")
                && fact.evidence[0].json_pointer.as_deref() == Some("/mismatches/0")
                && fact.evidence[0].value.as_deref().is_some_and(|value| {
                    value.contains("expected=sha256:expected")
                        && value.contains("actual=sha256:actual")
                })
                && fact.unknown_reason.as_deref().is_some_and(|reason| {
                    reason.contains("review/reir/rsscript.json")
                        && reason.contains("stale")
                        && reason.contains("sha256:expected")
                        && reason.contains("sha256:actual")
                })
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.metadata.demo_0_1_0.reir_artifact".to_owned())
        }));
    }

    #[test]
    fn package_vendor_to_bundle_emits_supply_chain_and_unresolved_facts() {
        let vendor = RsScriptPackageVendorInput {
            package: RsScriptPackageIdentityInput {
                name: "app".to_owned(),
                version: "0.1.0".to_owned(),
                edition: "2026".to_owned(),
            },
            package_dir: "/tmp/app".to_owned(),
            vendor_dir: "/tmp/app/vendor".to_owned(),
            dry_run: true,
            ok: false,
            risk: RsScriptPackageRisk::Unknown,
            entries: vec![RsScriptPackageVendorEntry {
                name: "dep".to_owned(),
                version: "1.0.0".to_owned(),
                source_path: "/tmp/dep".to_owned(),
                vendor_path: "/tmp/app/vendor/dep-1.0.0".to_owned(),
                checksum: "sha256:dep".to_owned(),
                native: true,
            }],
            unresolved: vec![RsScriptPackageVendorUnresolved {
                name: "registry-dep".to_owned(),
                requirement: Some("^1".to_owned()),
                source: "registry".to_owned(),
                reason: "dependency resolver not implemented for this source".to_owned(),
            }],
            reasons: vec!["registry-dep unresolved".to_owned()],
        };

        let json = serde_json::to_string(&vendor).unwrap();
        let bundle = rsscript_vendor_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.vendor.app_0_1_0.status"
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/app/vendor")
                && fact.evidence[0].json_pointer.as_deref() == Some("/ok")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.vendor.app_0_1_0.entry.dep_1_0_0.checksum"
                && fact.subject.id == "dep@1.0.0"
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/app/vendor/dep-1.0.0")
                && fact.evidence[0].json_pointer.as_deref() == Some("/entries/0")
                && fact.evidence[0].value.as_deref() == Some("sha256:dep")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "registry-dep@^1"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.as_deref() == Some("/tmp/app/vendor")
                && fact.evidence[0].json_pointer.as_deref() == Some("/unresolved/0")
                && fact.unknown_reason.as_deref()
                    == Some("dependency resolver not implemented for this source")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.vendor.app_0_1_0.entry.dep_1_0_0.checksum".to_owned())
        }));
    }

    #[test]
    fn rsscript_bundle_serializes_round_trip() {
        let review_map = sample_review_map();
        let package_review = sample_package_review();

        let bundle = rsscript_to_bundle(&review_map, &package_review);
        let json = bundle.to_json().unwrap();
        let round_trip = Bundle::from_json(&json).unwrap();

        assert_eq!(bundle.producers.len(), 1);
        assert_eq!(bundle.producers[0].name, "rssc");
        assert_eq!(bundle.facts.len(), 12);
        assert_eq!(bundle.edges.len(), 3);
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::PackageRiskSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NativeUnsafeSlice)
        );
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::Package && subject.id == "demo_pkg@1.2.3"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::CodeModule
                && subject.id == "demo_pkg::module::rss.package.review"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::NativeBoundary
                && subject.id == "demo_pkg::native::ffi.crypto"
        }));
        assert!(bundle.edges.iter().any(|edge| {
            edge.kind == EdgeKind::NormalizesToNativeFn
                && edge.from.id == "demo_pkg::native::ffi.crypto"
                && edge.to.id == "demo_pkg::native_fn"
        }));
        assert_eq!(round_trip, bundle);
    }

    #[test]
    fn rsscript_json_collects_package_review_with_embedded_review_map() {
        let package_review = r#"{
            "package": { "name": "rss-rayon", "version": "0.1.0" },
            "risk": "high",
            "summary": {
                "public_apis": 4,
                "mutating_apis": 1,
                "retaining_apis": 0,
                "resource_apis": 0,
                "native_apis": 4,
                "unsafe_apis": 0,
                "unknown_apis": 0
            },
            "files": [
                { "path": "rss/rayon/interface/lib.rssi", "kind": "interface" }
            ],
            "exports": [
                {
                    "name": "Rayon.sum_int",
                    "kind": "function",
                    "classification": "review_if_changed",
                    "normalized_effects": ["native", "parallel"]
                },
                {
                    "name": "Rayon.sort_int",
                    "kind": "function",
                    "classification": "review_if_changed",
                    "normalized_effects": ["native", "parallel"]
                }
            ],
            "review_map": {
                "files": [
                    {
                        "file": "rss/rayon/interface/lib.rssi",
                        "regions": [
                            {
                                "function": "Rayon.sum_int",
                                "classification": "must_review",
                                "line": 3,
                                "reasons": ["native boundary", "parallel boundary"]
                            },
                            {
                                "function": "Rayon.sort_int",
                                "classification": "must_review",
                                "line": 7,
                                "reasons": ["native boundary", "parallel boundary"]
                            }
                        ]
                    }
                ]
            }
        }"#;

        let bundle = rsscript_json_to_bundle(None, Some(package_review), None).unwrap();

        assert_eq!(bundle.producers.len(), 1);
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PackageRisk && fact.subject.id == "rss-rayon@0.1.0"
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "rss-rayon::Rayon.sum_int"
                && fact.evidence[0].line == Some(3)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PublicContract
                && fact.subject.kind == SubjectKind::CodePublicApi
                && fact.subject.id == "rss-rayon::public::function::Rayon.sum_int"
                && fact.evidence[0].value.as_deref() == Some("function")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "rss-rayon::native::Rayon"
                && fact.evidence[0].file.as_deref() == Some("rss/rayon/interface/lib.rssi")
                && fact.evidence[0].line == Some(3)
        }));
        assert_eq!(
            bundle
                .facts
                .iter()
                .filter(|fact| {
                    fact.id == "fact.native_boundary.rss_rayon__native__Rayon"
                        && fact.kind == FactKind::NativeBoundary
                })
                .count(),
            1
        );
        assert!(bundle.edges.iter().any(|edge| {
            edge.from.id == "rss-rayon::Rayon.sum_int" && edge.to.id == "rss-rayon::native::Rayon"
        }));
        assert!(bundle.edges.iter().any(|edge| {
            edge.from.id == "rss-rayon::Rayon.sort_int" && edge.to.id == "rss-rayon::native::Rayon"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::Package && subject.id == "rss-rayon@0.1.0"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::NativeBoundary && subject.id == "rss-rayon::native::Rayon"
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::NativeUnsafeSlice
                && slice
                    .facts
                    .contains(&"fact.native_boundary.rss_rayon__native__Rayon".to_owned())
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.package.rss_rayon_0_1_0.risk".to_owned())
        }));
    }

    #[test]
    fn review_map_json_import_preserves_module_and_use_declarations() {
        let json = r#"{
            "summary": {},
            "modules": [
                {
                    "file": "src/package/review.rss",
                    "module_path": "rss.package.review",
                    "line": 3,
                    "uses": [
                        { "path": "rss.package.contract.PackageContract", "line": 5 },
                        { "path": "rss.review.ReviewMap", "line": 6 }
                    ]
                }
            ],
            "files": []
        }"#;

        let input =
            review_map_input_from_json("demo_pkg", json).expect("review map JSON should import");
        let facts = review_map_to_facts(&input);

        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::ModuleDeclaration
                && fact.subject.id == "demo_pkg::module::rss.package.review"
                && fact.evidence[0].line == Some(3)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.evidence[0].symbol.as_deref()
                    == Some("rss.package.contract.PackageContract")
                && fact.evidence[0].line == Some(5)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.evidence[0].symbol.as_deref() == Some("rss.review.ReviewMap")
                && fact.evidence[0].line == Some(6)
        }));
    }

    #[test]
    fn rsscript_json_collects_review_map_only_with_explicit_package_name() {
        let review_map = r#"{
            "summary": { "total_functions": 2 },
            "files": [
                {
                    "file": "tests/fixtures/pass/module-use-basic.rss",
                    "regions": [
                        {
                            "function": "review_package",
                            "classification": "low_semantic_risk",
                            "line": 7,
                            "reasons": ["private pure helper"]
                        },
                        {
                            "function": "main",
                            "classification": "must_review",
                            "line": 11,
                            "reasons": ["entry point"]
                        }
                    ]
                }
            ]
        }"#;

        let bundle = rsscript_json_to_bundle(Some(review_map), None, Some("pkg_tool")).unwrap();

        assert_eq!(bundle.facts.len(), 1);
        let fact = &bundle.facts[0];
        assert_eq!(
            fact.kind,
            FactKind::Extension(REVIEW_REQUIRED_KIND.to_owned())
        );
        assert_eq!(fact.subject.id, "pkg_tool::main");
        assert_eq!(fact.evidence[0].line, Some(11));
        assert_eq!(bundle.subjects.len(), 1);
        assert_eq!(bundle.subjects[0].id, "pkg_tool::main");
        assert!(bundle.slices.is_empty());
    }
}
