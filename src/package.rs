use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analyzer::{
    analyze_source_with_interfaces, analyze_sources_with_interfaces, core_interfaces,
};
use crate::diagnostic::{Diagnostic, code};
use crate::formatter::format_program;
use crate::review::{
    ReviewFinding, ReviewMap, ReviewMapClassification, ReviewRisk, format_review_human,
    review_map_sources, review_sources,
};
use crate::rust_lower::NativeRustDependency;
use crate::syntax::ast::{
    DataEffect, EffectDecl, FieldDecl, FileFeature, FunctionDecl, GenericBound, GenericParam, Item,
    Param, Program, TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReview {
    pub package: PackageIdentity,
    pub manifest_path: String,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
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
    pub links: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    package: ManifestPackage,
    #[serde(default)]
    interfaces: ManifestPathSection,
    #[serde(default)]
    sources: ManifestPathSection,
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    review: Option<ManifestReview>,
    #[serde(default)]
    native: Option<ManifestNative>,
}

#[derive(Debug, Deserialize)]
struct ManifestPackage {
    name: String,
    version: String,
    edition: String,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestPathSection {
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestReview {
    risk: Option<String>,
    allow_native: Option<bool>,
    allow_unsafe: Option<bool>,
    unknown_is_error: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ManifestNative {
    #[serde(default)]
    rust: Option<ManifestNativeRust>,
}

#[derive(Debug, Deserialize)]
struct ManifestNativeRust {
    #[serde(default)]
    enabled: bool,
    path: Option<String>,
    #[serde(rename = "crate")]
    crate_name: Option<String>,
    build_scripts: Option<String>,
    proc_macros: Option<String>,
    #[serde(rename = "unsafe")]
    unsafe_policy: Option<String>,
    #[serde(default)]
    links: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataTarget {
    kind: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NativeBindingsManifest {
    #[serde(default)]
    bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct PackageSource {
    path: String,
    relative_path: String,
    contents: String,
    kind: PackageReviewFileKind,
}

#[derive(Debug, Clone)]
struct PackageDependencySpec {
    name: String,
    requirement: Option<String>,
    path: Option<String>,
    git: Option<String>,
    features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageFunctionContract {
    name: String,
    params: Vec<PackageParamContract>,
    return_type: Option<String>,
    returns_fresh: bool,
    effects: BTreeSet<String>,
    span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageParamContract {
    name: String,
    effect: Option<&'static str>,
    type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageTypeContract {
    name: String,
    kind: TypeKind,
    type_params: Vec<PackageGenericContract>,
    fields: Vec<PackageFieldContract>,
    span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageGenericContract {
    name: String,
    bound: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageFieldContract {
    name: String,
    type_name: String,
    is_handle: bool,
    is_weak: bool,
}

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    let package = load_package(package_dir)?;
    let manifest = &package.manifest;
    let sources = &package.sources;
    let dependency_interfaces = collect_dependency_interface_sources(package_dir, manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces = native_binding_interface_sources(sources, &native_bindings);

    let interface_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let dependency_interface_refs = dependency_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let mut external_interfaces = core_interfaces().to_vec();
    external_interfaces.extend(dependency_interface_refs);
    let mut combined_interfaces = external_interfaces.clone();
    combined_interfaces.extend(interface_refs.clone());
    let native_binding_interface_refs = native_binding_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let mut source_interfaces = external_interfaces.clone();
    source_interfaces.extend(native_binding_interface_refs);
    let source_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let mut diagnostics = package_interface_environment_diagnostics(&combined_interfaces);
    diagnostics.extend(interface_refs.iter().flat_map(|(path, contents)| {
        analyze_source_with_interfaces(path, contents, &external_interfaces)
    }));
    diagnostics.extend(analyze_sources_with_interfaces(
        &source_refs,
        &source_interfaces,
    ));
    diagnostics.extend(package_interface_contract_diagnostics(
        sources,
        &native_bindings,
    ));
    diagnostics.extend(package_native_binding_diagnostics(
        package_dir,
        sources,
        &native_bindings,
        manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
    ));
    dedup_diagnostics(&mut diagnostics);
    let review_map = review_map_sources(
        sources
            .iter()
            .map(|source| (source.path.as_str(), source.contents.as_str()))
            .collect(),
    );

    let native_rust = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .filter(|native| native.enabled)
        .map(|native| PackageNativeRustReview {
            path: native
                .path
                .clone()
                .unwrap_or_else(|| "native/rust".to_string()),
            crate_name: native.crate_name.clone(),
            build_scripts: native.build_scripts.clone(),
            proc_macros: native.proc_macros.clone(),
            unsafe_policy: native.unsafe_policy.clone(),
            links: native.links.clone(),
        });

    let mut reasons = Vec::new();
    collect_manifest_review_reasons(manifest, &mut reasons);
    collect_native_reasons(native_rust.as_ref(), &mut reasons);
    collect_review_map_reasons(&review_map, &mut reasons);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        reasons.push("package contains frontend errors".to_string());
    } else if !diagnostics.is_empty() {
        reasons.push("package contains frontend warnings".to_string());
    }
    reasons.sort();
    reasons.dedup();

    let risk = package_risk(manifest, native_rust.as_ref(), &review_map, &diagnostics);
    let api_summary = package_api_effect_summary(sources, &review_map);
    let summary = PackageReviewSummary {
        interface_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Interface)
            .count(),
        source_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Source)
            .count(),
        diagnostics: diagnostics.len(),
        errors: diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity.is_error())
            .count(),
        dependencies: manifest.dependencies.len(),
        dev_dependencies: manifest.dev_dependencies.len(),
        package_features: manifest.features.len(),
        public_types: api_summary.public_types,
        public_functions: api_summary.public_functions,
        public_apis: api_summary.public_apis,
        mutating_apis: api_summary.mutating_apis,
        retaining_apis: api_summary.retaining_apis,
        resource_apis: api_summary.resource_apis,
        fresh_returning_apis: api_summary.fresh_returning_apis,
        native_apis: api_summary.native_apis,
        unsafe_apis: api_summary.unsafe_apis,
        unknown_apis: api_summary.unknown_apis,
    };
    let files = sources
        .iter()
        .map(|source| PackageReviewFile {
            path: source.path.clone(),
            kind: source.kind,
        })
        .collect();
    let exports = package_review_exports(sources, &review_map);

    Ok(PackageReview {
        package: PackageIdentity {
            name: manifest.package.name.clone(),
            version: manifest.package.version.clone(),
            edition: manifest.package.edition.clone(),
        },
        manifest_path: package.manifest_path.display().to_string(),
        risk,
        reasons,
        summary,
        files,
        exports,
        native_rust,
        review_map,
        diagnostics,
    })
}

pub fn package_metadata(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageMetadataReport, String> {
    let review = review_package_dir(package_dir)?;
    let metadata_path = package_dir.join("review").join("package-review.json");
    let metadata = package_review_metadata_from_review(&review);
    let ok = review.summary.errors == 0;

    if !dry_run {
        let parent = metadata_path
            .parent()
            .ok_or_else(|| format!("metadata path has no parent: {}", metadata_path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        let json = serde_json::to_string_pretty(&metadata)
            .expect("package metadata JSON serialization should not fail");
        fs::write(&metadata_path, json)
            .map_err(|error| format!("failed to write {}: {error}", metadata_path.display()))?;
    }

    Ok(PackageMetadataReport {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        metadata_path: metadata_path.display().to_string(),
        dry_run,
        written: !dry_run,
        ok,
        risk: review.risk,
        reasons: review.reasons,
        metadata,
    })
}

pub fn package_lowering_input(package_dir: &Path) -> Result<PackageLoweringInput, String> {
    let package = load_package(package_dir)?;
    let dependency_interfaces =
        collect_dependency_interface_sources(package_dir, &package.manifest)?;
    let native_dependencies = package_native_rust_dependencies(package_dir, &package.manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces =
        native_binding_interface_sources(&package.sources, &native_bindings);
    let interfaces = dependency_interfaces
        .iter()
        .chain(native_binding_interfaces.iter())
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();

    let source_files = package
        .sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .collect::<Vec<_>>();
    let source = select_package_runnable_source(&source_files)?;
    let sources = source_files
        .iter()
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();
    Ok(PackageLoweringInput {
        package: PackageIdentity {
            name: package.manifest.package.name.clone(),
            version: package.manifest.package.version.clone(),
            edition: package.manifest.package.edition.clone(),
        },
        package_dir: package_dir.display().to_string(),
        source_path: source.path.clone(),
        source_relative_path: source.relative_path.clone(),
        source: source.contents.clone(),
        sources,
        interfaces,
        native_dependencies,
    })
}

fn package_native_rust_dependencies(
    package_dir: &Path,
    manifest: &Manifest,
) -> Result<Vec<NativeRustDependency>, String> {
    let Some(native) = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
    else {
        return Ok(Vec::new());
    };
    if !native.enabled {
        return Ok(Vec::new());
    }
    let crate_name = native
        .crate_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            "native.rust enabled packages must declare `crate` before Rust lowering.".to_string()
        })?;
    let native_path = native.path.as_deref().unwrap_or("native/rust");
    let bindings = package_native_bindings(package_dir)?;
    Ok(vec![NativeRustDependency {
        crate_name: crate_name.to_string(),
        path: absolute_package_path(package_dir)
            .join(native_path)
            .display()
            .to_string(),
        bindings,
    }])
}

fn absolute_package_path(package_dir: &Path) -> PathBuf {
    if package_dir.is_absolute() {
        return package_dir.to_path_buf();
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(package_dir)
}

fn package_native_bindings(package_dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = package_dir.join("native/bindings.rssbind.toml");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let manifest: NativeBindingsManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    Ok(manifest.bindings)
}

fn native_binding_interface_sources(
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
) -> Vec<PackageSource> {
    if native_bindings.is_empty() {
        return Vec::new();
    }
    let source_type_names = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .flat_map(|source| parse_source(&source.path, &source.contents).items)
        .filter_map(|item| match item {
            Item::Type(type_decl) => Some(type_decl.name),
            Item::Function(_) => None,
        })
        .collect::<BTreeSet<_>>();

    sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .filter_map(|source| {
            let mut selected_items = Vec::new();
            for item in parse_source(&source.path, &source.contents).items {
                match item {
                    Item::Type(type_decl) if !source_type_names.contains(&type_decl.name) => {
                        selected_items.push(Item::Type(type_decl));
                    }
                    Item::Function(function)
                        if function
                            .effects
                            .contains(&EffectDecl::Name("native".to_string()))
                            && native_bindings.contains_key(&function.name) =>
                    {
                        selected_items.push(Item::Function(function));
                    }
                    _ => {}
                }
            }
            if !selected_items
                .iter()
                .any(|item| matches!(item, Item::Function(_)))
            {
                return None;
            }
            let program = Program {
                features: vec![FileFeature::Native],
                unknown_features: Vec::new(),
                duplicate_features: Vec::new(),
                feature_spans: Vec::new(),
                profile_spans: Vec::new(),
                items: selected_items,
            };
            Some(PackageSource {
                path: format!("{}#native-bindings", source.path),
                relative_path: format!("{}#native-bindings", source.relative_path),
                contents: format_program(&program),
                kind: PackageReviewFileKind::Interface,
            })
        })
        .collect()
}

pub fn diff_package_dirs(old_dir: &Path, new_dir: &Path) -> Result<PackageDiff, String> {
    let old_package = load_package(old_dir)?;
    let new_package = load_package(new_dir)?;
    let old_review = review_package_dir(old_dir)?;
    let new_review = review_package_dir(new_dir)?;

    let mut manifest_changes = Vec::new();
    compare_package_identity(
        &old_package.manifest,
        &new_package.manifest,
        &mut manifest_changes,
    );
    compare_value_maps(
        "dependency",
        &old_package.manifest.dependencies,
        &new_package.manifest.dependencies,
        PackageRisk::High,
        &mut manifest_changes,
    );
    compare_value_maps(
        "dev-dependency",
        &old_package.manifest.dev_dependencies,
        &new_package.manifest.dev_dependencies,
        PackageRisk::Elevated,
        &mut manifest_changes,
    );
    compare_feature_maps(
        &old_package.manifest.features,
        &new_package.manifest.features,
        &mut manifest_changes,
    );
    compare_native_rust(
        old_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
        new_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
        &mut manifest_changes,
    );

    let interface_changes = compare_interface_sources(&old_package.sources, &new_package.sources);
    let mut reasons = package_diff_reasons(&manifest_changes, &interface_changes);
    if old_review.risk != new_review.risk {
        reasons.push(format!(
            "package risk changed from {} to {}",
            package_risk_label(old_review.risk),
            package_risk_label(new_review.risk)
        ));
    }
    reasons.sort();
    reasons.dedup();
    let risk = package_diff_risk(
        &manifest_changes,
        &interface_changes,
        old_review.risk,
        new_review.risk,
    );

    Ok(PackageDiff {
        old_package: package_identity(&old_package.manifest),
        new_package: package_identity(&new_package.manifest),
        risk,
        reasons,
        manifest_changes,
        interface_changes,
        old_review: old_review.summary,
        new_review: new_review.summary,
    })
}

pub fn check_package_dir(package_dir: &Path) -> Result<PackageCheck, String> {
    let review = review_package_dir(package_dir)?;
    let current_lock = lock_package_dir(package_dir)?;
    let graph = check_package_graph(package_dir)?;
    let lock = check_package_lock(package_dir, &current_lock)?;
    let native_rust = check_package_native_rust(package_dir, review.native_rust.as_ref())?;

    let mut reasons = review.reasons.clone();
    reasons.extend(graph.reasons.clone());
    reasons.extend(lock.reasons.clone());
    if let Some(native) = &native_rust {
        reasons.extend(native.reasons.clone());
    }
    reasons.sort();
    reasons.dedup();

    let diagnostics_have_errors = review
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error());
    let native_ok = native_rust
        .as_ref()
        .is_none_or(|native_check| native_check.ok);
    let ok = !diagnostics_have_errors && graph.ok && lock.matches && native_ok;
    let mut risk = review.risk.max(graph.risk).max(lock.risk);
    if let Some(native) = &native_rust {
        risk = risk.max(native.risk);
    }

    Ok(PackageCheck {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        ok,
        risk,
        reasons,
        summary: review.summary,
        graph,
        lock,
        native_rust,
        diagnostics: review.diagnostics,
    })
}

pub fn package_tree(package_dir: &Path) -> Result<PackageTree, String> {
    let mut visiting = BTreeSet::new();
    let root = package_tree_node(package_dir, PackageDependencyKind::Root, &mut visiting)?;
    let mut summary = PackageTreeSummary::default();
    collect_package_tree_summary(&root, &mut summary);
    Ok(PackageTree { root, summary })
}

pub fn publish_package_dry_run(package_dir: &Path) -> Result<PackagePublishDryRun, String> {
    publish_package_dry_run_with_registry(package_dir, None)
}

pub fn publish_package_dry_run_with_registry(
    package_dir: &Path,
    registry_dir: Option<&Path>,
) -> Result<PackagePublishDryRun, String> {
    let package = load_package(package_dir)?;
    let review = review_package_dir(package_dir)?;
    let check = check_package_dir(package_dir)?;
    let tree = package_tree(package_dir)?;
    let archive_files = package_archive_files(package_dir)?;
    let archive_hash = package_archive_hash(&archive_files);
    let root_features = package
        .manifest
        .features
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let root_lock_entry = lock_package_entry(package_dir, &package, root_features)?;

    let version_ok = is_semver_like(&package.manifest.package.version);
    let dependency_graph_ok = tree.summary.unknown_risk_packages == 0;
    let native_ok = check
        .native_rust
        .as_ref()
        .is_none_or(|native_check| native_check.ok);
    let frontend_ok = review.summary.errors == 0;

    let checks = vec![
        publish_check(
            "manifest valid",
            true,
            PackageRisk::Low,
            format!("{} parsed", package.manifest_path.display()),
        ),
        publish_check(
            "interfaces parse/check",
            frontend_ok,
            if frontend_ok {
                PackageRisk::Low
            } else {
                PackageRisk::High
            },
            format!("{} frontend errors", review.summary.errors),
        ),
        publish_check(
            "implementation checks",
            check.ok,
            check.risk,
            if check.ok {
                "package check passed".to_string()
            } else {
                check.reasons.join("; ")
            },
        ),
        publish_check(
            "native metadata generated",
            native_ok,
            check
                .native_rust
                .as_ref()
                .map(|native_check| native_check.risk)
                .unwrap_or(PackageRisk::Low),
            if let Some(native) = &check.native_rust {
                format!(
                    "native rust {} cargo_toml={} files={}",
                    native.path, native.cargo_toml_present, native.file_count
                )
            } else {
                "no native rust wrapper".to_string()
            },
        ),
        publish_check(
            "semantic version check",
            version_ok,
            if version_ok {
                PackageRisk::Low
            } else {
                PackageRisk::High
            },
            format!("version {}", package.manifest.package.version),
        ),
        publish_check(
            "package review metadata generated",
            true,
            review.risk,
            format!("review risk {}", package_risk_label(review.risk)),
        ),
        publish_check(
            "dependency graph review",
            dependency_graph_ok,
            if dependency_graph_ok {
                PackageRisk::Low
            } else {
                PackageRisk::Unknown
            },
            format!(
                "{} packages; {} unknown",
                tree.summary.packages, tree.summary.unknown_risk_packages
            ),
        ),
        publish_check(
            "package archive reproducible",
            true,
            PackageRisk::Low,
            format!("{} files; {archive_hash}", archive_files.len()),
        ),
    ];

    let ready = checks.iter().all(|check| check.ok);
    let risk = checks
        .iter()
        .fold(PackageRisk::Low, |risk, check| risk.max(check.risk));
    let mut reasons = checks
        .iter()
        .filter(|check| !check.ok)
        .map(|check| format!("{} failed: {}", check.name, check.detail))
        .collect::<Vec<_>>();
    reasons.extend(check.reasons);
    reasons.sort();
    reasons.dedup();
    let registry_index = package_registry_index_entry(
        &package,
        &root_lock_entry,
        check.native_rust.as_ref(),
        risk,
        &archive_hash,
    );
    let registry_target = registry_dir.map(|registry_dir| {
        package_registry_publish_target(
            registry_dir,
            &package.manifest.package.name,
            &package.manifest.package.version,
        )
    });

    Ok(PackagePublishDryRun {
        package: package_identity(&package.manifest),
        package_dir: package_dir.display().to_string(),
        ready,
        risk,
        reasons,
        registry_index,
        registry_target,
        archive_format: "rss.package.archive.v1".to_string(),
        archive_hash,
        archive_files,
        review: review.summary,
        dependency_summary: tree.summary,
        checks,
    })
}

pub fn vendor_package_dir(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageVendorReport, String> {
    let package = load_package(package_dir)?;
    let vendor_dir = package_dir.join("vendor");
    let mut visiting = BTreeSet::new();
    let mut entries = Vec::new();
    let mut unresolved = Vec::new();
    collect_vendor_dependencies(
        package_dir,
        &package.manifest.dependencies,
        &vendor_dir,
        &mut visiting,
        &mut entries,
        &mut unresolved,
    )?;

    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.version.cmp(&right.version))
    });
    entries.dedup_by(|left, right| left.name == right.name && left.version == right.version);
    unresolved.sort_by(|left, right| left.name.cmp(&right.name));

    if !dry_run {
        fs::create_dir_all(&vendor_dir)
            .map_err(|error| format!("failed to create {}: {error}", vendor_dir.display()))?;
        for entry in &entries {
            let source_path = Path::new(&entry.source_path);
            let vendor_path = Path::new(&entry.vendor_path);
            if vendor_path.exists() {
                fs::remove_dir_all(vendor_path).map_err(|error| {
                    format!("failed to remove {}: {error}", vendor_path.display())
                })?;
            }
            copy_package_directory(source_path, vendor_path)?;
        }
        let metadata_path = vendor_dir.join("rss-vendor.json");
        let metadata = serde_json::to_string_pretty(&entries)
            .expect("vendor metadata JSON serialization should not fail");
        fs::write(&metadata_path, metadata)
            .map_err(|error| format!("failed to write {}: {error}", metadata_path.display()))?;
    }

    let ok = unresolved.is_empty();
    let risk = if ok {
        PackageRisk::Low
    } else {
        PackageRisk::Unknown
    };
    let reasons = unresolved
        .iter()
        .map(|dependency| format!("{} unresolved: {}", dependency.name, dependency.reason))
        .collect::<Vec<_>>();

    Ok(PackageVendorReport {
        package: package_identity(&package.manifest),
        package_dir: package_dir.display().to_string(),
        vendor_dir: vendor_dir.display().to_string(),
        dry_run,
        ok,
        risk,
        entries,
        unresolved,
        reasons,
    })
}

pub fn lock_package_dir(package_dir: &Path) -> Result<PackageLock, String> {
    let package = load_package(package_dir)?;
    let root_features = package
        .manifest
        .features
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut packages = vec![lock_package_entry(package_dir, &package, root_features)?];
    let mut visiting = BTreeSet::new();
    let root_key = canonical_path_label(package_dir);
    visiting.insert(root_key.clone());
    collect_lock_dependency_packages(
        package_dir,
        &package.manifest.dependencies,
        &mut visiting,
        &mut packages,
    )?;
    collect_lock_dependency_packages(
        package_dir,
        &package.manifest.dev_dependencies,
        &mut visiting,
        &mut packages,
    )?;
    visiting.remove(&root_key);

    Ok(PackageLock {
        version: 1,
        packages,
        metadata: PackageLockMetadata {
            rsscript_version: env!("CARGO_PKG_VERSION").to_string(),
            created_by: "rsscript package lock".to_string(),
        },
    })
}

fn lock_package_entry(
    package_dir: &Path,
    package: &LoadedPackage,
    features: Vec<String>,
) -> Result<PackageLockPackage, String> {
    let review = review_package_dir(package_dir)?;
    let native = package
        .manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref());
    let native_hash = package_native_hash(package_dir, native)?;

    Ok(PackageLockPackage {
        name: package.manifest.package.name.clone(),
        version: package.manifest.package.version.clone(),
        source: format!("path+{}", package_dir.display()),
        checksum: package_checksum(package, native_hash.as_deref()),
        interface_hash: hash_sources(&package.sources, PackageReviewFileKind::Interface),
        review_hash: package_review_hash(&review),
        native_hash,
        features,
    })
}

pub fn diff_package_locks(old_path: &Path, new_path: &Path) -> Result<PackageLockDiff, String> {
    let old_lock = read_package_lock(old_path)?;
    let new_lock = read_package_lock(new_path)?;
    let package_changes = compare_locked_packages(&old_lock.packages, &new_lock.packages);
    let risk = package_changes
        .iter()
        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
    let mut reasons = package_lock_diff_reasons(&package_changes);
    if old_lock.version != new_lock.version {
        reasons.push("lockfile format version changed".to_string());
    }
    reasons.sort();
    reasons.dedup();

    Ok(PackageLockDiff {
        old_lock_path: old_path.display().to_string(),
        new_lock_path: new_path.display().to_string(),
        risk,
        reasons,
        old_packages: old_lock.packages.len(),
        new_packages: new_lock.packages.len(),
        package_changes,
    })
}

pub fn format_package_review_json(review: &PackageReview) -> String {
    serde_json::to_string(review).expect("package review JSON serialization should not fail")
}

pub fn format_package_metadata_json(metadata: &PackageMetadataReport) -> String {
    serde_json::to_string(metadata).expect("package metadata JSON serialization should not fail")
}

pub fn format_package_diff_json(diff: &PackageDiff) -> String {
    serde_json::to_string(diff).expect("package diff JSON serialization should not fail")
}

pub fn format_package_check_json(check: &PackageCheck) -> String {
    serde_json::to_string(check).expect("package check JSON serialization should not fail")
}

pub fn format_package_tree_json(tree: &PackageTree) -> String {
    serde_json::to_string(tree).expect("package tree JSON serialization should not fail")
}

pub fn format_package_publish_json(publish: &PackagePublishDryRun) -> String {
    serde_json::to_string(publish).expect("package publish JSON serialization should not fail")
}

pub fn format_package_vendor_json(vendor: &PackageVendorReport) -> String {
    serde_json::to_string(vendor).expect("package vendor JSON serialization should not fail")
}

pub fn format_package_lock_json(lock: &PackageLock) -> String {
    serde_json::to_string(lock).expect("package lock JSON serialization should not fail")
}

pub fn format_package_lock_toml(lock: &PackageLock) -> String {
    toml::to_string_pretty(lock).expect("package lock TOML serialization should not fail")
}

pub fn format_package_lock_diff_json(diff: &PackageLockDiff) -> String {
    serde_json::to_string(diff).expect("package lock diff JSON serialization should not fail")
}

pub fn format_package_review_human(review: &PackageReview) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package {} {} ({}) risk {}\n",
        review.package.name,
        review.package.version,
        review.package.edition,
        package_risk_label(review.risk)
    ));
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} native APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.dependencies,
        review.summary.package_features,
        review.summary.public_types,
        review.summary.public_functions,
        review.summary.public_apis,
        review.summary.mutating_apis,
        review.summary.retaining_apis,
        review.summary.resource_apis,
        review.summary.fresh_returning_apis,
        review.summary.native_apis,
        review.summary.unsafe_apis,
        review.summary.unknown_apis,
        review.summary.diagnostics,
        review.summary.errors
    ));
    if !review.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &review.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    if let Some(native) = &review.native_rust {
        output.push_str(&format!("native rust: {}", native.path));
        if let Some(crate_name) = &native.crate_name {
            output.push_str(&format!(" crate {crate_name}"));
        }
        output.push('\n');
    }
    output.push_str(&format_package_review_exports_human(&review.exports));
    output
}

pub fn format_package_metadata_human(metadata: &PackageMetadataReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package metadata {} {} {} risk {}\n",
        metadata.package.name,
        metadata.package.version,
        if metadata.dry_run { "dry-run" } else { "wrote" },
        package_risk_label(metadata.risk)
    ));
    output.push_str(&format!("metadata path: {}\n", metadata.metadata_path));
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} public types; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} native APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        metadata.metadata.summary.interface_files,
        metadata.metadata.summary.source_files,
        metadata.metadata.summary.public_types,
        metadata.metadata.summary.public_functions,
        metadata.metadata.summary.public_apis,
        metadata.metadata.summary.mutating_apis,
        metadata.metadata.summary.retaining_apis,
        metadata.metadata.summary.resource_apis,
        metadata.metadata.summary.fresh_returning_apis,
        metadata.metadata.summary.native_apis,
        metadata.metadata.summary.unsafe_apis,
        metadata.metadata.summary.unknown_apis,
        metadata.metadata.summary.diagnostics,
        metadata.metadata.summary.errors
    ));
    for reason in &metadata.reasons {
        output.push_str(&format!("reason: {reason}\n"));
    }
    output.push_str(&format_package_review_exports_human(
        &metadata.metadata.exports,
    ));
    output
}

