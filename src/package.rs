use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analyzer::{analyze_source_with_core, analyze_source_with_interfaces, core_interfaces};
use crate::diagnostic::Diagnostic;
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

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    let package = load_package(package_dir)?;
    let manifest = &package.manifest;
    let sources = &package.sources;

    let interface_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let mut combined_interfaces = core_interfaces().to_vec();
    combined_interfaces.extend(interface_refs);
    let diagnostics = sources
        .iter()
        .flat_map(|source| {
            if source.kind == PackageReviewFileKind::Source {
                analyze_source_with_interfaces(&source.path, &source.contents, &combined_interfaces)
            } else {
                analyze_source_with_core(&source.path, &source.contents)
            }
        })
        .collect::<Vec<_>>();
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

pub fn lock_package_dir(package_dir: &Path) -> Result<PackageLock, String> {
    let package = load_package(package_dir)?;
    let review = review_package_dir(package_dir)?;
    let native = package
        .manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref());
    let native_hash = package_native_hash(package_dir, native)?;
    let features = package
        .manifest
        .features
        .keys()
        .cloned()
        .collect::<Vec<_>>();

    Ok(PackageLock {
        version: 1,
        packages: vec![PackageLockPackage {
            name: package.manifest.package.name.clone(),
            version: package.manifest.package.version.clone(),
            source: format!("path+{}", package_dir.display()),
            checksum: package_checksum(&package, native_hash.as_deref()),
            interface_hash: hash_sources(&package.sources, PackageReviewFileKind::Interface),
            review_hash: package_review_hash(&review),
            native_hash,
            features,
        }],
        metadata: PackageLockMetadata {
            rsscript_version: env!("CARGO_PKG_VERSION").to_string(),
            created_by: "rsscript package lock".to_string(),
        },
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

pub fn format_package_diff_json(diff: &PackageDiff) -> String {
    serde_json::to_string(diff).expect("package diff JSON serialization should not fail")
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

fn read_package_lock(path: &Path) -> Result<PackageLock, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&source).map_err(|error| format!("failed to parse {}: {error}", path.display()))
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
