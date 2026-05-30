use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::source_set::load_package;
use super::{
    PackageIdentity, PackageRisk, PackageVendorEntry, PackageVendorReport, PackageVendorUnresolved,
    canonical_path_label, copy_package_directory, package_checksum, package_dependency_spec,
    package_identity, package_native_hash, sanitize_vendor_path_component,
};

pub fn vendor_package_dir(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageVendorReport, String> {
    let package = load_package(package_dir)?;
    let vendor_dir = package_dir.join("vendor");
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
    });
    entries.dedup_by(|left, right| left.name == right.name && left.version == right.version);
    unresolved.sort_by(|left, right| left.name.cmp(&right.name));

    if !dry_run {
        fs::create_dir_all(&vendor_dir)
            .map_err(|error| format!("failed to create {}: {error}", vendor_dir.display()))?;
        for entry in &entries {
            let source_path = Path::new(&entry.source_path);
            let vendor_path = Path::new(&entry.vendor_path);
            if vendor_path.exists() {
                fs::remove_dir_all(vendor_path).map_err(|error| {
                    format!("failed to remove {}: {error}", vendor_path.display())
                })?;
            }
            copy_package_directory(source_path, vendor_path)?;
        }
        let metadata_path = vendor_dir.join("rss-vendor.json");
        let metadata = serde_json::to_string_pretty(&entries)
            .expect("vendor metadata JSON serialization should not fail");
        fs::write(&metadata_path, metadata)
            .map_err(|error| format!("failed to write {}: {error}", metadata_path.display()))?;
    }

    let ok = unresolved.is_empty();
    let risk = if ok {
        PackageRisk::Low
    } else {
        PackageRisk::Unknown
    };
    let reasons = unresolved
        .iter()
        .map(|dependency| format!("{} unresolved: {}", dependency.name, dependency.reason))
        .collect::<Vec<_>>();

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
        let vendor_path = vendor_dir.join(vendor_package_dir_name(&identity));
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

fn vendor_package_dir_name(identity: &PackageIdentity) -> String {
    format!(
        "{}-{}",
        sanitize_vendor_path_component(&identity.name),
        sanitize_vendor_path_component(&identity.version)
    )
}