fn format_package_review_exports_human(exports: &[PackageReviewExport]) -> String {
    if exports.is_empty() {
        return String::new();
    }
    let mut output = String::from("exports:\n");
    for export in exports {
        output.push_str(&format!(
            "  - {} {}: {}",
            export.kind, export.name, export.classification
        ));
        if !export.reasons.is_empty() {
            output.push_str(&format!(" ({})", export.reasons.join(", ")));
        }
        output.push('\n');
    }
    output
}

pub fn format_package_diff_human(diff: &PackageDiff) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package diff {} {} -> {} risk {}\n",
        diff.new_package.name,
        diff.old_package.version,
        diff.new_package.version,
        package_risk_label(diff.risk)
    ));
    if !diff.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &diff.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    for change in &diff.manifest_changes {
        output.push_str(&format!(
            "{} {}: {} -> {} ({})\n",
            change.kind,
            change.name,
            change.before.as_deref().unwrap_or("<none>"),
            change.after.as_deref().unwrap_or("<none>"),
            package_risk_label(change.risk)
        ));
    }
    for change in &diff.interface_changes {
        output.push_str(&format!(
            "interface {} {:?} ({})\n",
            change.file,
            change.change,
            package_risk_label(change.risk)
        ));
        if !change.findings.is_empty() {
            output.push_str(&format_review_human(&change.findings));
        }
    }
    output
}

