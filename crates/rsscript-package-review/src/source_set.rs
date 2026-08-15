use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rsscript_project::{
    ProjectSourceCapture, ProjectSourceCaptureLimits, capture_optional_project_utf8,
    capture_project_manifest,
};
use serde::Deserialize;

use rsscript_package_model::PackageReviewFileKind;

const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;
pub const SNAPSHOT_MANIFEST_SOURCE_FILE: &str = ".rsscript-snapshot-manifest";
const SOURCE_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const PACKAGE_SOURCE_MAX_FILES: usize = 20_000;
const PACKAGE_SOURCE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PACKAGE_SOURCE_MAX_DEPTH: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: ManifestPackage,
    #[serde(default)]
    pub interfaces: ManifestPathSection,
    #[serde(default)]
    pub sources: ManifestPathSection,
    #[serde(default)]
    pub tests: ManifestPathSection,
    #[serde(default)]
    pub dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    pub dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub dependency: Option<ManifestDependencyPolicy>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub review: Option<ManifestReview>,
    #[serde(default)]
    pub native: Option<ManifestNative>,
    #[serde(default, rename = "virtual")]
    pub virtual_package: Option<ManifestVirtual>,
    #[serde(default)]
    pub implements: BTreeMap<String, ManifestProviderImplementation>,
    #[serde(default)]
    pub providers: BTreeMap<String, ManifestProviderChoice>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPackage {
    pub name: String,
    pub version: String,
    pub edition: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPathSection {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exports: Vec<String>,
    #[serde(default)]
    pub features: BTreeMap<String, ManifestFeaturePathSection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFeaturePathSection {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exports: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDependencyPolicy {
    #[serde(default)]
    pub budget: ManifestDependencyBudget,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDependencyBudget {
    pub max_direct_dependencies: Option<usize>,
    pub max_total_packages: Option<usize>,
    pub max_native_packages: Option<usize>,
    pub max_high_risk_packages: Option<usize>,
    pub max_unknown_packages: Option<usize>,
    pub max_build_execution_packages: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestReview {
    #[serde(default)]
    pub policy: ManifestReviewPolicy,
    #[serde(default)]
    pub feature_policy: ManifestReviewFeaturePolicy,
    #[serde(default)]
    pub expect: ManifestReviewExpect,
    #[serde(default)]
    pub external_binding_bindings: Vec<ManifestExternalBindingBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExternalBindingBinding {
    pub symbol: String,
    pub category: String,
    pub provider: Option<String>,
    pub service: Option<String>,
    pub action: Option<String>,
    pub resource: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestReviewPolicy {
    pub deny_unknown: Option<bool>,
    pub deny_native: Option<bool>,
    pub deny_unsafe_apis: Option<bool>,
    pub max_public_params: Option<usize>,
    pub max_nested_type_depth: Option<usize>,
    pub native_api_risk: Option<String>,
    pub build_execution_default: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestReviewFeaturePolicy {
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestReviewExpect {
    pub risk: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestNative {
    #[serde(default)]
    pub rust: Option<ManifestNativeRust>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestNativeRust {
    #[serde(default)]
    pub enabled: bool,
    pub path: Option<String>,
    #[serde(rename = "crate")]
    pub crate_name: Option<String>,
    #[serde(default)]
    pub cargo_features: Vec<String>,
    #[serde(
        default = "native_default_features_enabled",
        rename = "default-features",
        alias = "default_features"
    )]
    pub default_features: bool,
    #[serde(default)]
    pub feature_map: BTreeMap<String, ManifestNativeRustFeature>,
    #[serde(default)]
    pub policy: ManifestNativeRustPolicy,
    #[serde(default)]
    pub links: Vec<String>,
}

fn native_default_features_enabled() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestNativeRustFeature {
    #[serde(default)]
    pub cargo_features: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestNativeRustPolicy {
    pub build_scripts: Option<String>,
    pub proc_macros: Option<String>,
    pub native_links: Option<String>,
    pub ffi: Option<String>,
    pub rss_unsafe_apis: Option<String>,
    pub wrapper_unsafe_blocks: Option<String>,
    pub transitive_unsafe_blocks: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestVirtual {
    #[serde(default)]
    pub has_default: bool,
    pub provider: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestProviderImplementation {
    pub version: Option<String>,
    #[serde(default)]
    pub interface_features: Vec<String>,
    pub interface_effective_hash: Option<String>,
}

#[derive(Debug, Default)]
pub struct ManifestProviderChoice {
    pub package: Option<String>,
    pub version: Option<String>,
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
    pub fn effective_build_scripts(&self) -> Option<&str> {
        self.policy.build_scripts.as_deref()
    }

    pub fn effective_proc_macros(&self) -> Option<&str> {
        self.policy.proc_macros.as_deref()
    }

    pub fn effective_unsafe_policies(&self) -> EffectiveNativeUnsafePolicies<'_> {
        EffectiveNativeUnsafePolicies {
            rss_unsafe_apis: self.policy.rss_unsafe_apis.as_deref(),
            wrapper_unsafe_blocks: self.policy.wrapper_unsafe_blocks.as_deref(),
            transitive_unsafe_blocks: self.policy.transitive_unsafe_blocks.as_deref(),
        }
    }

    pub fn effective_native_links(&self) -> Option<&str> {
        self.policy.native_links.as_deref()
    }

    pub fn effective_ffi(&self) -> Option<&str> {
        self.policy.ffi.as_deref()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EffectiveNativeUnsafePolicies<'a> {
    pub rss_unsafe_apis: Option<&'a str>,
    pub wrapper_unsafe_blocks: Option<&'a str>,
    pub transitive_unsafe_blocks: Option<&'a str>,
}

impl EffectiveNativeUnsafePolicies<'_> {
    pub fn has_non_forbidden_boundary(self) -> bool {
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
pub struct PackageSource {
    pub path: String,
    pub relative_path: String,
    pub contents: String,
    pub kind: PackageReviewFileKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPackageFeatures {
    pub selected: Vec<String>,
    pub unknown: Vec<String>,
}

pub struct LoadedPackage {
    pub manifest_path: PathBuf,
    pub manifest_source: String,
    pub manifest: Manifest,
    pub sources: Vec<PackageSource>,
}

pub fn load_package(package_dir: &Path) -> Result<LoadedPackage, String> {
    load_package_with_features(package_dir, None)
}

pub fn load_package_with_features(
    package_dir: &Path,
    selected_features: Option<&[String]>,
) -> Result<LoadedPackage, String> {
    let captured_manifest = capture_project_manifest(package_dir, MANIFEST_MAX_BYTES)?;
    load_package_from_manifest_source(package_dir, captured_manifest.source(), selected_features)
}

/// Assemble package sources from manifest bytes already admitted by the
/// project boundary. This performs semantic manifest parsing and constrained
/// source capture, but it deliberately never reopens `rsspkg.toml`.
pub fn load_package_from_manifest_source(
    package_dir: &Path,
    physical_manifest_source: &str,
    selected_features: Option<&[String]>,
) -> Result<LoadedPackage, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    let manifest = parse_package_manifest_source(package_dir, physical_manifest_source)?;
    let manifest_source = capture_optional_project_utf8(
        package_dir,
        SNAPSHOT_MANIFEST_SOURCE_FILE,
        MANIFEST_MAX_BYTES,
        "package snapshot manifest identity",
    )?
    .unwrap_or_else(|| physical_manifest_source.to_string());

    let selected_features = selected_features
        .map(|features| resolve_package_features(&manifest, features).selected)
        .unwrap_or_else(|| selected_root_package_features(&manifest));
    let base_interface_roots = default_paths(&manifest.interfaces.paths, "interface");
    let selected_feature_interface_roots =
        selected_interface_feature_paths(&manifest, &selected_features);
    let excluded_feature_interface_roots = all_interface_feature_paths(&manifest);
    let source_roots = default_paths(&manifest.sources.paths, "src");
    let test_roots = manifest.tests.paths.clone();
    let mut capture = ProjectSourceCapture::new(
        package_dir,
        ProjectSourceCaptureLimits {
            max_files: PACKAGE_SOURCE_MAX_FILES,
            max_total_bytes: PACKAGE_SOURCE_MAX_BYTES,
            max_file_bytes: SOURCE_FILE_MAX_BYTES,
            max_depth: PACKAGE_SOURCE_MAX_DEPTH,
        },
    )?;
    let mut sources = Vec::new();
    sources.extend(read_package_sources_excluding(
        package_dir,
        &base_interface_roots,
        &excluded_feature_interface_roots,
        PackageReviewFileKind::Interface,
        &mut capture,
    )?);
    sources.extend(read_package_sources_excluding(
        package_dir,
        &selected_feature_interface_roots,
        &[],
        PackageReviewFileKind::Interface,
        &mut capture,
    )?);
    sources.extend(read_package_sources_excluding(
        package_dir,
        &source_roots,
        &[],
        PackageReviewFileKind::Source,
        &mut capture,
    )?);
    sources.extend(read_package_sources_excluding(
        package_dir,
        &test_roots,
        &[],
        PackageReviewFileKind::Test,
        &mut capture,
    )?);
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(LoadedPackage {
        manifest_path,
        manifest_source,
        manifest,
        sources,
    })
}

pub fn load_package_manifest(package_dir: &Path) -> Result<Manifest, String> {
    load_package_manifest_with_source(package_dir).map(|(_, manifest)| manifest)
}

pub fn load_package_manifest_with_source(package_dir: &Path) -> Result<(String, Manifest), String> {
    let captured_manifest = capture_project_manifest(package_dir, MANIFEST_MAX_BYTES)?;
    let manifest_source = captured_manifest.source().to_string();
    let manifest = parse_package_manifest_source(package_dir, &manifest_source)?;
    Ok((manifest_source, manifest))
}

/// Interpret bytes already captured by the project boundary as package
/// semantics. This deliberately performs no filesystem access; dependency
/// graph capture supplies the exact manifest source it read.
pub fn parse_package_manifest_source(
    package_dir: &Path,
    manifest_source: &str,
) -> Result<Manifest, String> {
    let manifest_path = package_dir.join("rsspkg.toml");
    toml::from_str(manifest_source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))
}

pub fn selected_root_package_features(manifest: &Manifest) -> Vec<String> {
    let requested = manifest.features.keys().cloned().collect::<Vec<_>>();
    resolve_package_features(manifest, &requested).selected
}

pub fn resolve_package_features(
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
    capture: &mut ProjectSourceCapture,
) -> Result<Vec<PackageSource>, String> {
    capture
        .capture(roots, excluded_roots)?
        .into_iter()
        .map(|source| {
            let relative_path = source.relative_path().to_string();
            Ok(PackageSource {
                path: package_dir.join(&relative_path).display().to_string(),
                relative_path,
                contents: source.contents().to_string(),
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
