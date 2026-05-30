use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::diagnostic::{Diagnostic, code};

use super::source_set::{
    Manifest, PackageSource, load_package_manifest, load_package_with_features,
    resolve_package_features,
};
use super::{PackageReviewFileKind, canonical_path_label, toml_value_label};

#[derive(Debug, Clone)]
pub(super) struct PackageDependencySpec {
    pub(super) name: String,
    pub(super) requirement: Option<String>,
    pub(super) path: Option<String>,
    pub(super) git: Option<String>,
    pub(super) features: Vec<String>,
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
    collect_dependency_feature_resolution_diagnostics_from_map(
        package_dir,
        &manifest.dependencies,
        &mut BTreeSet::new(),
        &mut diagnostics,
    )?;
    collect_dependency_feature_resolution_diagnostics_from_map(
        package_dir,
        &manifest.dev_dependencies,
        &mut BTreeSet::new(),
        &mut diagnostics,
    )?;
    Ok(diagnostics)
}

fn collect_dependency_feature_resolution_diagnostics_from_map(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    visiting: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
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
        let resolved = resolve_package_features(&dependency_manifest, &spec.features);
        for feature in resolved.unknown {
            diagnostics.push(package_unknown_feature_diagnostic(
                package_dir,
                name,
                &feature,
            ));
        }
        collect_dependency_feature_resolution_diagnostics_from_map(
            &dependency_dir,
            &dependency_manifest.dependencies,
            visiting,
            diagnostics,
        )?;
        collect_dependency_feature_resolution_diagnostics_from_map(
            &dependency_dir,
            &dependency_manifest.dev_dependencies,
            visiting,
            diagnostics,
        )?;
        visiting.remove(&canonical);
    }
    Ok(())
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