pub fn format_package_check_human(check: &PackageCheck) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package check {} {} ({}) {} risk {}\n",
        check.package.name,
        check.package.version,
        check.package.edition,
        if check.ok { "ok" } else { "failed" },
        package_risk_label(check.risk)
    ));
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} native APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        check.summary.interface_files,
        check.summary.source_files,
        check.summary.dependencies,
        check.summary.package_features,
        check.summary.public_types,
        check.summary.public_functions,
        check.summary.public_apis,
        check.summary.mutating_apis,
        check.summary.retaining_apis,
        check.summary.resource_apis,
        check.summary.fresh_returning_apis,
        check.summary.native_apis,
        check.summary.unsafe_apis,
        check.summary.unknown_apis,
        check.summary.diagnostics,
        check.summary.errors
    ));
    output.push_str(&format!(
        "graph: {} ({})\n",
        if check.graph.ok { "ok" } else { "failed" },
        package_risk_label(check.graph.risk)
    ));
    output.push_str(&format!(
        "lock: {} {}\n",
        check.lock.path,
        if check.lock.matches {
            "matches"
        } else if check.lock.present {
            "stale"
        } else {
            "missing"
        }
    ));
    if let Some(native) = &check.native_rust {
        output.push_str(&format!(
            "native rust: {} cargo_toml={} cargo_metadata={} package={} targets={} unsafe={} links={} build_env={} build_download={} files={}\n",
            native.path,
            native.cargo_toml_present,
            native.cargo_metadata_ok,
            native.cargo_package_name.as_deref().unwrap_or("<unknown>"),
            if native.target_kinds.is_empty() {
                "<none>".to_string()
            } else {
                native.target_kinds.join(",")
            },
            native.unsafe_detected,
            if native.linked_libraries.is_empty() {
                "<none>".to_string()
            } else {
                native.linked_libraries.join(",")
            },
            native.build_env_detected,
            native.build_download_detected,
            native.file_count
        ));
    }
    if !check.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &check.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    output
}

pub fn format_package_tree_human(tree: &PackageTree) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package tree: {} packages; {} path deps; {} unresolved; {} native; {} high risk; {} unknown\n",
        tree.summary.packages,
        tree.summary.path_dependencies,
        tree.summary.unresolved_dependencies,
        tree.summary.native_packages,
        tree.summary.high_risk_packages,
        tree.summary.unknown_risk_packages
    ));
    format_package_tree_node_human(&tree.root, "", true, &mut output);
    output
}

pub fn format_package_publish_human(publish: &PackagePublishDryRun) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package publish dry-run {} {} {} risk {}\n",
        publish.package.name,
        publish.package.version,
        if publish.ready { "ready" } else { "blocked" },
        package_risk_label(publish.risk)
    ));
    output.push_str(&format!(
        "archive: {} {} files {}\n",
        publish.archive_format,
        publish.archive_files.len(),
        publish.archive_hash
    ));
    output.push_str(&format!(
        "registry index: {} {} {} risk {} native={} unsafe={}\n",
        publish.registry_index.schema,
        publish.registry_index.name,
        publish.registry_index.version,
        package_risk_label(publish.registry_index.risk),
        publish.registry_index.native,
        publish.registry_index.unsafe_boundary
    ));
    if let Some(target) = &publish.registry_target {
        output.push_str(&format!(
            "registry target: {} index={} archive_manifest={}\n",
            target.registry_dir, target.index_path, target.archive_manifest_path
        ));
    }
    for check in &publish.checks {
        output.push_str(&format!(
            "{}: {} ({}) {}\n",
            check.name,
            if check.ok { "ok" } else { "failed" },
            package_risk_label(check.risk),
            check.detail
        ));
    }
    if !publish.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &publish.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    output
}

pub fn format_package_vendor_human(vendor: &PackageVendorReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package vendor {} {} {} risk {}\n",
        vendor.package.name,
        vendor.package.version,
        if vendor.dry_run { "dry-run" } else { "wrote" },
        package_risk_label(vendor.risk)
    ));
    output.push_str(&format!("vendor dir: {}\n", vendor.vendor_dir));
    for entry in &vendor.entries {
        output.push_str(&format!(
            "vendored {} {} -> {} {}\n",
            entry.name, entry.version, entry.vendor_path, entry.checksum
        ));
    }
    for dependency in &vendor.unresolved {
        output.push_str(&format!(
            "unresolved {} {} ({})\n",
            dependency.name, dependency.source, dependency.reason
        ));
    }
    output
}

pub fn format_package_lock_diff_human(diff: &PackageLockDiff) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package lock update {} -> {} risk {}\n",
        diff.old_lock_path,
        diff.new_lock_path,
        package_risk_label(diff.risk)
    ));
    if !diff.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &diff.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    for package in &diff.package_changes {
        output.push_str(&format!(
            "package {}: {} -> {} ({})\n",
            package.name,
            package.before_version.as_deref().unwrap_or("<none>"),
            package.after_version.as_deref().unwrap_or("<none>"),
            package_risk_label(package.risk)
        ));
        for change in &package.changes {
            output.push_str(&format!(
                "  {}: {} -> {} ({})\n",
                change.field,
                change.before.as_deref().unwrap_or("<none>"),
                change.after.as_deref().unwrap_or("<none>"),
                package_risk_label(change.risk)
            ));
        }
    }
    output
}

struct LoadedPackage {
    manifest_path: PathBuf,
    manifest_source: String,
    manifest: Manifest,
    sources: Vec<PackageSource>,
}

fn load_package(package_dir: &Path) -> Result<LoadedPackage, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    let manifest_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&manifest_source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;

    let interface_roots = default_paths(&manifest.interfaces.paths, "interface");
    let source_roots = default_paths(&manifest.sources.paths, "src");
    let mut sources = Vec::new();
    sources.extend(read_package_sources(
        package_dir,
        &interface_roots,
        PackageReviewFileKind::Interface,
    )?);
    sources.extend(read_package_sources(
        package_dir,
        &source_roots,
        PackageReviewFileKind::Source,
    )?);
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(LoadedPackage {
        manifest_path,
        manifest_source,
        manifest,
        sources,
    })
}

fn default_paths(paths: &[String], default: &str) -> Vec<String> {
    if paths.is_empty() {
        vec![default.to_string()]
    } else {
        paths.to_vec()
    }
}

fn read_package_sources(
    package_dir: &Path,
    roots: &[String],
    kind: PackageReviewFileKind,
) -> Result<Vec<PackageSource>, String> {
    let mut sources = Vec::new();
    for root in roots {
        let root_path = package_dir.join(root);
        if !root_path.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rsscript_files(&root_path, &mut files)?;
        files.sort();
        for file in files {
            let contents = fs::read_to_string(&file)
                .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
            sources.push(PackageSource {
                path: file.display().to_string(),
                relative_path: relative_path(package_dir, &file),
                contents,
                kind,
            });
        }
    }
    Ok(sources)
}

fn select_package_runnable_source<'a>(
    source_files: &[&'a PackageSource],
) -> Result<&'a PackageSource, String> {
    if source_files.is_empty() {
        return Err("rss run requires one package source file under `src`.".to_string());
    }

    let main_sources = source_files
        .iter()
        .copied()
        .filter(|source| source.relative_path == "src/main.rss")
        .collect::<Vec<_>>();
    if source_files.len() == 1 {
        return Ok(source_files[0]);
    }
    if main_sources.len() == 1 {
        return Ok(main_sources[0]);
    }

    Err(
        "rss run package lowering requires `src/main.rss` when a package has multiple `.rss` source files."
            .to_string(),
    )
}

fn collect_dependency_interface_sources(
    package_dir: &Path,
    manifest: &Manifest,
) -> Result<Vec<PackageSource>, String> {
    let mut visiting = BTreeSet::new();
    let mut sources = Vec::new();
    collect_dependency_interface_sources_from_map(
        package_dir,
        &manifest.dependencies,
        &mut visiting,
        &mut sources,
    )?;
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(sources)
}

fn collect_dependency_interface_sources_from_map(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    visiting: &mut BTreeSet<String>,
    sources: &mut Vec<PackageSource>,
) -> Result<(), String> {
    for (name, value) in dependencies {
        let spec = package_dependency_spec(name, value);
        let Some(path) = &spec.path else {
            continue;
        };
        let dependency_dir = package_dir.join(path);
        if !dependency_dir.join("rsspkg.toml").exists() {
            continue;
        }
        let canonical = canonical_path_label(&dependency_dir);
        if !visiting.insert(canonical.clone()) {
            continue;
        }
        let dependency_package = load_package(&dependency_dir)?;
        sources.extend(
            dependency_package
                .sources
                .iter()
                .filter(|source| source.kind == PackageReviewFileKind::Interface)
                .cloned(),
        );
        collect_dependency_interface_sources_from_map(
            &dependency_dir,
            &dependency_package.manifest.dependencies,
            visiting,
            sources,
        )?;
        visiting.remove(&canonical);
    }
    Ok(())
}

