use std::collections::BTreeMap;
use std::path::Path;

use super::graph::package_tree;
use super::lock::{lock_package_entry, package_archive_files, package_archive_hash};
use super::source_set::{LoadedPackage, Manifest, load_package, selected_root_package_features};
use super::{
    PACKAGE_REVIEW_METADATA_SCHEMA, PackageLockPackage, PackageNativeRustCheck,
    PackagePublishCheck, PackagePublishDryRun, PackageRegistryFootprint, PackageRegistryIndexEntry,
    PackageRegistryPublishTarget, PackageRisk, PackageTreeSummary, check_package_dir,
    package_dependency_spec, package_identity, package_risk_label, review_package_dir,
    sanitize_vendor_path_component,
};

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
    let root_features = selected_root_package_features(&package.manifest);
    let root_lock_entry = lock_package_entry(package_dir, &package, root_features.clone())?;

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
            "package review risk classified",
            review.risk != PackageRisk::Unknown,
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
        &root_features,
        &tree.summary,
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

/// Registry-index review-risk badges, derived from the entry's own authoritative
/// fields (its aggregate publish `risk` plus the native/unsafe boundary signals),
/// so the badges never disagree with the rest of the index entry. The richer
/// capability badge set lives on `PackageReview::badges` (which has the full
/// review summary).
fn registry_index_badges(risk: PackageRisk, native: bool, unsafe_boundary: bool) -> Vec<String> {
    let mut badges = vec![format!("risk:{}", super::package_risk_label(risk))];
    if native {
        badges.push("native".to_string());
    }
    if unsafe_boundary {
        badges.push("unsafe".to_string());
    }
    badges
}

fn package_registry_index_entry(
    package: &LoadedPackage,
    lock_entry: &PackageLockPackage,
    native_check: Option<&PackageNativeRustCheck>,
    risk: PackageRisk,
    archive_hash: &str,
    root_features: &[String],
    tree_summary: &PackageTreeSummary,
) -> PackageRegistryIndexEntry {
    let native = package
        .manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .is_some_and(|native| native.enabled);
    let unsafe_boundary = package_index_unsafe_boundary(&package.manifest, native_check);
    let badges = registry_index_badges(risk, native, unsafe_boundary);
    PackageRegistryIndexEntry {
        schema: "rss.registry.index.v1".to_string(),
        name: package.manifest.package.name.clone(),
        version: package.manifest.package.version.clone(),
        checksum: archive_hash.to_string(),
        interface_hash: lock_entry.interface_hash.clone(),
        effective_interface_hash_default: lock_entry.interface_hash.clone(),
        review_hash: lock_entry.review_hash.clone(),
        review_schema: PACKAGE_REVIEW_METADATA_SCHEMA.to_string(),
        native_hash: lock_entry.native_hash.clone(),
        risk,
        native,
        virtual_package: package
            .manifest
            .virtual_package
            .as_ref()
            .map(|virtual_package| super::PackageVirtual {
                has_default: virtual_package.has_default,
                provider: virtual_package.provider.clone(),
            }),
        unsafe_boundary,
        badges,
        dependencies: package_index_dependencies(&package.manifest.dependencies),
        features: package_index_features(&package.manifest, root_features),
        footprint_default: package_index_footprint(native, tree_summary),
    }
}

fn package_index_features(
    manifest: &Manifest,
    root_features: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut features = BTreeMap::new();
    features.insert("default".to_string(), root_features.to_vec());
    for (name, dependencies) in &manifest.features {
        features.insert(name.clone(), dependencies.clone());
    }
    features
}

fn package_index_footprint(native: bool, summary: &PackageTreeSummary) -> PackageRegistryFootprint {
    PackageRegistryFootprint {
        direct_dependencies: summary.packages.saturating_sub(1),
        total_packages: summary.packages,
        path_dependencies: summary.path_dependencies,
        unresolved_dependencies: summary.unresolved_dependencies,
        native,
        native_packages: summary.native_packages,
        build_time_execution: summary.build_execution_packages > 0,
        build_execution_packages: summary.build_execution_packages,
        high_risk_packages: summary.high_risk_packages,
        unknown_facts: summary.unknown_risk_packages,
    }
}

fn package_registry_publish_target(
    registry_dir: &Path,
    package_name: &str,
    package_version: &str,
) -> PackageRegistryPublishTarget {
    let package_component = sanitize_vendor_path_component(package_name);
    let version_component = sanitize_vendor_path_component(package_version);
    let index_path = registry_dir
        .join("index")
        .join(&package_component)
        .join(format!("{version_component}.json"));
    let archive_manifest_path = registry_dir
        .join("archives")
        .join(&package_component)
        .join(&version_component)
        .join("archive-manifest.json");
    PackageRegistryPublishTarget {
        registry_dir: registry_dir.display().to_string().replace('\\', "/"),
        index_path: index_path.display().to_string().replace('\\', "/"),
        archive_manifest_path: archive_manifest_path
            .display()
            .to_string()
            .replace('\\', "/"),
    }
}

fn package_index_unsafe_boundary(
    manifest: &Manifest,
    native_check: Option<&PackageNativeRustCheck>,
) -> bool {
    manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .and_then(|native| native.effective_unsafe_policy())
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
    semver::Version::parse(version).is_ok()
}

#[cfg(test)]
mod version_tests {
    use super::is_semver_like;

    #[test]
    fn publish_versions_follow_semver() {
        for valid in [
            "1.2.3",
            "1.2.3-alpha.1",
            "1.2.3+build.7",
            "1.2.3-alpha+build.7",
        ] {
            assert!(is_semver_like(valid), "expected valid SemVer: {valid}");
        }
        for invalid in ["1.2", "01.2.3", "1.02.3", "1.2.03", "1.2.3-"] {
            assert!(
                !is_semver_like(invalid),
                "expected invalid SemVer: {invalid}"
            );
        }
    }
}
