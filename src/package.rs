use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analyzer::{analyze_source_with_interfaces, core_interfaces};
use crate::diagnostic::{Diagnostic, code};
use crate::review::{
    ReviewFinding, ReviewMap, ReviewRisk, format_review_human, review_map_sources, review_sources,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReview {
    pub package: PackageIdentity,
    pub manifest_path: String,
    pub risk: PackageRisk,
    pub reasons: Vec<String>,
    pub summary: PackageReviewSummary,
    pub files: Vec<PackageReviewFile>,
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
    pub archive_hash: String,
    pub review: PackageReviewSummary,
    pub dependency_summary: PackageTreeSummary,
    pub checks: Vec<PackagePublishCheck>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReviewFile {
    pub path: String,
    pub kind: PackageReviewFileKind,
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

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    let package = load_package(package_dir)?;
    let manifest = &package.manifest;
    let sources = &package.sources;
    let dependency_interfaces = collect_dependency_interface_sources(package_dir, manifest)?;

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
    combined_interfaces.extend(interface_refs);
    let mut diagnostics = package_interface_environment_diagnostics(&combined_interfaces);
    diagnostics.extend(
        sources
            .iter()
            .flat_map(|source| {
                if source.kind == PackageReviewFileKind::Source {
                    analyze_source_with_interfaces(
                        &source.path,
                        &source.contents,
                        &combined_interfaces,
                    )
                } else {
                    analyze_source_with_interfaces(
                        &source.path,
                        &source.contents,
                        &external_interfaces,
                    )
                }
            })
            .collect::<Vec<_>>(),
    );
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
    };
    let files = sources
        .iter()
        .map(|source| PackageReviewFile {
            path: source.path.clone(),
            kind: source.kind,
        })
        .collect();

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
    let interfaces = dependency_interfaces
        .iter()
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
    })
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
    let package = load_package(package_dir)?;
    let review = review_package_dir(package_dir)?;
    let check = check_package_dir(package_dir)?;
    let tree = package_tree(package_dir)?;
    let archive_hash = package_archive_hash(package_dir)?;

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
            archive_hash.clone(),
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

    Ok(PackagePublishDryRun {
        package: package_identity(&package.manifest),
        package_dir: package_dir.display().to_string(),
        ready,
        risk,
        reasons,
        archive_hash,
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
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} diagnostics ({} errors)\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.dependencies,
        review.summary.package_features,
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
        "summary: {} interface files; {} source files; {} diagnostics ({} errors)\n",
        metadata.metadata.summary.interface_files,
        metadata.metadata.summary.source_files,
        metadata.metadata.summary.diagnostics,
        metadata.metadata.summary.errors
    ));
    for reason in &metadata.reasons {
        output.push_str(&format!("reason: {reason}\n"));
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
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} diagnostics ({} errors)\n",
        check.summary.interface_files,
        check.summary.source_files,
        check.summary.dependencies,
        check.summary.package_features,
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
            "native rust: {} cargo_toml={} files={}\n",
            native.path, native.cargo_toml_present, native.file_count
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
    output.push_str(&format!("archive: {}\n", publish.archive_hash));
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
    let cargo_toml_present = native_root.join("Cargo.toml").exists();
    let mut files = Vec::new();
    if native_root.exists() {
        collect_regular_files(&native_root, &mut files)?;
    }
    let mut reasons = Vec::new();
    if !native_root.exists() {
        reasons.push("native Rust path missing".to_string());
    }
    if !cargo_toml_present {
        reasons.push("native Rust Cargo.toml missing".to_string());
    }
    if files.is_empty() {
        reasons.push("native Rust source files missing".to_string());
    }
    let ok = reasons.is_empty();
    let risk = if ok {
        PackageRisk::Elevated
    } else {
        PackageRisk::High
    };

    Ok(Some(PackageNativeRustCheck {
        path: native.path.clone(),
        cargo_toml_present,
        file_count: files.len(),
        ok,
        risk,
        reasons,
    }))
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

fn package_archive_hash(package_dir: &Path) -> Result<String, String> {
    let package = load_package(package_dir)?;
    let native = package
        .manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref());
    let native_hash = package_native_hash(package_dir, native)?;
    Ok(package_checksum(&package, native_hash.as_deref()))
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
        "{}:{}:{}:{}:{}:{}:{}\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.diagnostics,
        review.summary.errors,
        review.summary.dependencies,
        review.summary.dev_dependencies,
        review.summary.package_features
    ));
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