fn relative_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn collect_rsscript_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        if is_rsscript_source_path(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_rsscript_files(&path, files)?;
        } else if is_rsscript_source_path(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_regular_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_regular_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn copy_package_directory(source: &Path, destination: &Path) -> Result<(), String> {
    if source.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::copy(source, destination).map_err(|error| {
            format!(
                "failed to copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        return Ok(());
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", source.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        if should_skip_vendor_copy_entry(&name.to_string_lossy()) {
            continue;
        }
        let target = destination.join(name);
        if path.is_dir() || path.is_file() {
            copy_package_directory(&path, &target)?;
        }
    }
    Ok(())
}

fn should_skip_vendor_copy_entry(name: &str) -> bool {
    matches!(name, ".git" | "target" | "vendor")
}

fn read_package_lock(path: &Path) -> Result<PackageLock, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&source).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn package_interface_environment_diagnostics(interfaces: &[(&str, &str)]) -> Vec<Diagnostic> {
    analyze_source_with_interfaces("<package-interface-environment>", "", interfaces)
        .into_iter()
        .filter(|diagnostic| diagnostic.code == code::DUPLICATE_DECLARATION)
        .collect()
}

fn package_interface_contract_diagnostics(
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    if !sources
        .iter()
        .any(|source| source.kind == PackageReviewFileKind::Source)
    {
        return Vec::new();
    }
    let source_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Source);
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Source);
    let interface_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let mut diagnostics = Vec::new();

    for (name, interface_contract) in interface_type_contracts {
        let Some(source_contract) = source_type_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface type `{name}` has no source declaration."),
                    interface_contract.span.clone(),
                    "missing source declaration",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every interface type must be declared by package source.")
                .with_fix(
                    "implement_interface_type",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_type_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_type_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface type `{name}` does not match its source declaration."),
                    interface_contract.span.clone(),
                    "interface/source type mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_type_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_type_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source declaration so their public type contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_function_contracts {
        let Some(source_contract) = source_function_contracts.get(&name) else {
            if interface_contract.effects.contains("native") && native_bindings.contains_key(&name)
            {
                continue;
            }
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface function `{name}` has no public source implementation."),
                    interface_contract.span.clone(),
                    "missing source implementation",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every public interface function must be implemented by package source.")
                .with_fix(
                    "implement_interface_function",
                    format!("Add `pub fn {name}` with the declared signature, or remove it from the interface."),
                    "manual",
                ),
            );
            continue;
        };

        if !package_function_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface function `{name}` does not match its source implementation."),
                    interface_contract.span.clone(),
                    "interface/source signature mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_function_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_function_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source implementation so their public signatures match exactly.",
                    "manual",
                ),
            );
        }
    }

    diagnostics
}

fn package_native_binding_diagnostics(
    package_dir: &Path,
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
    native: Option<&ManifestNativeRust>,
) -> Vec<Diagnostic> {
    if native_bindings.is_empty() {
        return Vec::new();
    }
    let interface_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let crate_name = native
        .and_then(|native| native.crate_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let mut diagnostics = Vec::new();
    let native_enabled = native.is_some_and(|native| native.enabled);

    if !native_enabled {
        diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_NATIVE_BINDING,
                "native bindings require enabled `[native.rust]` configuration.",
                native_binding_manifest_span(package_dir),
                "native binding without native Rust wrapper",
            )
            .with_cause("A binding manifest maps RSScript native contracts to Rust wrapper functions, so the package must enable a native Rust wrapper crate.")
            .with_fix(
                "enable_native_rust",
                "Add `[native.rust] enabled = true` with a wrapper crate, or remove `native/bindings.rssbind.toml`.",
                "manual",
            ),
        );
    } else if crate_name.is_none() {
        diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_NATIVE_BINDING,
                "native bindings require `[native.rust].crate`.",
                native_binding_manifest_span(package_dir),
                "native binding crate missing",
            )
            .with_cause("Generated Rust must know which native crate owns the binding targets.")
            .with_fix(
                "declare_native_crate",
                "Set `[native.rust] crate = \"...\"` to the Rust wrapper crate name.",
                "manual",
            ),
        );
    }

    for (symbol, target) in native_bindings {
        let span = native_binding_span(package_dir, symbol);
        if symbol.trim().is_empty() || target.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    "native binding entries must have non-empty RSScript symbols and Rust targets.",
                    span,
                    "invalid native binding",
                )
                .with_cause("Binding keys name RSScript native functions; values name Rust wrapper functions.")
                .with_fix(
                    "fix_native_binding",
                    "Write a binding such as `\"Native.echo\" = \"rss_json_native::echo\"`.",
                    "manual",
                ),
            );
            continue;
        }

        let Some(contract) = interface_function_contracts.get(symbol) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` does not match any package interface function."),
                    span,
                    "unknown native binding symbol",
                )
                .with_cause("Native bindings are reviewable only when their RSScript side is declared in a package `.rssi` contract.")
                .with_fix(
                    "declare_native_interface",
                    format!("Declare `native fn {symbol}(...)` in the package interface, or remove this binding."),
                    "manual",
                ),
            );
            continue;
        };

        if !contract.effects.contains("native") {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` points to a non-native interface function."),
                    span.clone(),
                    "non-native binding symbol",
                )
                .with_cause("Only interface functions declared with the native boundary can be implemented by native wrapper bindings.")
                .with_fix(
                    "mark_native_interface",
                    format!("Declare `{symbol}` as `native fn` or add `effects(native)`, or remove this binding."),
                    "manual",
                ),
            );
        }

        if let Some(crate_name) = crate_name
            && !target.starts_with(&format!("{crate_name}::"))
        {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!(
                        "native binding `{symbol}` targets `{target}`, outside configured native crate `{crate_name}`."
                    ),
                    span,
                    "native binding crate mismatch",
                )
                .with_cause("The generated Cargo package only wires the configured native Rust crate as a dependency for this package.")
                .with_fix(
                    "use_configured_native_crate",
                    format!("Use a Rust path starting with `{crate_name}::`, or update `[native.rust].crate`."),
                    "manual",
                ),
            );
        }
    }

    diagnostics
}

fn native_binding_span(package_dir: &Path, symbol: &str) -> crate::diagnostic::Span {
    let path = package_dir.join("native/bindings.rssbind.toml");
    let file = path.display().to_string();
    let source = fs::read_to_string(&path).unwrap_or_default();
    for (index, line) in source.lines().enumerate() {
        if let Some(column) = line.find(symbol) {
            return crate::diagnostic::Span {
                file,
                line: index + 1,
                column: column + 1,
                length: symbol.len().max(1),
            };
        }
    }
    crate::diagnostic::Span {
        file,
        line: 1,
        column: 1,
        length: symbol.len().max(1),
    }
}

fn native_binding_manifest_span(package_dir: &Path) -> crate::diagnostic::Span {
    crate::diagnostic::Span {
        file: package_dir
            .join("native/bindings.rssbind.toml")
            .display()
            .to_string(),
        line: 1,
        column: 1,
        length: 10,
    }
}

fn package_type_contracts_match(
    interface: &PackageTypeContract,
    source: &PackageTypeContract,
) -> bool {
    interface.name == source.name
        && interface.kind == source.kind
        && interface.type_params == source.type_params
        && interface.fields == source.fields
}

fn package_function_contracts_match(
    interface: &PackageFunctionContract,
    source: &PackageFunctionContract,
) -> bool {
    interface.name == source.name
        && interface.params == source.params
        && interface.return_type == source.return_type
        && interface.returns_fresh == source.returns_fresh
        && interface.effects == source.effects
}

fn collect_package_type_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageTypeContract> {
    let mut contracts = BTreeMap::new();
    for source in sources.iter().filter(|source| source.kind == kind) {
        let program = parse_source(&source.path, &source.contents);
        for item in program.items {
            let Item::Type(type_decl) = item else {
                continue;
            };
            contracts.insert(type_decl.name.clone(), package_type_contract(&type_decl));
        }
    }
    contracts
}

fn collect_package_function_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageFunctionContract> {
    let mut contracts = BTreeMap::new();
    for source in sources.iter().filter(|source| source.kind == kind) {
        let program = parse_source(&source.path, &source.contents);
        for item in program.items {
            let Item::Function(function) = item else {
                continue;
            };
            if kind == PackageReviewFileKind::Interface || function.is_public {
                contracts.insert(function.name.clone(), package_function_contract(&function));
            }
        }
    }
    contracts
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PackageApiSummary {
    public_types: usize,
    public_functions: usize,
    public_apis: usize,
    mutating_apis: usize,
    retaining_apis: usize,
    resource_apis: usize,
    fresh_returning_apis: usize,
    native_apis: usize,
    unsafe_apis: usize,
    unknown_apis: usize,
}

fn package_api_effect_summary(
    sources: &[PackageSource],
    review_map: &ReviewMap,
) -> PackageApiSummary {
    let interface_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_contracts;
    let source_type_contracts;
    let contracts = if interface_contracts.is_empty() {
        source_contracts =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_contracts
    } else {
        &interface_contracts
    };
    let type_contracts = if interface_type_contracts.is_empty() {
        source_type_contracts =
            collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_type_contracts
    } else {
        &interface_type_contracts
    };
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();

    PackageApiSummary {
        public_types: type_contracts.len(),
        public_functions: contracts.len(),
        public_apis: type_contracts.len() + contracts.len(),
        mutating_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .params
                    .iter()
                    .any(|param| param.effect == Some("mut"))
            })
            .count(),
        retaining_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.starts_with("retains("))
            })
            .count(),
        resource_apis: contracts
            .values()
            .filter(|contract| package_contract_has_resource_boundary(contract, &resource_types))
            .count(),
        fresh_returning_apis: contracts
            .values()
            .filter(|contract| contract.returns_fresh)
            .count(),
        native_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "native")
            })
            .count(),
        unsafe_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "unsafe")
            })
            .count(),
        unknown_apis: package_unknown_api_count(contracts, review_map),
    }
}

fn package_unknown_api_count(
    contracts: &BTreeMap<String, PackageFunctionContract>,
    review_map: &ReviewMap,
) -> usize {
    contracts
        .keys()
        .filter(|function| {
            review_map.files.iter().any(|file| {
                file.regions.iter().any(|region| {
                    &region.function == *function
                        && region.classification == ReviewMapClassification::Unknown
                })
            })
        })
        .count()
}

fn package_review_exports(
    sources: &[PackageSource],
    review_map: &ReviewMap,
) -> Vec<PackageReviewExport> {
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_type_contracts;
    let type_contracts = if interface_type_contracts.is_empty() {
        source_type_contracts =
            collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_type_contracts
    } else {
        &interface_type_contracts
    };
    let interface_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let source_function_contracts;
    let function_contracts = if interface_function_contracts.is_empty() {
        source_function_contracts =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_function_contracts
    } else {
        &interface_function_contracts
    };
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut exports = Vec::new();
    exports.extend(type_contracts.values().map(package_type_review_export));
    exports.extend(
        function_contracts
            .values()
            .map(|contract| package_function_review_export(contract, &resource_types, review_map)),
    );
    exports.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    exports
}

fn package_type_review_export(contract: &PackageTypeContract) -> PackageReviewExport {
    let mut reasons = vec![format!(
        "public {} type",
        package_type_kind_label(contract.kind)
    )];
    if contract.kind == TypeKind::Resource {
        reasons.push("resource type".to_string());
    }
    if contract.fields.iter().any(|field| field.is_handle) {
        reasons.push("handle field".to_string());
    }
    if contract.fields.iter().any(|field| field.is_weak) {
        reasons.push("weak field".to_string());
    }
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "type".to_string(),
        classification: "review_if_changed".to_string(),
        reasons,
    }
}

