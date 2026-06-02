use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::diagnostic::{Diagnostic, code};

use super::source_set::{
    Manifest, ManifestReviewFeaturePolicy, PackageSource, load_package_manifest,
    load_package_with_features, resolve_package_features,
};
use super::{PackageReviewFileKind, canonical_path_label, toml_value_label};

#[derive(Debug, Clone)]
pub(super) struct PackageDependencySpec {
    pub(super) name: String,
    pub(super) requirement: Option<String>,
    pub(super) path: Option<String>,
    pub(super) git: Option<String>,
    pub(super) features: Vec<String>,
    pub(super) compile_only: bool,
    pub(super) test_only: bool,
    pub(super) platform_provided: bool,
}

pub(super) fn collect_dependency_interface_sources(
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

pub(super) fn package_feature_resolution_diagnostics(
    package_dir: &Path,
    manifest: &Manifest,
) -> Result<Vec<Diagnostic>, String> {
    let mut diagnostics = Vec::new();
    let feature_policy = manifest
        .review
        .as_ref()
        .map(|review| &review.feature_policy);
    collect_dependency_feature_resolution_diagnostics_from_map(
        package_dir,
        &manifest.dependencies,
        feature_policy,
        &mut BTreeSet::new(),
        &mut diagnostics,
    )?;
    collect_dependency_feature_resolution_diagnostics_from_map(
        package_dir,
        &manifest.dev_dependencies,
        feature_policy,
        &mut BTreeSet::new(),
        &mut diagnostics,
    )?;
    Ok(diagnostics)
}

fn collect_dependency_feature_resolution_diagnostics_from_map(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    feature_policy: Option<&ManifestReviewFeaturePolicy>,
    visiting: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), String> {
    for (name, value) in dependencies {
        diagnostics.extend(package_dependency_unknown_key_diagnostics(
            package_dir,
            name,
            value,
        ));
        let spec = package_dependency_spec(name, value);
        if spec.git.is_some() {
            diagnostics.push(package_unsupported_dependency_source_diagnostic(
                package_dir,
                name,
                "git",
            ));
        }
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
        let dependency_manifest = load_package_manifest(&dependency_dir)?;
        let resolved = resolve_package_features(&dependency_manifest, &spec.features);
        for feature in resolved.unknown {
            diagnostics.push(package_unknown_feature_diagnostic(
                package_dir,
                name,
                &feature,
            ));
        }
        if let Some(feature_policy) = feature_policy {
            for feature in &resolved.selected {
                if package_feature_denied(feature_policy, name, feature) {
                    diagnostics.push(package_denied_feature_diagnostic(
                        package_dir,
                        name,
                        feature,
                    ));
                }
            }
        }
        collect_dependency_feature_resolution_diagnostics_from_map(
            &dependency_dir,
            &dependency_manifest.dependencies,
            feature_policy,
            visiting,
            diagnostics,
        )?;
        collect_dependency_feature_resolution_diagnostics_from_map(
            &dependency_dir,
            &dependency_manifest.dev_dependencies,
            feature_policy,
            visiting,
            diagnostics,
        )?;
        visiting.remove(&canonical);
    }
    Ok(())
}

fn package_dependency_unknown_key_diagnostics(
    package_dir: &Path,
    dependency: &str,
    value: &toml::Value,
) -> Vec<Diagnostic> {
    let Some(table) = value.as_table() else {
        return Vec::new();
    };
    const ALLOWED_KEYS: &[&str] = &[
        "version",
        "path",
        "git",
        "features",
        "compile_only",
        "test_only",
        "platform_provided",
    ];
    table
        .keys()
        .filter(|key| !ALLOWED_KEYS.contains(&key.as_str()))
        .map(|key| {
            Diagnostic::error(
                code::PACKAGE_REVIEW_POLICY_VIOLATION,
                format!("dependency `{dependency}` has unknown key `{key}`."),
                super::package_dependency_span(package_dir, dependency),
                "unknown dependency key",
            )
            .with_cause("Package dependency metadata is review-critical and unknown keys cannot be ignored.")
            .with_fix(
                "remove_unknown_dependency_key",
                format!("Remove `{key}` or replace it with a supported dependency key."),
                "manual",
            )
        })
        .collect()
}

fn package_unsupported_dependency_source_diagnostic(
    package_dir: &Path,
    dependency: &str,
    source: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_UNSUPPORTED_DEPENDENCY_SOURCE,
        format!("dependency `{dependency}` uses unsupported package source `{source}`."),
        super::package_dependency_span(package_dir, dependency),
        "unsupported dependency source",
    )
    .with_cause("Git dependencies are not part of the v0.6 accepted dependency-source grammar.")
    .with_fix(
        "use_supported_dependency_source",
        "Use a registry version requirement or a local path dependency.",
        "manual",
    )
}

fn package_feature_denied(
    feature_policy: &ManifestReviewFeaturePolicy,
    package: &str,
    feature: &str,
) -> bool {
    feature_policy
        .deny
        .iter()
        .any(|pattern| package_feature_deny_pattern_matches(pattern, package, feature))
}

fn package_feature_deny_pattern_matches(pattern: &str, package: &str, feature: &str) -> bool {
    let Some((package_pattern, feature_pattern)) = pattern.split_once('/') else {
        return pattern == feature;
    };
    (package_pattern == "*" || package_pattern == package) && feature_pattern == feature
}

fn package_unknown_feature_diagnostic(
    package_dir: &Path,
    dependency: &str,
    feature: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_FEATURE_RESOLUTION,
        format!("dependency `{dependency}` selects unknown package feature `{feature}`."),
        super::package_dependency_span(package_dir, dependency),
        "unknown package feature",
    )
    .with_cause("Selected dependency features must be declared by the dependency package.")
    .with_fix(
        "fix_dependency_features",
        format!("Remove `{feature}` from the dependency feature list, or declare it in the dependency package."),
        "manual",
    )
}

fn package_denied_feature_diagnostic(
    package_dir: &Path,
    dependency: &str,
    feature: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_REVIEW_POLICY_VIOLATION,
        format!("dependency `{dependency}` selects denied package feature `{feature}`."),
        super::package_dependency_span(package_dir, dependency),
        "denied package feature",
    )
    .with_cause("Selected dependency features must satisfy `[review.feature_policy]`.")
    .with_fix(
        "fix_dependency_features",
        format!(
            "Remove `{feature}` from the dependency feature list, or choose another dependency."
        ),
        "manual",
    )
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
        let dependency_manifest = load_package_manifest(&dependency_dir)?;
        let selected_features = resolve_package_features(&dependency_manifest, &spec.features);
        let dependency_package =
            load_package_with_features(&dependency_dir, Some(&selected_features.selected))?;
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

pub(super) fn package_dependency_spec(name: &str, value: &toml::Value) -> PackageDependencySpec {
    if let Some(requirement) = value.as_str() {
        return PackageDependencySpec {
            name: name.to_string(),
            requirement: Some(requirement.to_string()),
            path: None,
            git: None,
            features: Vec::new(),
            compile_only: false,
            test_only: false,
            platform_provided: false,
        };
    }
    let Some(table) = value.as_table() else {
        return PackageDependencySpec {
            name: name.to_string(),
            requirement: Some(toml_value_label(value)),
            path: None,
            git: None,
            features: Vec::new(),
            compile_only: false,
            test_only: false,
            platform_provided: false,
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
        compile_only: table
            .get("compile_only")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        test_only: table
            .get("test_only")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        platform_provided: table
            .get("platform_provided")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
    }
}
