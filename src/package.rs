use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

mod check;
mod contract;
mod dependency;
mod diff;
mod format;
mod graph;
mod lock;
mod metadata;
mod native;
mod policy;
mod publish;
mod review;
mod source_set;
mod types;
mod vendor;

pub use check::check_package_dir;
use dependency::{
    PackageDependencySpec, collect_dependency_interface_sources, package_dependency_spec,
    package_feature_resolution_diagnostics,
};
pub use diff::diff_package_dirs;
pub use format::*;
pub use graph::package_tree;
pub use lock::{diff_package_locks, lock_package_dir};
pub use metadata::{package_lowering_input, package_metadata};
use native::{manifest_native_enabled, manifest_native_unsafe_boundary};
pub use publish::{publish_package_dry_run, publish_package_dry_run_with_registry};
pub use review::review_package_dir;
use source_set::{LoadedPackage, Manifest, ManifestNativeRust, PackageSource};
pub use types::*;
pub use vendor::vendor_package_dir;

fn relative_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
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

pub(super) fn dedup_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
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

fn is_rsscript_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
}

fn package_manifest_key_span(package_dir: &Path, key: &str) -> crate::diagnostic::Span {
    let path = package_dir.join("rsspkg.toml");
    let file = path.display().to_string();
    let source = fs::read_to_string(&path).unwrap_or_default();
    for (index, line) in source.lines().enumerate() {
        if let Some(column) = line.find(key) {
            return crate::diagnostic::Span {
                file,
                line: index + 1,
                column: column + 1,
                length: key.len().max(1),
            };
        }
    }
    crate::diagnostic::Span {
        file,
        line: 1,
        column: 1,
        length: key.len().max(1),
    }
}

fn package_dependency_span(package_dir: &Path, dependency: &str) -> crate::diagnostic::Span {
    package_manifest_key_span(package_dir, dependency)
}

fn collect_package_feature_boundary_reasons(
    features: &BTreeMap<String, Vec<String>>,
    reasons: &mut Vec<String>,
) {
    for (name, values) in features {
        if package_feature_may_change_boundary_risk(name, values) {
            reasons.push(format!(
                "package feature `{name}` may change native/unsafe/build risk"
            ));
        }
    }
}

fn package_feature_may_change_boundary_risk(name: &str, values: &[String]) -> bool {
    package_feature_token_is_boundary_risk(name)
        || values
            .iter()
            .any(|value| package_feature_token_is_boundary_risk(value))
}

fn package_feature_token_is_boundary_risk(token: &str) -> bool {
    let normalized = token.to_ascii_lowercase();
    ["native", "unsafe", "ffi", "build", "proc", "macro", "link"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn package_identity(manifest: &Manifest) -> PackageIdentity {
    PackageIdentity {
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        edition: manifest.package.edition.clone(),
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

fn package_risk_label(risk: PackageRisk) -> &'static str {
    match risk {
        PackageRisk::Low => "low",
        PackageRisk::Elevated => "elevated",
        PackageRisk::High => "high",
        PackageRisk::Unknown => "unknown",
    }
}