fn package_function_review_export(
    contract: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
    review_map: &ReviewMap,
) -> PackageReviewExport {
    let mut reasons = vec!["public function".to_string()];
    for param in &contract.params {
        if matches!(param.effect, Some("mut" | "take")) {
            reasons.push(format!(
                "{} parameter `{}`",
                param.effect.expect("effect matched"),
                param.name
            ));
        }
        if package_type_name_has_resource_boundary(&param.type_name, resource_types) {
            reasons.push(format!("resource parameter `{}`", param.name));
        }
    }
    if contract.return_type.as_ref().is_some_and(|return_type| {
        package_type_name_has_resource_boundary(return_type, resource_types)
    }) {
        reasons.push("resource return type".to_string());
    }
    if contract.returns_fresh {
        reasons.push("returns fresh value".to_string());
    }
    for effect in &contract.effects {
        if effect.starts_with("retains(") {
            reasons.push(effect.clone());
        } else if matches!(effect.as_str(), "native" | "unsafe") {
            reasons.push(format!("{effect} boundary"));
        } else if matches!(
            effect.as_str(),
            "no_panic" | "noalloc" | "no_block" | "pure"
        ) {
            reasons.push(format!("guarantee `{effect}`"));
        }
    }
    let classification = if package_review_map_function_is_unknown(&contract.name, review_map) {
        reasons.push("unknown review-map region".to_string());
        "unknown"
    } else {
        "review_if_changed"
    };
    reasons.sort();
    reasons.dedup();
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "function".to_string(),
        classification: classification.to_string(),
        reasons,
    }
}

fn package_review_map_function_is_unknown(function: &str, review_map: &ReviewMap) -> bool {
    review_map.files.iter().any(|file| {
        file.regions.iter().any(|region| {
            region.function == function && region.classification == ReviewMapClassification::Unknown
        })
    })
}

fn package_contract_has_resource_boundary(
    contract: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
) -> bool {
    contract
        .params
        .iter()
        .any(|param| package_type_name_has_resource_boundary(&param.type_name, resource_types))
        || contract.return_type.as_ref().is_some_and(|return_type| {
            package_type_name_has_resource_boundary(return_type, resource_types)
        })
}

fn package_type_name_has_resource_boundary(
    type_name: &str,
    resource_types: &BTreeSet<&str>,
) -> bool {
    type_name
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == '.')
        })
        .any(|part| part == "ResourcePool" || resource_types.contains(part))
}

fn package_type_contract(type_decl: &TypeDecl) -> PackageTypeContract {
    PackageTypeContract {
        name: type_decl.name.clone(),
        kind: type_decl.kind,
        type_params: type_decl
            .type_params
            .iter()
            .map(package_generic_contract)
            .collect(),
        fields: type_decl
            .fields
            .iter()
            .map(package_field_contract)
            .collect(),
        span: type_decl.span.clone(),
    }
}

fn package_function_contract(function: &FunctionDecl) -> PackageFunctionContract {
    PackageFunctionContract {
        name: function.name.clone(),
        params: function.params.iter().map(package_param_contract).collect(),
        return_type: function.return_ty.as_ref().map(package_type_name),
        returns_fresh: function.returns_fresh,
        effects: function.effects.iter().map(package_effect_name).collect(),
        span: function.span.clone(),
    }
}

fn package_generic_contract(param: &GenericParam) -> PackageGenericContract {
    PackageGenericContract {
        name: param.name.clone(),
        bound: param.bound.map(package_generic_bound_label),
    }
}

fn package_generic_bound_label(bound: GenericBound) -> &'static str {
    match bound {
        GenericBound::Managed => "Managed",
        GenericBound::Struct => "Struct",
        GenericBound::Resource => "Resource",
    }
}

fn package_field_contract(field: &FieldDecl) -> PackageFieldContract {
    PackageFieldContract {
        name: field.name.clone(),
        type_name: package_type_name(&field.ty),
        is_handle: field.is_handle,
        is_weak: field.is_weak,
    }
}

fn package_param_contract(param: &Param) -> PackageParamContract {
    PackageParamContract {
        name: param.name.clone(),
        effect: param.effect.map(package_effect_label),
        type_name: package_type_name(&param.ty),
    }
}

fn package_effect_label(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn package_effect_name(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn package_type_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }
    let args = ty
        .args
        .iter()
        .map(package_type_name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}<{args}>", ty.name)
}

fn package_type_kind_label(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Resource => "resource",
    }
}

