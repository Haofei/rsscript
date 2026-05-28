use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::analyzer::{analyze_source_with_core, analyze_source_with_interfaces, core_interfaces};
use crate::diagnostic::Diagnostic;
use crate::review::{ReviewMap, review_map_sources};

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
    contents: String,
    kind: PackageReviewFileKind,
}

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
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
    collect_manifest_review_reasons(&manifest, &mut reasons);
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

    let risk = package_risk(&manifest, native_rust.as_ref(), &review_map, &diagnostics);
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
            name: manifest.package.name,
            version: manifest.package.version,
            edition: manifest.package.edition,
        },
        manifest_path: manifest_path.display().to_string(),
        risk,
        reasons,
        summary,
        files,
        native_rust,
        review_map,
        diagnostics,
    })
}

pub fn format_package_review_json(review: &PackageReview) -> String {
    serde_json::to_string(review).expect("package review JSON serialization should not fail")
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
                contents,
                kind,
            });
        }
    }
    Ok(sources)
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

fn is_rsscript_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
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
