use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::package::PackageReviewFileKind;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) package: ManifestPackage,
    #[serde(default)]
    pub(super) interfaces: ManifestPathSection,
    #[serde(default)]
    pub(super) sources: ManifestPathSection,
    #[serde(default)]
    pub(super) tests: ManifestPathSection,
    #[serde(default)]
    pub(super) dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    pub(super) dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub(super) dependency: Option<ManifestDependencyPolicy>,
    #[serde(default)]
    pub(super) features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub(super) review: Option<ManifestReview>,
    #[serde(default)]
    pub(super) native: Option<ManifestNative>,
    #[serde(default, rename = "virtual")]
    pub(super) virtual_package: Option<ManifestVirtual>,
    #[serde(default)]
    pub(super) implements: BTreeMap<String, ManifestProviderImplementation>,
    #[serde(default)]
    pub(super) providers: BTreeMap<String, ManifestProviderChoice>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestPackage {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) edition: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestPathSection {
    #[serde(default)]
    pub(super) paths: Vec<String>,
    #[serde(default)]
    pub(super) exports: Vec<String>,
    #[serde(default)]
    pub(super) features: BTreeMap<String, ManifestFeaturePathSection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestFeaturePathSection {
    #[serde(default)]
    pub(super) paths: Vec<String>,
    #[serde(default)]
    pub(super) exports: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestDependencyPolicy {
    #[serde(default)]
    pub(super) budget: ManifestDependencyBudget,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestDependencyBudget {
    pub(super) max_direct_dependencies: Option<usize>,
    pub(super) max_total_packages: Option<usize>,
    pub(super) max_native_packages: Option<usize>,
    pub(super) max_high_risk_packages: Option<usize>,
    pub(super) max_unknown_packages: Option<usize>,
    pub(super) max_build_execution_packages: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestReview {
    #[serde(default)]
    pub(super) policy: ManifestReviewPolicy,
    #[serde(default)]
    pub(super) feature_policy: ManifestReviewFeaturePolicy,
    #[serde(default)]
    pub(super) expect: ManifestReviewExpect,
    #[serde(default)]
    pub(super) capability_bindings: Vec<ManifestCapabilityBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestCapabilityBinding {
    pub(super) symbol: String,
    pub(super) category: String,
    pub(super) provider: Option<String>,
    pub(super) service: Option<String>,
    pub(super) action: Option<String>,
    pub(super) resource: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestReviewPolicy {
    pub(super) deny_unknown: Option<bool>,
    pub(super) deny_native: Option<bool>,
    pub(super) deny_unsafe_apis: Option<bool>,
    pub(super) max_public_params: Option<usize>,
    pub(super) max_nested_type_depth: Option<usize>,
    pub(super) native_api_risk: Option<String>,
    pub(super) build_execution_default: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestReviewFeaturePolicy {
    #[serde(default)]
    pub(super) deny: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestReviewExpect {
    pub(super) risk: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestNative {
    #[serde(default)]
    pub(super) rust: Option<ManifestNativeRust>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestNativeRust {
    #[serde(default)]
    pub(super) enabled: bool,
    pub(super) path: Option<String>,
    #[serde(rename = "crate")]
    pub(super) crate_name: Option<String>,
    #[serde(default)]
    pub(super) cargo_features: Vec<String>,
    #[serde(default)]
    pub(super) feature_map: BTreeMap<String, ManifestNativeRustFeature>,
    #[serde(default)]
    pub(super) policy: ManifestNativeRustPolicy,
    pub(super) build_scripts: Option<String>,
    pub(super) proc_macros: Option<String>,
    #[serde(rename = "unsafe")]
    pub(super) unsafe_policy: Option<String>,
    pub(super) native_links: Option<String>,
    pub(super) ffi: Option<String>,
    #[serde(default)]
    pub(super) links: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestNativeRustFeature {
    #[serde(default)]
    pub(super) cargo_features: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestNativeRustPolicy {
    pub(super) build_scripts: Option<String>,
    pub(super) proc_macros: Option<String>,
    pub(super) native_links: Option<String>,
    pub(super) ffi: Option<String>,
    pub(super) rss_unsafe_apis: Option<String>,
    pub(super) wrapper_unsafe_blocks: Option<String>,
    pub(super) transitive_unsafe_blocks: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestVirtual {
    #[serde(default)]
    pub(super) has_default: bool,
    pub(super) provider: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestProviderImplementation {
    pub(super) version: Option<String>,
    #[serde(default)]
    pub(super) interface_features: Vec<String>,
    pub(super) interface_effective_hash: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct ManifestProviderChoice {
    pub(super) package: Option<String>,
    pub(super) version: Option<String>,
}

impl<'de> Deserialize<'de> for ManifestProviderChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ProviderChoiceTable {
            package: Option<String>,
            version: Option<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ProviderChoice {
            Package(String),
            Table(ProviderChoiceTable),
        }

        match ProviderChoice::deserialize(deserializer)? {
            ProviderChoice::Package(package) => Ok(ManifestProviderChoice {
                package: Some(package),
                version: None,
            }),
            ProviderChoice::Table(table) => Ok(ManifestProviderChoice {
                package: table.package,
                version: table.version,
            }),
        }
    }
}

impl ManifestNativeRust {
    pub(super) fn effective_build_scripts(&self) -> Option<&str> {
        self.build_scripts
            .as_deref()
            .or(self.policy.build_scripts.as_deref())
    }

    pub(super) fn effective_proc_macros(&self) -> Option<&str> {
        self.proc_macros
            .as_deref()
            .or(self.policy.proc_macros.as_deref())
    }

    pub(super) fn effective_unsafe_policy(&self) -> Option<&str> {
        self.unsafe_policy
            .as_deref()
            .or(self.policy.wrapper_unsafe_blocks.as_deref())
            .or(self.policy.rss_unsafe_apis.as_deref())
    }

    pub(super) fn effective_native_links(&self) -> Option<&str> {
        self.native_links
            .as_deref()
            .or(self.policy.native_links.as_deref())
    }

    pub(super) fn effective_ffi(&self) -> Option<&str> {
        self.ffi.as_deref().or(self.policy.ffi.as_deref())
    }
}

#[derive(Debug, Clone)]
pub(super) struct PackageSource {
    pub(super) path: String,
    pub(super) relative_path: String,
    pub(super) contents: String,
    pub(super) kind: PackageReviewFileKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ResolvedPackageFeatures {
    pub(super) selected: Vec<String>,
    pub(super) unknown: Vec<String>,
}

pub(super) struct LoadedPackage {
    pub(super) manifest_path: PathBuf,
    pub(super) manifest_source: String,
    pub(super) manifest: Manifest,
    pub(super) sources: Vec<PackageSource>,
}

pub(super) fn load_package(package_dir: &Path) -> Result<LoadedPackage, String> {
    load_package_with_features(package_dir, None)
}

pub(super) fn load_package_with_features(
    package_dir: &Path,
    selected_features: Option<&[String]>,
) -> Result<LoadedPackage, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    let manifest_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&manifest_source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;

    let selected_features = selected_features
        .map(|features| resolve_package_features(&manifest, features).selected)
        .unwrap_or_else(|| selected_root_package_features(&manifest));
    let base_interface_roots = default_paths(&manifest.interfaces.paths, "interface");
    let selected_feature_interface_roots =
        selected_interface_feature_paths(&manifest, &selected_features);
    let excluded_feature_interface_roots = all_interface_feature_paths(&manifest);
    let source_roots = default_paths(&manifest.sources.paths, "src");
    let test_roots = manifest.tests.paths.clone();
    let mut sources = Vec::new();
    sources.extend(read_package_sources_excluding(
        package_dir,
        &base_interface_roots,
        &excluded_feature_interface_roots,
        PackageReviewFileKind::Interface,
    )?);
    sources.extend(read_package_sources(
        package_dir,
        &selected_feature_interface_roots,
        PackageReviewFileKind::Interface,
    )?);
    sources.extend(read_package_sources(
        package_dir,
        &source_roots,
        PackageReviewFileKind::Source,
    )?);
    sources.extend(read_package_sources(
        package_dir,
        &test_roots,
        PackageReviewFileKind::Test,
    )?);
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(LoadedPackage {
        manifest_path,
        manifest_source,
        manifest,
        sources,
    })
}

pub(super) fn load_package_manifest(package_dir: &Path) -> Result<Manifest, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    let manifest_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    toml::from_str(&manifest_source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))
}

pub(super) fn selected_root_package_features(manifest: &Manifest) -> Vec<String> {
    let requested = manifest.features.keys().cloned().collect::<Vec<_>>();
    resolve_package_features(manifest, &requested).selected
}

pub(super) fn resolve_package_features(
    manifest: &Manifest,
    requested_features: &[String],
) -> ResolvedPackageFeatures {
    let mut selected = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for feature in requested_features {
        if manifest.features.contains_key(feature) {
            resolve_package_feature(manifest, feature, &mut selected);
        } else {
            unknown.insert(feature.clone());
        }
    }
    ResolvedPackageFeatures {
        selected: selected.into_iter().collect(),
        unknown: unknown.into_iter().collect(),
    }
}

fn resolve_package_feature(manifest: &Manifest, feature: &str, selected: &mut BTreeSet<String>) {
    if !selected.insert(feature.to_string()) {
        return;
    }
    let Some(dependencies) = manifest.features.get(feature) else {
        return;
    };
    for dependency in dependencies {
        if manifest.features.contains_key(dependency) {
            resolve_package_feature(manifest, dependency, selected);
        }
    }
}

fn selected_interface_feature_paths(
    manifest: &Manifest,
    selected_features: &[String],
) -> Vec<String> {
    let mut roots = Vec::new();
    let _ = &manifest.interfaces.exports;
    for feature in selected_features {
        let Some(section) = manifest.interfaces.features.get(feature) else {
            continue;
        };
        roots.extend(section.paths.iter().cloned());
        let _ = &section.exports;
    }
    dedup_strings(&mut roots);
    roots
}

fn all_interface_feature_paths(manifest: &Manifest) -> Vec<String> {
    let mut roots = manifest
        .interfaces
        .features
        .values()
        .flat_map(|section| section.paths.iter().cloned())
        .collect::<Vec<_>>();
    dedup_strings(&mut roots);
    roots
}

fn dedup_strings(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
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
    read_package_sources_excluding(package_dir, roots, &[], kind)
}

fn read_package_sources_excluding(
    package_dir: &Path,
    roots: &[String],
    excluded_roots: &[String],
    kind: PackageReviewFileKind,
) -> Result<Vec<PackageSource>, String> {
    let mut sources = Vec::new();
    let package_root = canonical_package_root(package_dir)?;
    let excluded_roots = excluded_roots
        .iter()
        .map(|root| confined_manifest_path(package_dir, &package_root, root, "source root"))
        .collect::<Result<Vec<_>, _>>()?;
    for root in roots {
        let root_path = confined_manifest_path(package_dir, &package_root, root, "source root")?;
        if !root_path.exists() {
            continue;
        }
        let mut files = Vec::new();
        collect_rsscript_files_excluding(&root_path, &excluded_roots, &mut files)?;
        files.sort();
        for file in files {
            let contents = fs::read_to_string(&file)
                .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
            let relative_path = super::relative_path(&package_root, &file);
            let display_path = package_dir.join(&relative_path).display().to_string();
            sources.push(PackageSource {
                path: display_path,
                relative_path,
                contents,
                kind,
            });
        }
    }
    Ok(sources)
}

pub(super) fn canonical_package_root(package_dir: &Path) -> Result<PathBuf, String> {
    package_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize package root {}: {error}",
            package_dir.display()
        )
    })
}

pub(super) fn validate_manifest_relative_path(value: &str, label: &str) -> Result<(), String> {
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(format!(
            "{label} `{value}` must be relative to the package root."
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "{label} `{value}` must not escape the package root with `..`."
        ));
    }
    Ok(())
}

pub(super) fn confined_manifest_path(
    package_dir: &Path,
    package_root: &Path,
    value: &str,
    label: &str,
) -> Result<PathBuf, String> {
    validate_manifest_relative_path(value, label)?;
    let path = package_dir.join(value);
    if !path.exists() {
        return Ok(path);
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize {label} `{value}`: {error}"))?;
    canonical.strip_prefix(package_root).map_err(|_| {
        format!(
            "{label} `{value}` resolves outside package root {}.",
            package_root.display()
        )
    })?;
    Ok(canonical)
}

fn collect_rsscript_files_excluding(
    path: &Path,
    excluded_roots: &[PathBuf],
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if excluded_roots.iter().any(|root| path == root) {
        return Ok(());
    }
    if path.is_file() {
        if super::is_rsscript_source_path(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        // `file_type()` does NOT follow symlinks (unlike `Path::is_dir`/`is_file`).
        // Reject symlinked entries so a package cannot point review/lock/metadata
        // at files outside its own root (symlink escape).
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to stat entry in {}: {error}", path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "refusing to follow symlink in package source tree: {}",
                entry.path().display()
            ));
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_rsscript_files_excluding(&path, excluded_roots, files)?;
        } else if file_type.is_file() && super::is_rsscript_source_path(&path) {
            files.push(path);
        }
    }
    Ok(())
}
