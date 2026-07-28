use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::package::PackageReviewFileKind;

const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;
const SOURCE_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const PACKAGE_SOURCE_MAX_FILES: usize = 20_000;
const PACKAGE_SOURCE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PACKAGE_SOURCE_MAX_DEPTH: usize = 64;

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
    #[serde(
        default = "native_default_features_enabled",
        rename = "default-features",
        alias = "default_features"
    )]
    pub(super) default_features: bool,
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

fn native_default_features_enabled() -> bool {
    true
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

    pub(super) fn effective_unsafe_policies(&self) -> EffectiveNativeUnsafePolicies<'_> {
        let has_granular = self.policy.rss_unsafe_apis.is_some()
            || self.policy.wrapper_unsafe_blocks.is_some()
            || self.policy.transitive_unsafe_blocks.is_some();
        let legacy = (!has_granular)
            .then_some(self.unsafe_policy.as_deref())
            .flatten();
        EffectiveNativeUnsafePolicies {
            rss_unsafe_apis: self.policy.rss_unsafe_apis.as_deref().or(legacy),
            wrapper_unsafe_blocks: self.policy.wrapper_unsafe_blocks.as_deref().or(legacy),
            transitive_unsafe_blocks: self.policy.transitive_unsafe_blocks.as_deref().or(legacy),
        }
    }

    pub(super) fn has_mixed_legacy_and_granular_unsafe_policy(&self) -> bool {
        self.unsafe_policy.is_some()
            && (self.policy.rss_unsafe_apis.is_some()
                || self.policy.wrapper_unsafe_blocks.is_some()
                || self.policy.transitive_unsafe_blocks.is_some())
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

#[derive(Debug, Clone, Copy)]
pub(super) struct EffectiveNativeUnsafePolicies<'a> {
    pub(super) rss_unsafe_apis: Option<&'a str>,
    pub(super) wrapper_unsafe_blocks: Option<&'a str>,
    pub(super) transitive_unsafe_blocks: Option<&'a str>,
}

