use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::lock::{package_checksum, package_native_hash};
use super::source_set::load_package;
use super::{
    ArtifactStore, DirectoryCommitOutcome, PackageIdentity, PackageRisk, PackageVendorEntry,
    PackageVendorReport, PackageVendorUnresolved, canonical_checked_root, canonical_path_label,
    copy_package_directory, package_dependency_spec, package_identity,
    sanitize_vendor_path_component,
};

pub fn vendor_package_dir(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageVendorReport, String> {
    let store = if dry_run {
        None
    } else {
        Some(ArtifactStore::open(package_dir)?)
    };
    let package = load_package(package_dir)?;
    let package_root = canonical_checked_root(package_dir, "package vendor write")?;
    let vendor_dir = package_root.join("vendor");
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
            .then(left.source_path.cmp(&right.source_path))
            .then(left.checksum.cmp(&right.checksum))
    });
    entries.dedup_by(|left, right| {
        left.name == right.name
            && left.version == right.version
            && left.source_path == right.source_path
            && left.checksum == right.checksum
    });
    unresolved.sort_by(|left, right| left.name.cmp(&right.name));

    let mut commit_warnings = Vec::new();
    if !dry_run {
        let metadata = serde_json::to_string_pretty(&entries)
            .expect("vendor metadata JSON serialization should not fail");
        let outcome = store
            .as_ref()
            .expect("non-dry vendor operation has an artifact store")
            .replace_directory("vendor", "vendor directory", |staging| {
                for entry in &entries {
                    let source_path = Path::new(&entry.source_path);
                    let destination_name =
                        Path::new(&entry.vendor_path).file_name().ok_or_else(|| {
                            format!("vendor destination has no name: {}", entry.vendor_path)
                        })?;
                    copy_package_directory(source_path, &staging.join(destination_name))?;
                }
                fs::write(staging.join("rss-vendor.json"), metadata.as_bytes())
                    .map_err(|error| format!("failed to stage vendor metadata: {error}"))
            })?;
        if let DirectoryCommitOutcome::CommittedWithCleanupWarning { warnings, .. } = outcome {
            commit_warnings = warnings;
        }
    }

    let ok = unresolved.is_empty();
    let risk = if !commit_warnings.is_empty() {
        PackageRisk::Elevated
    } else if ok {
        PackageRisk::Low
    } else {
        PackageRisk::Unknown
    };
    let mut reasons = unresolved
        .iter()
        .map(|dependency| format!("{} unresolved: {}", dependency.name, dependency.reason))
        .collect::<Vec<_>>();
    reasons.extend(commit_warnings);

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
        let vendor_path = vendor_dir.join(vendor_package_dir_name(&identity, &canonical));
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

fn vendor_package_dir_name(identity: &PackageIdentity, canonical_source: &str) -> String {
    let source_digest = hex::encode(Sha256::digest(canonical_source.as_bytes()));
    format!(
        "{}-{}-{}",
        sanitize_vendor_path_component(&identity.name),
        sanitize_vendor_path_component(&identity.version),
        source_digest
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_directory_names_include_source_identity() {
        let identity = PackageIdentity {
            name: "same/name".to_string(),
            version: "1.0.0".to_string(),
            edition: "2026".to_string(),
        };

        let first = vendor_package_dir_name(&identity, "/packages/first");
        let second = vendor_package_dir_name(&identity, "/packages/second");

        assert_ne!(first, second);
        assert!(first.starts_with("same_name-1.0.0-"));
        assert_eq!(
            first.rsplit('-').next().map(str::len),
            Some(64),
            "vendor paths must retain the full source digest"
        );
    }
}