fn package_type_contract_label(contract: &PackageTypeContract) -> String {
    let type_params = if contract.type_params.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            contract
                .type_params
                .iter()
                .map(|param| match param.bound {
                    Some(bound) => format!("{}: {bound}", param.name),
                    None => param.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let fields = if contract.fields.is_empty() {
        "<none>".to_string()
    } else {
        contract
            .fields
            .iter()
            .map(|field| {
                if field.is_weak {
                    format!("{}: weak {}", field.name, field.type_name)
                } else if field.is_handle {
                    format!("{}: handle {}", field.name, field.type_name)
                } else {
                    format!("{}: {}", field.name, field.type_name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{} {}{type_params} {{ {fields} }}",
        package_type_kind_label(contract.kind),
        contract.name
    )
}

fn package_function_contract_label(contract: &PackageFunctionContract) -> String {
    let params = contract
        .params
        .iter()
        .map(|param| match param.effect {
            Some(effect) => format!("{}: {} {}", param.name, effect, param.type_name),
            None => format!("{}: {}", param.name, param.type_name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut label = format!("pub fn {}({params})", contract.name);
    if let Some(return_type) = &contract.return_type {
        if contract.returns_fresh {
            label.push_str(&format!(" -> fresh {return_type}"));
        } else {
            label.push_str(&format!(" -> {return_type}"));
        }
    }
    if !contract.effects.is_empty() {
        label.push_str(&format!(
            " effects({})",
            contract
                .effects
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    label
}

fn dedup_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.code.clone(),
            diagnostic.summary.clone(),
            diagnostic.span.file.clone(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.span.length,
        ))
    });
}

fn package_review_metadata_from_review(review: &PackageReview) -> PackageReviewMetadata {
    PackageReviewMetadata {
        schema: "rss.review.package.v1".to_string(),
        package: review.package.clone(),
        risk: review.risk,
        reasons: review.reasons.clone(),
        summary: review.summary.clone(),
        files: review.files.clone(),
        exports: review.exports.clone(),
        native_rust: review.native_rust.clone(),
        review_map: review.review_map.clone(),
        diagnostics: review.diagnostics.clone(),
    }
}

fn check_package_lock(
    package_dir: &Path,
    current_lock: &PackageLock,
) -> Result<PackageCheckLock, String> {
    let lock_path = package_dir.join("rsspkg.lock");
    if !lock_path.exists() {
        return Ok(PackageCheckLock {
            path: lock_path.display().to_string(),
            present: false,
            matches: false,
            risk: PackageRisk::Elevated,
            reasons: vec!["rsspkg.lock missing".to_string()],
            package_changes: Vec::new(),
        });
    }

    let locked = read_package_lock(&lock_path)?;
    let package_changes = compare_locked_packages(&locked.packages, &current_lock.packages);
    let mut reasons = package_lock_diff_reasons(&package_changes);
    if locked.version != current_lock.version {
        reasons.push("lockfile format version changed".to_string());
    }
    reasons.sort();
    reasons.dedup();
    let mut risk = package_changes
        .iter()
        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
    if locked.version != current_lock.version {
        risk = risk.max(PackageRisk::Elevated);
    }

    Ok(PackageCheckLock {
        path: lock_path.display().to_string(),
        present: true,
        matches: reasons.is_empty(),
        risk,
        reasons,
        package_changes,
    })
}

fn check_package_graph(package_dir: &Path) -> Result<PackageGraphCheck, String> {
    let tree = package_tree(package_dir)?;
    let mut packages_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    collect_package_graph_identities(&tree.root, &mut packages_by_name);

    let mut reasons = Vec::new();
    if tree.summary.unresolved_dependencies > 0 {
        reasons.push(format!(
            "dependency graph contains {} unresolved dependencies",
            tree.summary.unresolved_dependencies
        ));
    }
    for (name, identities) in packages_by_name {
        if identities.len() > 1 {
            reasons.push(format!(
                "dependency `{name}` resolves to multiple package identities: {}",
                identities.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    reasons.sort();
    reasons.dedup();

    let ok = reasons.is_empty();
    let risk = if ok {
        PackageRisk::Low
    } else if tree.summary.unresolved_dependencies > 0 {
        PackageRisk::Unknown
    } else {
        PackageRisk::High
    };

    Ok(PackageGraphCheck { ok, risk, reasons })
}

fn collect_package_graph_identities(
    node: &PackageTreeNode,
    packages_by_name: &mut BTreeMap<String, BTreeSet<String>>,
) {
    if let Some(version) = &node.version {
        packages_by_name
            .entry(node.name.clone())
            .or_default()
            .insert(format!("{version} {}", node.source));
    }
    for dependency in &node.dependencies {
        collect_package_graph_identities(dependency, packages_by_name);
    }
}

fn check_package_native_rust(
    package_dir: &Path,
    native: Option<&PackageNativeRustReview>,
) -> Result<Option<PackageNativeRustCheck>, String> {
    let Some(native) = native else {
        return Ok(None);
    };
    let native_root = package_dir.join(&native.path);
    let cargo_toml = native_root.join("Cargo.toml");
    let cargo_toml_present = cargo_toml.exists();
    let mut files = Vec::new();
    if native_root.exists() {
        collect_regular_files(&native_root, &mut files)?;
    }
    let unsafe_detected = native_rust_unsafe_detected(&files)?;
    let build_risk = native_build_script_risks(&files)?;
    let mut reasons = Vec::new();
    if !native_root.exists() {
        reasons.push("native Rust path missing".to_string());
    }
    if !cargo_toml_present {
        reasons.push("native Rust Cargo.toml missing".to_string());
    }
    if native
        .crate_name
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        reasons.push("native Rust crate name missing".to_string());
    }
    if files.is_empty() {
        reasons.push("native Rust source files missing".to_string());
    }
    if unsafe_detected && native.unsafe_policy.as_deref() == Some("forbid") {
        reasons.push("native Rust unsafe usage detected".to_string());
    }
    if native.build_scripts.as_deref() == Some("forbid") {
        if build_risk.env_detected {
            reasons.push("native Rust build script reads environment".to_string());
        }
        if build_risk.download_detected {
            reasons.push("native Rust build script may download code".to_string());
        }
    }
    let metadata = if cargo_toml_present {
        scan_native_cargo_metadata(&cargo_toml, native, &mut reasons)?
    } else {
        NativeCargoMetadataScan::default()
    };
    let ok = reasons.is_empty();
    let risk = if ok {
        PackageRisk::Elevated
    } else {
        PackageRisk::High
    };

    Ok(Some(PackageNativeRustCheck {
        path: native.path.clone(),
        cargo_toml_present,
        cargo_metadata_ok: metadata.ok,
        cargo_package_name: metadata.package_name,
        target_kinds: metadata.target_kinds,
        unsafe_detected,
        linked_libraries: native.links.clone(),
        build_env_detected: build_risk.env_detected,
        build_download_detected: build_risk.download_detected,
        file_count: files.len(),
        ok,
        risk,
        reasons,
    }))
}

#[derive(Debug, Default)]
struct NativeCargoMetadataScan {
    ok: bool,
    package_name: Option<String>,
    target_kinds: Vec<String>,
}

fn scan_native_cargo_metadata(
    cargo_toml: &Path,
    native: &PackageNativeRustReview,
    reasons: &mut Vec<String>,
) -> Result<NativeCargoMetadataScan, String> {
    let native_root = cargo_toml
        .parent()
        .ok_or_else(|| format!("native Cargo.toml has no parent: {}", cargo_toml.display()))?;
    let scan_root = native_cargo_metadata_temp_dir(cargo_toml);
    if scan_root.exists() {
        let _ = fs::remove_dir_all(&scan_root);
    }
    copy_package_directory(native_root, &scan_root)?;
    let scan_cargo_toml = scan_root.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .arg("--manifest-path")
        .arg(&scan_cargo_toml)
        .output()
        .map_err(|error| {
            format!(
                "failed to run cargo metadata for {}: {error}",
                cargo_toml.display()
            )
        })?;
    let _ = fs::remove_dir_all(&scan_root);
    if !output.status.success() {
        reasons.push("native Rust cargo metadata failed".to_string());
        return Ok(NativeCargoMetadataScan::default());
    }

    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "failed to parse cargo metadata for {}: {error}",
            cargo_toml.display()
        )
    })?;
    let Some(package) = metadata.packages.first() else {
        reasons.push("native Rust cargo metadata reported no packages".to_string());
        return Ok(NativeCargoMetadataScan::default());
    };

    if let Some(expected) = native.crate_name.as_deref().map(str::trim)
        && !expected.is_empty()
        && expected != package.name
    {
        reasons.push(format!(
            "native Rust crate name `{expected}` does not match Cargo package `{}`",
            package.name
        ));
    }

    let mut target_kinds = package
        .targets
        .iter()
        .flat_map(|target| target.kind.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    target_kinds.sort();

    if target_kinds.iter().any(|kind| kind == "custom-build")
        && native.build_scripts.as_deref() == Some("forbid")
    {
        reasons.push("native Rust build script target present".to_string());
    }
    if target_kinds.iter().any(|kind| kind == "proc-macro")
        && native.proc_macros.as_deref() == Some("forbid")
    {
        reasons.push("native Rust proc macro target present".to_string());
    }

    Ok(NativeCargoMetadataScan {
        ok: true,
        package_name: Some(package.name.clone()),
        target_kinds,
    })
}

fn native_cargo_metadata_temp_dir(cargo_toml: &Path) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let name = cargo_toml
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("native");
    env::temp_dir().join(format!(
        "rsscript-native-metadata-{name}-{}-{now}",
        std::process::id()
    ))
}

fn native_rust_unsafe_detected(files: &[PathBuf]) -> Result<bool, String> {
    for file in files {
        if file.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        if source_contains_rust_unsafe_keyword(&source) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Debug, Default)]
struct NativeBuildScriptRisk {
    env_detected: bool,
    download_detected: bool,
}

fn native_build_script_risks(files: &[PathBuf]) -> Result<NativeBuildScriptRisk, String> {
    let mut risk = NativeBuildScriptRisk::default();
    for file in files {
        if file.file_name().and_then(|name| name.to_str()) != Some("build.rs") {
            continue;
        }
        let source = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let stripped = source_without_rust_comments(&source);
        if build_script_reads_environment(&stripped) {
            risk.env_detected = true;
        }
        if build_script_may_download_code(&stripped) {
            risk.download_detected = true;
        }
    }
    Ok(risk)
}

fn build_script_reads_environment(source: &str) -> bool {
    [
        "env::var",
        "env::var_os",
        "std::env::var",
        "std::env::var_os",
        "env!(",
        "option_env!(",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

fn build_script_may_download_code(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    [
        "http://",
        "https://",
        "reqwest",
        "ureq",
        "curl",
        "wget",
        "git clone",
        "git2",
        "tcpstream",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn source_without_rust_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            _ => {
                out.push(bytes[index] as char);
                index += 1;
            }
        }
    }
    out
}

fn source_contains_rust_unsafe_keyword(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            b'"' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == b'"' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'\'' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == b'\'' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            byte if is_rust_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_rust_ident_continue(bytes[index]) {
                    index += 1;
                }
                if &source[start..index] == "unsafe" {
                    return true;
                }
            }
            _ => index += 1,
        }
    }
    false
}

fn is_rust_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_rust_ident_continue(byte: u8) -> bool {
    is_rust_ident_start(byte) || byte.is_ascii_digit()
}

fn package_tree_node(
    package_dir: &Path,
    dependency_kind: PackageDependencyKind,
    visiting: &mut BTreeSet<String>,
) -> Result<PackageTreeNode, String> {
    let package = load_package(package_dir)?;
    let review = review_package_dir(package_dir)?;
    let identity = package_identity(&package.manifest);
    let visit_key = canonical_path_label(package_dir);
    if !visiting.insert(visit_key.clone()) {
        return Ok(PackageTreeNode {
            name: identity.name,
            version: Some(identity.version),
            requirement: None,
            source: format!("path+{}", package_dir.display()),
            risk: PackageRisk::Elevated,
            features: package.manifest.features.keys().cloned().collect(),
            native: review.native_rust.is_some(),
            dependency_kind,
            reasons: vec!["dependency cycle truncated".to_string()],
            dependencies: Vec::new(),
        });
    }

    let mut dependencies = Vec::new();
    dependencies.extend(package_tree_dependencies(
        package_dir,
        &package.manifest.dependencies,
        PackageDependencyKind::Normal,
        visiting,
    )?);
    dependencies.extend(package_tree_dependencies(
        package_dir,
        &package.manifest.dev_dependencies,
        PackageDependencyKind::Dev,
        visiting,
    )?);
    visiting.remove(&visit_key);

    Ok(PackageTreeNode {
        name: identity.name,
        version: Some(identity.version),
        requirement: None,
        source: format!("path+{}", package_dir.display()),
        risk: review.risk,
        features: package.manifest.features.keys().cloned().collect(),
        native: review.native_rust.is_some(),
        dependency_kind,
        reasons: review.reasons,
        dependencies,
    })
}

fn package_tree_dependencies(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    dependency_kind: PackageDependencyKind,
    visiting: &mut BTreeSet<String>,
) -> Result<Vec<PackageTreeNode>, String> {
    let mut nodes = Vec::new();
    for (name, value) in dependencies {
        let spec = package_dependency_spec(name, value);
        if let Some(path) = &spec.path {
            let dependency_dir = package_dir.join(path);
            if dependency_dir.join("rsspkg.toml").exists() {
                let mut node = package_tree_node(&dependency_dir, dependency_kind, visiting)?;
                node.requirement = spec.requirement.clone();
                node.features = spec.features.clone();
                node.source = format!("path+{}", dependency_dir.display());
                nodes.push(node);
            } else {
                nodes.push(unresolved_dependency_node(
                    spec,
                    dependency_kind,
                    vec!["path dependency manifest missing".to_string()],
                ));
            }
        } else {
            nodes.push(unresolved_dependency_node(
                spec,
                dependency_kind,
                vec!["dependency resolver not implemented for this source".to_string()],
            ));
        }
    }
    Ok(nodes)
}

fn collect_lock_dependency_packages(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    visiting: &mut BTreeSet<String>,
    packages: &mut Vec<PackageLockPackage>,
) -> Result<(), String> {
    for (name, value) in dependencies {
        let spec = package_dependency_spec(name, value);
        let Some(path) = &spec.path else {
            continue;
        };
        let dependency_dir = package_dir.join(path);
        if !dependency_dir.join("rsspkg.toml").exists() {
            continue;
        }
        let canonical = canonical_path_label(&dependency_dir);
        if !visiting.insert(canonical.clone()) {
            continue;
        }
        let dependency_package = load_package(&dependency_dir)?;
        packages.push(lock_package_entry(
            &dependency_dir,
            &dependency_package,
            spec.features,
        )?);
        collect_lock_dependency_packages(
            &dependency_dir,
            &dependency_package.manifest.dependencies,
            visiting,
            packages,
        )?;
        collect_lock_dependency_packages(
            &dependency_dir,
            &dependency_package.manifest.dev_dependencies,
            visiting,
            packages,
        )?;
        visiting.remove(&canonical);
    }
    Ok(())
}

fn collect_vendor_dependencies(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    vendor_dir: &Path,
    visiting: &mut BTreeSet<String>,
    entries: &mut Vec<PackageVendorEntry>,
    unresolved: &mut Vec<PackageVendorUnresolved>,
) -> Result<(), String> {
    for (name, value) in dependencies {
        let spec = package_dependency_spec(name, value);
        let Some(path) = &spec.path else {
            unresolved.push(PackageVendorUnresolved {
                name: spec.name,
                requirement: spec.requirement,
                source: if let Some(git) = spec.git {
                    format!("git+{git}")
                } else {
                    "registry".to_string()
                },
                reason: "dependency resolver not implemented for this source".to_string(),
            });
            continue;
        };

        let dependency_dir = package_dir.join(path);
        if !dependency_dir.join("rsspkg.toml").exists() {
            unresolved.push(PackageVendorUnresolved {
                name: spec.name,
                requirement: spec.requirement,
                source: format!("path+{}", dependency_dir.display()),
                reason: "path dependency manifest missing".to_string(),
            });
            continue;
        }

        let dependency_package = load_package(&dependency_dir)?;
        let identity = package_identity(&dependency_package.manifest);
        let canonical = canonical_path_label(&dependency_dir);
        let vendor_path = vendor_dir.join(vendor_package_dir_name(&identity));
        let native = dependency_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref());
        let native_hash = package_native_hash(&dependency_dir, native)?;
        entries.push(PackageVendorEntry {
            name: identity.name.clone(),
            version: identity.version.clone(),
            source_path: dependency_dir.display().to_string(),
            vendor_path: vendor_path.display().to_string(),
            checksum: package_checksum(&dependency_package, native_hash.as_deref()),
            native: native.is_some_and(|native| native.enabled),
        });

        if visiting.insert(canonical.clone()) {
            collect_vendor_dependencies(
                &dependency_dir,
                &dependency_package.manifest.dependencies,
                vendor_dir,
                visiting,
                entries,
                unresolved,
            )?;
            visiting.remove(&canonical);
        }
    }
    Ok(())
}

fn package_dependency_spec(name: &str, value: &toml::Value) -> PackageDependencySpec {
    if let Some(requirement) = value.as_str() {
        return PackageDependencySpec {
            name: name.to_string(),
            requirement: Some(requirement.to_string()),
            path: None,
            git: None,
            features: Vec::new(),
        };
    }
    let Some(table) = value.as_table() else {
        return PackageDependencySpec {
            name: name.to_string(),
            requirement: Some(toml_value_label(value)),
            path: None,
            git: None,
            features: Vec::new(),
        };
    };
    PackageDependencySpec {
        name: name.to_string(),
        requirement: table
            .get("version")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        path: table
            .get("path")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        git: table
            .get("git")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        features: table
            .get("features")
            .and_then(toml::Value::as_array)
            .map(|features| {
                features
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn unresolved_dependency_node(
    spec: PackageDependencySpec,
    dependency_kind: PackageDependencyKind,
    reasons: Vec<String>,
) -> PackageTreeNode {
    let source = if let Some(path) = &spec.path {
        format!("path+{path}")
    } else if let Some(git) = &spec.git {
        format!("git+{git}")
    } else {
        "registry".to_string()
    };
    PackageTreeNode {
        name: spec.name,
        version: None,
        requirement: spec.requirement,
        source,
        risk: PackageRisk::Unknown,
        features: spec.features,
        native: false,
        dependency_kind,
        reasons,
        dependencies: Vec::new(),
    }
}

fn publish_check(
    name: impl Into<String>,
    ok: bool,
    risk: PackageRisk,
    detail: impl Into<String>,
) -> PackagePublishCheck {
    PackagePublishCheck {
        name: name.into(),
        ok,
        risk,
        detail: detail.into(),
    }
}

fn package_registry_index_entry(
    package: &LoadedPackage,
    lock_entry: &PackageLockPackage,
    native_check: Option<&PackageNativeRustCheck>,
    risk: PackageRisk,
    archive_hash: &str,
) -> PackageRegistryIndexEntry {
    PackageRegistryIndexEntry {
        schema: "rss.registry.index.v1".to_string(),
        name: package.manifest.package.name.clone(),
        version: package.manifest.package.version.clone(),
        checksum: archive_hash.to_string(),
        interface_hash: lock_entry.interface_hash.clone(),
        review_hash: lock_entry.review_hash.clone(),
        native_hash: lock_entry.native_hash.clone(),
        risk,
        native: package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref())
            .is_some_and(|native| native.enabled),
        unsafe_boundary: package_index_unsafe_boundary(&package.manifest, native_check),
        dependencies: package_index_dependencies(&package.manifest.dependencies),
    }
}

fn package_registry_publish_target(
    registry_dir: &Path,
    package_name: &str,
    package_version: &str,
) -> PackageRegistryPublishTarget {
    let package_component = sanitize_vendor_path_component(package_name);
    let version_component = sanitize_vendor_path_component(package_version);
    PackageRegistryPublishTarget {
        registry_dir: registry_dir.display().to_string(),
        index_path: registry_dir
            .join("index")
            .join(&package_component)
            .join(format!("{version_component}.json"))
            .display()
            .to_string(),
        archive_manifest_path: registry_dir
            .join("archives")
            .join(&package_component)
            .join(&version_component)
            .join("archive-manifest.json")
            .display()
            .to_string(),
    }
}

fn package_index_unsafe_boundary(
    manifest: &Manifest,
    native_check: Option<&PackageNativeRustCheck>,
) -> bool {
    manifest
        .review
        .as_ref()
        .and_then(|review| review.allow_unsafe)
        .unwrap_or(false)
        || manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref())
            .and_then(|native| native.unsafe_policy.as_deref())
            .is_some_and(|policy| policy != "forbid")
        || native_check.is_some_and(|native| native.unsafe_detected)
}

fn package_index_dependencies(
    dependencies: &BTreeMap<String, toml::Value>,
) -> BTreeMap<String, String> {
    dependencies
        .iter()
        .map(|(name, value)| {
            let spec = package_dependency_spec(name, value);
            let requirement = spec
                .requirement
                .or_else(|| spec.git.map(|git| format!("git+{git}")))
                .or_else(|| spec.path.map(|path| format!("path+{path}")))
                .unwrap_or_else(|| "*".to_string());
            (spec.name, requirement)
        })
        .collect()
}

fn is_semver_like(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

fn vendor_package_dir_name(identity: &PackageIdentity) -> String {
    format!(
        "{}-{}",
        sanitize_vendor_path_component(&identity.name),
        sanitize_vendor_path_component(&identity.version)
    )
}

fn sanitize_vendor_path_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn canonical_path_label(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn collect_package_tree_summary(node: &PackageTreeNode, summary: &mut PackageTreeSummary) {
    summary.packages += 1;
    if node.source.starts_with("path+") && node.dependency_kind != PackageDependencyKind::Root {
        summary.path_dependencies += 1;
    }
    if node.risk == PackageRisk::Unknown {
        summary.unknown_risk_packages += 1;
    }
    if node.risk == PackageRisk::High {
        summary.high_risk_packages += 1;
    }
    if node.version.is_none() {
        summary.unresolved_dependencies += 1;
    }
    if node.native {
        summary.native_packages += 1;
    }
    for dependency in &node.dependencies {
        collect_package_tree_summary(dependency, summary);
    }
}

fn format_package_tree_node_human(
    node: &PackageTreeNode,
    prefix: &str,
    is_last: bool,
    output: &mut String,
) {
    let connector = if node.dependency_kind == PackageDependencyKind::Root {
        ""
    } else if is_last {
        "`-- "
    } else {
        "|-- "
    };
    output.push_str(prefix);
    output.push_str(connector);
    output.push_str(&node.name);
    if let Some(version) = &node.version {
        output.push(' ');
        output.push_str(version);
    }
    if let Some(requirement) = &node.requirement {
        output.push_str(" req ");
        output.push_str(requirement);
    }
    output.push_str(" [");
    output.push_str(package_risk_label(node.risk));
    if node.native {
        output.push_str(", native");
    }
    if !node.features.is_empty() {
        output.push_str(", features ");
        output.push_str(&node.features.join(","));
    }
    output.push_str("]\n");

    let child_prefix = if node.dependency_kind == PackageDependencyKind::Root {
        String::new()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}|   ")
    };
    for (index, dependency) in node.dependencies.iter().enumerate() {
        format_package_tree_node_human(
            dependency,
            &child_prefix,
            index + 1 == node.dependencies.len(),
            output,
        );
    }
}

fn is_rsscript_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
}

fn package_checksum(package: &LoadedPackage, native_hash: Option<&str>) -> String {
    let mut input = String::new();
    input.push_str("manifest\n");
    input.push_str(&package.manifest_source);
    input.push_str("\nsources\n");
    append_sources_hash_input(&mut input, &package.sources);
    if let Some(native_hash) = native_hash {
        input.push_str("\nnative\n");
        input.push_str(native_hash);
    }
    sha256_label(input.as_bytes())
}

fn package_archive_files(package_dir: &Path) -> Result<Vec<PackageArchiveFile>, String> {
    let mut paths = Vec::new();
    collect_package_archive_paths(package_dir, package_dir, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| package_archive_file(package_dir, &path))
        .collect()
}

fn collect_package_archive_paths(
    root: &Path,
    path: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        if should_skip_archive_entry(root, &path, &name.to_string_lossy()) {
            continue;
        }
        if path.is_dir() || path.is_file() {
            collect_package_archive_paths(root, &path, files)?;
        }
    }
    Ok(())
}

fn should_skip_archive_entry(root: &Path, path: &Path, name: &str) -> bool {
    if matches!(name, ".git" | "target" | "vendor" | ".DS_Store") {
        return true;
    }
    let relative = relative_path(root, path);
    matches!(
        relative.as_str(),
        "review/package-review.json" | "vendor/rss-vendor.json"
    )
}

fn package_archive_file(root: &Path, path: &Path) -> Result<PackageArchiveFile, String> {
    let contents =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(PackageArchiveFile {
        path: relative_path(root, path),
        size: contents.len() as u64,
        sha256: sha256_label(&contents),
    })
}

fn package_archive_hash(files: &[PackageArchiveFile]) -> String {
    let mut input = String::new();
    input.push_str("rss.package.archive.v1\n");
    for file in files {
        input.push_str(&file.path);
        input.push('\n');
        input.push_str(&file.size.to_string());
        input.push('\n');
        input.push_str(&file.sha256);
        input.push('\n');
    }
    sha256_label(input.as_bytes())
}

fn hash_sources(sources: &[PackageSource], kind: PackageReviewFileKind) -> String {
    let filtered = sources
        .iter()
        .filter(|source| source.kind == kind)
        .cloned()
        .collect::<Vec<_>>();
    let mut input = String::new();
    append_sources_hash_input(&mut input, &filtered);
    sha256_label(input.as_bytes())
}

fn append_sources_hash_input(input: &mut String, sources: &[PackageSource]) {
    let mut sources = sources.iter().collect::<Vec<_>>();
    sources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    for source in sources {
        input.push_str(&source.relative_path);
        input.push('\n');
        input.push_str(&source.contents);
        input.push('\n');
    }
}

fn package_review_hash(review: &PackageReview) -> String {
    let mut input = String::new();
    input.push_str(&review.package.name);
    input.push('\n');
    input.push_str(&review.package.version);
    input.push('\n');
    input.push_str(&review.package.edition);
    input.push('\n');
    input.push_str(package_risk_label(review.risk));
    input.push('\n');
    for reason in &review.reasons {
        input.push_str(reason);
        input.push('\n');
    }
    input.push_str(&format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.diagnostics,
        review.summary.errors,
        review.summary.dependencies,
        review.summary.dev_dependencies,
        review.summary.package_features,
        review.summary.public_types,
        review.summary.public_functions,
        review.summary.public_apis,
        review.summary.mutating_apis,
        review.summary.retaining_apis,
        review.summary.resource_apis,
        review.summary.fresh_returning_apis,
        review.summary.native_apis,
        review.summary.unsafe_apis,
        review.summary.unknown_apis
    ));
    for export in &review.exports {
        input.push_str(&export.kind);
        input.push('\n');
        input.push_str(&export.name);
        input.push('\n');
        input.push_str(&export.classification);
        input.push('\n');
        for reason in &export.reasons {
            input.push_str(reason);
            input.push('\n');
        }
    }
    if let Some(native) = &review.native_rust {
        input.push_str(&native.path);
        input.push('\n');
        input.push_str(native.crate_name.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(native.build_scripts.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(native.proc_macros.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(native.unsafe_policy.as_deref().unwrap_or(""));
        input.push('\n');
        for link in &native.links {
            input.push_str(link);
            input.push('\n');
        }
    }
    for diagnostic in &review.diagnostics {
        input.push_str(&diagnostic.code);
        input.push('\n');
        input.push_str(&diagnostic.summary);
        input.push('\n');
    }
    sha256_label(input.as_bytes())
}

fn package_native_hash(
    package_dir: &Path,
    native: Option<&ManifestNativeRust>,
) -> Result<Option<String>, String> {
    let Some(native) = native.filter(|native| native.enabled) else {
        return Ok(None);
    };
    let native_root = package_dir.join(native.path.as_deref().unwrap_or("native/rust"));
    let mut input = String::new();
    input.push_str(native.path.as_deref().unwrap_or("native/rust"));
    input.push('\n');
    input.push_str(native.crate_name.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.build_scripts.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.proc_macros.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.unsafe_policy.as_deref().unwrap_or(""));
    input.push('\n');
    for link in &native.links {
        input.push_str(link);
        input.push('\n');
    }

    if native_root.exists() {
        let mut files = Vec::new();
        collect_regular_files(&native_root, &mut files)?;
        files.sort();
        for file in files {
            input.push_str(&relative_path(package_dir, &file));
            input.push('\n');
            let contents = fs::read(&file)
                .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
            input.push_str(&sha256_label(&contents));
            input.push('\n');
        }
    }
    let binding_manifest = package_dir.join("native/bindings.rssbind.toml");
    if binding_manifest.exists() {
        input.push_str(&relative_path(package_dir, &binding_manifest));
        input.push('\n');
        let contents = fs::read(&binding_manifest)
            .map_err(|error| format!("failed to read {}: {error}", binding_manifest.display()))?;
        input.push_str(&sha256_label(&contents));
        input.push('\n');
    }

    Ok(Some(sha256_label(input.as_bytes())))
}

fn sha256_label(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::from("sha256:");
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn collect_manifest_review_reasons(manifest: &Manifest, reasons: &mut Vec<String>) {
    if !manifest.features.is_empty() {
        reasons.push("package declares selectable package features".to_string());
    }
    if let Some(review) = &manifest.review {
        if review.unknown_is_error == Some(true) {
            reasons.push("unknown package risk is configured as an error".to_string());
        }
        if review.allow_native == Some(true) {
            reasons.push("manifest allows native boundaries".to_string());
        }
        if review.allow_unsafe == Some(true) {
            reasons.push("manifest allows unsafe boundaries".to_string());
        }
    }
}

fn collect_native_reasons(native: Option<&PackageNativeRustReview>, reasons: &mut Vec<String>) {
    let Some(native) = native else {
        return;
    };
    reasons.push("native Rust wrapper enabled".to_string());
    if native
        .build_scripts
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust build scripts require review".to_string());
    }
    if native
        .proc_macros
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust proc macros require review".to_string());
    }
    if native
        .unsafe_policy
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust unsafe policy requires review".to_string());
    }
    if !native.links.is_empty() {
        reasons.push("native Rust links external libraries".to_string());
    }
}

fn collect_review_map_reasons(review_map: &ReviewMap, reasons: &mut Vec<String>) {
    if review_map.summary.unknown.functions > 0 {
        reasons.push("review map contains unknown functions".to_string());
    }
    if review_map.summary.review_required.functions > 0 {
        reasons.push("review map contains must-review functions".to_string());
    }
}

fn package_identity(manifest: &Manifest) -> PackageIdentity {
    PackageIdentity {
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        edition: manifest.package.edition.clone(),
    }
}

fn compare_package_identity(
    old: &Manifest,
    new: &Manifest,
    changes: &mut Vec<PackageManifestChange>,
) {
    if old.package.name != new.package.name {
        changes.push(manifest_change(
            "package",
            "name",
            Some(old.package.name.clone()),
            Some(new.package.name.clone()),
            PackageRisk::High,
        ));
    }
    if old.package.version != new.package.version {
        changes.push(manifest_change(
            "package",
            "version",
            Some(old.package.version.clone()),
            Some(new.package.version.clone()),
            PackageRisk::Elevated,
        ));
    }
    if old.package.edition != new.package.edition {
        changes.push(manifest_change(
            "package",
            "edition",
            Some(old.package.edition.clone()),
            Some(new.package.edition.clone()),
            PackageRisk::Elevated,
        ));
    }
}

fn compare_value_maps(
    kind: &str,
    old: &BTreeMap<String, toml::Value>,
    new: &BTreeMap<String, toml::Value>,
    risk: PackageRisk,
    changes: &mut Vec<PackageManifestChange>,
) {
    let names = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.get(&name).map(toml_value_label);
        let after = new.get(&name).map(toml_value_label);
        if before != after {
            changes.push(manifest_change(kind, name, before, after, risk));
        }
    }
}

fn compare_feature_maps(
    old: &BTreeMap<String, Vec<String>>,
    new: &BTreeMap<String, Vec<String>>,
    changes: &mut Vec<PackageManifestChange>,
) {
    let names = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.get(&name).map(|values| feature_values_label(values));
        let after = new.get(&name).map(|values| feature_values_label(values));
        if before != after {
            changes.push(manifest_change(
                "package-feature",
                name,
                before,
                after,
                PackageRisk::Elevated,
            ));
        }
    }
}

fn compare_native_rust(
    old: Option<&ManifestNativeRust>,
    new: Option<&ManifestNativeRust>,
    changes: &mut Vec<PackageManifestChange>,
) {
    let old_enabled = old.is_some_and(|native| native.enabled);
    let new_enabled = new.is_some_and(|native| native.enabled);
    if old_enabled != new_enabled {
        changes.push(manifest_change(
            "native-rust",
            "enabled",
            Some(old_enabled.to_string()),
            Some(new_enabled.to_string()),
            PackageRisk::High,
        ));
    }
    compare_optional_native_field(
        "path",
        old.and_then(|native| native.path.as_deref()),
        new.and_then(|native| native.path.as_deref()),
        PackageRisk::Elevated,
        changes,
    );
    compare_optional_native_field(
        "crate",
        old.and_then(|native| native.crate_name.as_deref()),
        new.and_then(|native| native.crate_name.as_deref()),
        PackageRisk::Elevated,
        changes,
    );
    compare_optional_native_field(
        "build_scripts",
        old.and_then(|native| native.build_scripts.as_deref()),
        new.and_then(|native| native.build_scripts.as_deref()),
        PackageRisk::High,
        changes,
    );
    compare_optional_native_field(
        "proc_macros",
        old.and_then(|native| native.proc_macros.as_deref()),
        new.and_then(|native| native.proc_macros.as_deref()),
        PackageRisk::High,
        changes,
    );
    compare_optional_native_field(
        "unsafe",
        old.and_then(|native| native.unsafe_policy.as_deref()),
        new.and_then(|native| native.unsafe_policy.as_deref()),
        PackageRisk::High,
        changes,
    );
    let old_links = old.map(|native| native.links.join(", "));
    let new_links = new.map(|native| native.links.join(", "));
    if old_links != new_links {
        changes.push(manifest_change(
            "native-rust",
            "links",
            old_links,
            new_links,
            PackageRisk::High,
        ));
    }
}

fn compare_optional_native_field(
    name: &str,
    old: Option<&str>,
    new: Option<&str>,
    risk: PackageRisk,
    changes: &mut Vec<PackageManifestChange>,
) {
    if old != new {
        changes.push(manifest_change(
            "native-rust",
            name,
            old.map(str::to_string),
            new.map(str::to_string),
            risk,
        ));
    }
}

fn compare_interface_sources(
    old_sources: &[PackageSource],
    new_sources: &[PackageSource],
) -> Vec<PackageInterfaceChange> {
    let old_interfaces = interface_sources_by_relative_path(old_sources);
    let new_interfaces = interface_sources_by_relative_path(new_sources);
    let files = old_interfaces
        .keys()
        .chain(new_interfaces.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for file in files {
        match (old_interfaces.get(&file), new_interfaces.get(&file)) {
            (Some(old), Some(new)) if old.contents != new.contents => {
                let findings = review_sources(&old.path, &old.contents, &new.path, &new.contents);
                changes.push(PackageInterfaceChange {
                    file,
                    change: PackageInterfaceChangeKind::Modified,
                    risk: interface_change_risk(&findings),
                    findings,
                });
            }
            (None, Some(_)) => changes.push(PackageInterfaceChange {
                file,
                change: PackageInterfaceChangeKind::Added,
                risk: PackageRisk::Elevated,
                findings: Vec::new(),
            }),
            (Some(_), None) => changes.push(PackageInterfaceChange {
                file,
                change: PackageInterfaceChangeKind::Removed,
                risk: PackageRisk::High,
                findings: Vec::new(),
            }),
            _ => {}
        }
    }
    changes
}

fn compare_locked_packages(
    old_packages: &[PackageLockPackage],
    new_packages: &[PackageLockPackage],
) -> Vec<PackageLockPackageChange> {
    let old_packages = locked_packages_by_name(old_packages);
    let new_packages = locked_packages_by_name(new_packages);
    let names = old_packages
        .keys()
        .chain(new_packages.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for name in names {
        match (old_packages.get(&name), new_packages.get(&name)) {
            (None, Some(new)) => changes.push(PackageLockPackageChange {
                name,
                before_version: None,
                after_version: Some(new.version.clone()),
                risk: PackageRisk::Elevated,
                changes: vec![PackageLockFieldChange {
                    field: "package".to_string(),
                    before: None,
                    after: Some("added".to_string()),
                    risk: PackageRisk::Elevated,
                }],
            }),
            (Some(old), None) => changes.push(PackageLockPackageChange {
                name,
                before_version: Some(old.version.clone()),
                after_version: None,
                risk: PackageRisk::High,
                changes: vec![PackageLockFieldChange {
                    field: "package".to_string(),
                    before: Some("present".to_string()),
                    after: None,
                    risk: PackageRisk::High,
                }],
            }),
            (Some(old), Some(new)) => {
                let field_changes = compare_locked_package_fields(old, new);
                if !field_changes.is_empty() {
                    let risk = field_changes
                        .iter()
                        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
                    changes.push(PackageLockPackageChange {
                        name,
                        before_version: Some(old.version.clone()),
                        after_version: Some(new.version.clone()),
                        risk,
                        changes: field_changes,
                    });
                }
            }
            (None, None) => {}
        }
    }
    changes
}

fn locked_packages_by_name(
    packages: &[PackageLockPackage],
) -> BTreeMap<String, &PackageLockPackage> {
    packages
        .iter()
        .map(|package| (package.name.clone(), package))
        .collect()
}

fn compare_locked_package_fields(
    old: &PackageLockPackage,
    new: &PackageLockPackage,
) -> Vec<PackageLockFieldChange> {
    let mut changes = Vec::new();
    push_lock_field_change(
        &mut changes,
        "version",
        Some(old.version.as_str()),
        Some(new.version.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "source",
        Some(old.source.as_str()),
        Some(new.source.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "checksum",
        Some(old.checksum.as_str()),
        Some(new.checksum.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "interface_hash",
        Some(old.interface_hash.as_str()),
        Some(new.interface_hash.as_str()),
        PackageRisk::High,
    );
    push_lock_field_change(
        &mut changes,
        "review_hash",
        Some(old.review_hash.as_str()),
        Some(new.review_hash.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "native_hash",
        old.native_hash.as_deref(),
        new.native_hash.as_deref(),
        PackageRisk::High,
    );
    let old_features = feature_values_label(&old.features);
    let new_features = feature_values_label(&new.features);
    push_lock_field_change(
        &mut changes,
        "features",
        Some(old_features.as_str()),
        Some(new_features.as_str()),
        PackageRisk::Elevated,
    );
    changes
}

fn push_lock_field_change(
    changes: &mut Vec<PackageLockFieldChange>,
    field: &str,
    before: Option<&str>,
    after: Option<&str>,
    risk: PackageRisk,
) {
    if before != after {
        changes.push(PackageLockFieldChange {
            field: field.to_string(),
            before: before.map(str::to_string),
            after: after.map(str::to_string),
            risk,
        });
    }
}

fn interface_sources_by_relative_path(
    sources: &[PackageSource],
) -> BTreeMap<String, &PackageSource> {
    sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.relative_path.clone(), source))
        .collect()
}

fn package_diff_reasons(
    manifest_changes: &[PackageManifestChange],
    interface_changes: &[PackageInterfaceChange],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if manifest_changes
        .iter()
        .any(|change| change.kind == "dependency")
    {
        reasons.push("RSScript dependencies changed".to_string());
    }
    if manifest_changes
        .iter()
        .any(|change| change.kind == "package-feature")
    {
        reasons.push("package features changed".to_string());
    }
    if manifest_changes
        .iter()
        .any(|change| change.kind == "native-rust")
    {
        reasons.push("native Rust wrapper metadata changed".to_string());
    }
    if !interface_changes.is_empty() {
        reasons.push("public .rssi semantic contract changed".to_string());
    }
    if interface_changes
        .iter()
        .any(|change| change.risk == PackageRisk::High)
    {
        reasons.push("high-risk interface change detected".to_string());
    }
    reasons
}

fn package_lock_diff_reasons(changes: &[PackageLockPackageChange]) -> Vec<String> {
    let mut reasons = Vec::new();
    if changes
        .iter()
        .flat_map(|change| &change.changes)
        .any(|change| change.field == "package" && change.after.as_deref() == Some("added"))
    {
        reasons.push("RSScript package added to lockfile".to_string());
    }
    if changes
        .iter()
        .flat_map(|change| &change.changes)
        .any(|change| change.field == "package" && change.after.is_none())
    {
        reasons.push("RSScript package removed from lockfile".to_string());
    }
    if lock_field_changed(changes, "version") {
        reasons.push("RSScript package version changed".to_string());
    }
    if lock_field_changed(changes, "interface_hash") {
        reasons.push(".rssi interface hash changed".to_string());
    }
    if lock_field_changed(changes, "review_hash") {
        reasons.push("review metadata hash changed".to_string());
    }
    if lock_field_changed(changes, "native_hash") {
        reasons.push("native wrapper source hash changed".to_string());
    }
    if lock_field_changed(changes, "checksum") {
        reasons.push("package checksum changed".to_string());
    }
    if lock_field_changed(changes, "features") {
        reasons.push("package feature selection changed".to_string());
    }
    if lock_field_changed(changes, "source") {
        reasons.push("package source changed".to_string());
    }
    reasons
}

fn lock_field_changed(changes: &[PackageLockPackageChange], field: &str) -> bool {
    changes
        .iter()
        .flat_map(|change| &change.changes)
        .any(|change| change.field == field)
}

fn package_diff_risk(
    manifest_changes: &[PackageManifestChange],
    interface_changes: &[PackageInterfaceChange],
    old_risk: PackageRisk,
    new_risk: PackageRisk,
) -> PackageRisk {
    let mut risk = old_risk.max(new_risk);
    for change in manifest_changes {
        risk = risk.max(change.risk);
    }
    for change in interface_changes {
        risk = risk.max(change.risk);
    }
    risk
}

fn interface_change_risk(findings: &[ReviewFinding]) -> PackageRisk {
    if findings.iter().any(|finding| {
        matches!(
            finding.risk,
            ReviewRisk::Unsafe | ReviewRisk::Effect | ReviewRisk::Boundary | ReviewRisk::Guarantee
        )
    }) {
        PackageRisk::High
    } else if findings.is_empty() {
        PackageRisk::Low
    } else {
        PackageRisk::Elevated
    }
}

fn manifest_change(
    kind: impl Into<String>,
    name: impl Into<String>,
    before: Option<String>,
    after: Option<String>,
    risk: PackageRisk,
) -> PackageManifestChange {
    PackageManifestChange {
        kind: kind.into(),
        name: name.into(),
        before,
        after,
        risk,
    }
}

fn toml_value_label(value: &toml::Value) -> String {
    value.to_string()
}

fn feature_values_label(values: &[String]) -> String {
    if values.is_empty() {
        "[]".to_string()
    } else {
        values.join(", ")
    }
}

fn package_risk(
    manifest: &Manifest,
    native: Option<&PackageNativeRustReview>,
    review_map: &ReviewMap,
    diagnostics: &[Diagnostic],
) -> PackageRisk {
    if manifest
        .review
        .as_ref()
        .and_then(|review| review.risk.as_deref())
        == Some("unknown")
        || review_map.summary.unknown.functions > 0
    {
        return PackageRisk::Unknown;
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return PackageRisk::High;
    }
    if let Some(native) = native
        && (native
            .build_scripts
            .as_deref()
            .is_some_and(|policy| policy != "forbid")
            || native
                .proc_macros
                .as_deref()
                .is_some_and(|policy| policy != "forbid")
            || native
                .unsafe_policy
                .as_deref()
                .is_some_and(|policy| policy != "forbid")
            || !native.links.is_empty())
    {
        return PackageRisk::High;
    }
    if native.is_some() || review_map.summary.review_required.functions > 0 {
        return PackageRisk::Elevated;
    }
    PackageRisk::Low
}

fn package_risk_label(risk: PackageRisk) -> &'static str {
    match risk {
        PackageRisk::Low => "low",
        PackageRisk::Elevated => "elevated",
        PackageRisk::High => "high",
        PackageRisk::Unknown => "unknown",
    }
}