impl EffectiveNativeUnsafePolicies<'_> {
    pub(super) fn has_non_forbidden_boundary(self) -> bool {
        [
            self.rss_unsafe_apis,
            self.wrapper_unsafe_blocks,
            self.transitive_unsafe_blocks,
        ]
        .into_iter()
        .flatten()
        .any(|policy| policy != "forbid")
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
    let package_root = canonical_package_root(package_dir)?;
    let manifest_path = package_dir.join("rsspkg.toml");
    let (manifest_source, _) = read_bounded_utf8_file(
        &package_root,
        &package_root.join("rsspkg.toml"),
        MANIFEST_MAX_BYTES,
        "package manifest",
    )?;
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
    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    sources.extend(read_package_sources_excluding(
        package_dir,
        &base_interface_roots,
        &excluded_feature_interface_roots,
        PackageReviewFileKind::Interface,
        &mut budget,
    )?);
    sources.extend(read_package_sources_excluding(
        package_dir,
        &selected_feature_interface_roots,
        &[],
        PackageReviewFileKind::Interface,
        &mut budget,
    )?);
    sources.extend(read_package_sources_excluding(
        package_dir,
        &source_roots,
        &[],
        PackageReviewFileKind::Source,
        &mut budget,
    )?);
    sources.extend(read_package_sources_excluding(
        package_dir,
        &test_roots,
        &[],
        PackageReviewFileKind::Test,
        &mut budget,
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
    let package_root = canonical_package_root(package_dir)?;
    let manifest_path = package_dir.join("rsspkg.toml");
    let (manifest_source, _) = read_bounded_utf8_file(
        &package_root,
        &package_root.join("rsspkg.toml"),
        MANIFEST_MAX_BYTES,
        "package manifest",
    )?;
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

fn read_package_sources_excluding(
    package_dir: &Path,
    roots: &[String],
    excluded_roots: &[String],
    kind: PackageReviewFileKind,
    budget: &mut SourceBudget,
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
        collect_rsscript_files_excluding(&root_path, &excluded_roots, &mut files, 0, budget)?;
        files.sort();
        for file in files {
            let (contents, bytes) = read_bounded_utf8_file(
                &package_root,
                &file,
                SOURCE_FILE_MAX_BYTES,
                "package source",
            )?;
            budget.add_file(bytes, &file)?;
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
    _package_dir: &Path,
    package_root: &Path,
    value: &str,
    label: &str,
) -> Result<PathBuf, String> {
    validate_manifest_relative_path(value, label)?;
    let path = package_root.join(value);
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
    let mut current = package_root.to_path_buf();
    for component in Path::new(value).components() {
        let Component::Normal(component) = component else {
            continue;
        };
        current.push(component);
        super::package_path_metadata(&current, label)?;
    }
    Ok(path)
}

fn collect_rsscript_files_excluding(
    path: &Path,
    excluded_roots: &[PathBuf],
    files: &mut Vec<PathBuf>,
    depth: usize,
    budget: &mut SourceBudget,
) -> Result<(), String> {
    budget.check_depth(depth, path)?;
    if excluded_roots.iter().any(|root| path == root) {
        return Ok(());
    }
    let metadata = super::package_path_metadata(path, "package source tree")?;
    if metadata.is_file() {
        if super::is_rsscript_source_path(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "package source tree only accepts regular files or directories: {}",
            path.display()
        ));
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        // `file_type()` does NOT follow symlinks (unlike `Path::is_dir`/`is_file`).
        // Reject symlinked entries so a package cannot point review/lock/metadata
        // at files outside its own root (symlink escape).
        let path = entry.path();
        let metadata = super::package_path_metadata(&path, "package source tree")?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            collect_rsscript_files_excluding(&path, excluded_roots, files, depth + 1, budget)?;
        } else if file_type.is_file() && super::is_rsscript_source_path(&path) {
            files.push(path);
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct SourceBudget {
    files: usize,
    bytes: u64,
}

impl SourceBudget {
    fn check_depth(&self, depth: usize, path: &Path) -> Result<(), String> {
        if depth > PACKAGE_SOURCE_MAX_DEPTH {
            return Err(format!(
                "package source tree exceeded depth limit of {PACKAGE_SOURCE_MAX_DEPTH} at {}",
                path.display()
            ));
        }
        Ok(())
    }

    fn add_file(&mut self, size: u64, path: &Path) -> Result<(), String> {
        if size > SOURCE_FILE_MAX_BYTES {
            return Err(format!(
                "package source {} exceeded per-file byte limit of {SOURCE_FILE_MAX_BYTES}",
                path.display()
            ));
        }
        self.files = self.files.saturating_add(1);
        if self.files > PACKAGE_SOURCE_MAX_FILES {
            return Err(format!(
                "package source tree exceeded file limit of {PACKAGE_SOURCE_MAX_FILES}"
            ));
        }
        self.bytes = self
            .bytes
            .checked_add(size)
            .ok_or_else(|| "package source byte accounting overflowed".to_string())?;
        if self.bytes > PACKAGE_SOURCE_MAX_BYTES {
            return Err(format!(
                "package source tree exceeded total byte limit of {PACKAGE_SOURCE_MAX_BYTES}"
            ));
        }
        Ok(())
    }
}

fn read_bounded_utf8_file(
    package_root: &Path,
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<(String, u64), String> {
    let (file, metadata) = super::open_regular_file_within_root(package_root, path, label)?;
    if metadata.len() > max_bytes {
        if label == "package source" {
            return Err(format!(
                "{label} {} exceeded per-file byte limit of {max_bytes}",
                path.display()
            ));
        }
        return Err(format!(
            "{label} {} exceeded byte limit of {max_bytes}",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{label} {} exceeded byte limit of {max_bytes}",
            path.display()
        ));
    }
    let actual = bytes.len() as u64;
    String::from_utf8(bytes)
        .map(|contents| (contents, actual))
        .map_err(|error| format!("failed to read {} as UTF-8: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsscript-source-budget-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn write_manifest(root: &Path) {
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"budget\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .expect("manifest");
    }

    #[test]
    fn rejects_oversized_manifest_before_parsing() {
        let root = fixture("manifest");
        fs::create_dir_all(&root).expect("root");
        fs::write(
            root.join("rsspkg.toml"),
            vec![b'x'; MANIFEST_MAX_BYTES as usize + 1],
        )
        .expect("oversized manifest");

        let error = load_package_manifest(&root).expect_err("manifest must be bounded");
        assert!(error.contains("exceeded byte limit"), "{error}");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_manifest_without_reading_target() {
        use std::os::unix::fs::symlink;

        let root = fixture("manifest-symlink");
        let outside = fixture("manifest-symlink-outside");
        fs::create_dir_all(&root).expect("root");
        fs::write(
            &outside,
            "[package]\nname = \"outside\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .expect("outside manifest");
        symlink(&outside, root.join("rsspkg.toml")).expect("manifest symlink");

        load_package_manifest(&root).expect_err("manifest symlink must be rejected");
        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_file(outside).expect("outside cleanup");
    }

    #[test]
    fn rejects_oversized_source_before_loading_contents() {
        let root = fixture("source");
        fs::create_dir_all(root.join("src")).expect("source root");
        write_manifest(&root);
        fs::write(
            root.join("src/large.rss"),
            vec![b'x'; SOURCE_FILE_MAX_BYTES as usize + 1],
        )
        .expect("oversized source");

        let error = match load_package(&root) {
            Ok(_) => panic!("source must be bounded"),
            Err(error) => error,
        };
        assert!(error.contains("per-file byte limit"), "{error}");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_source_tree_beyond_depth_limit() {
        let root = fixture("depth");
        let mut directory = root.join("src");
        fs::create_dir_all(&directory).expect("source root");
        write_manifest(&root);
        for _ in 0..=PACKAGE_SOURCE_MAX_DEPTH {
            directory.push("nested");
        }
        fs::create_dir_all(&directory).expect("deep source tree");
        fs::write(
            directory.join("main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("source");

        let error = match load_package(&root) {
            Ok(_) => panic!("depth must be bounded"),
            Err(error) => error,
        };
        assert!(error.contains("exceeded depth limit"), "{error}");
        fs::remove_dir_all(root).expect("cleanup");
    }
}
