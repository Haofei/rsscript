use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::formatter::format_source;

use super::source_set::{
    ManifestNativeRust, load_package, load_package_manifest, load_package_with_features,
    resolve_package_features, selected_root_package_features,
};
use super::{
    LoadedPackage, PackageArchiveFile, PackageLock, PackageLockDiff, PackageLockFieldChange,
    PackageLockMetadata, PackageLockPackage, PackageLockPackageChange, PackageReview,
    PackageReviewFileKind, PackageRisk, PackageSource, collect_regular_files,
    package_dependency_spec, package_feature_may_change_boundary_risk, package_risk_label,
    relative_path, review_package_dir,
};

pub fn lock_package_dir(package_dir: &Path) -> Result<PackageLock, String> {
    let package = load_package(package_dir)?;
    let root_features = selected_root_package_features(&package.manifest);
    let mut packages = vec![lock_package_entry(package_dir, &package, root_features)?];
    let mut visiting = BTreeSet::new();
    let root_key = super::canonical_path_label(package_dir);
    visiting.insert(root_key.clone());
    collect_lock_dependency_packages(
        package_dir,
        &package.manifest.dependencies,
        &mut visiting,
        &mut packages,
    )?;
    collect_lock_dependency_packages(
        package_dir,
        &package.manifest.dev_dependencies,
        &mut visiting,
        &mut packages,
    )?;
    visiting.remove(&root_key);

    Ok(PackageLock {
        version: 1,
        packages,
        metadata: PackageLockMetadata {
            rsscript_version: env!("CARGO_PKG_VERSION").to_string(),
            created_by: "rsscript pkg lock".to_string(),
        },
    })
}

pub(super) fn lock_package_entry(
    package_dir: &Path,
    package: &LoadedPackage,
    features: Vec<String>,
) -> Result<PackageLockPackage, String> {
    let review = review_package_dir(package_dir)?;
    let native = package
        .manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref());
    let native_hash = package_native_hash(package_dir, native)?;

    Ok(PackageLockPackage {
        name: package.manifest.package.name.clone(),
        version: package.manifest.package.version.clone(),
        source: format!("path+{}", package_dir.display()),
        checksum: package_checksum(package, native_hash.as_deref()),
        interface_hash: effective_interface_hash(&package.sources, &features),
        review_hash: package_review_hash(&review),
        native_hash,
        features,
    })
}

fn collect_lock_dependency_packages(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    visiting: &mut BTreeSet<String>,
    packages: &mut Vec<PackageLockPackage>,
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
        let canonical = super::canonical_path_label(&dependency_dir);
        if !visiting.insert(canonical.clone()) {
            continue;
        }
        let dependency_manifest = load_package_manifest(&dependency_dir)?;
        let selected_features = resolve_package_features(&dependency_manifest, &spec.features);
        let dependency_package =
            load_package_with_features(&dependency_dir, Some(&selected_features.selected))?;
        packages.push(lock_package_entry(
            &dependency_dir,
            &dependency_package,
            selected_features.selected,
        )?);
        collect_lock_dependency_packages(
            &dependency_dir,
            &dependency_package.manifest.dependencies,
            visiting,
            packages,
        )?;
        collect_lock_dependency_packages(
            &dependency_dir,
            &dependency_package.manifest.dev_dependencies,
            visiting,
            packages,
        )?;
        visiting.remove(&canonical);
    }
    Ok(())
}

pub fn diff_package_locks(old_path: &Path, new_path: &Path) -> Result<PackageLockDiff, String> {
    let old_lock = read_package_lock(old_path)?;
    let new_lock = read_package_lock(new_path)?;
    let package_changes = compare_locked_packages(&old_lock.packages, &new_lock.packages);
    let risk = package_changes
        .iter()
        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
    let mut reasons = package_lock_diff_reasons(&package_changes);
    if old_lock.version != new_lock.version {
        reasons.push("lockfile format version changed".to_string());
    }
    reasons.sort();
    reasons.dedup();

    Ok(PackageLockDiff {
        old_lock_path: old_path.display().to_string(),
        new_lock_path: new_path.display().to_string(),
        risk,
        reasons,
        old_packages: old_lock.packages.len(),
        new_packages: new_lock.packages.len(),
        package_changes,
    })
}

pub(super) fn read_package_lock(path: &Path) -> Result<PackageLock, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&source).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

pub(super) fn compare_locked_packages(
    old_packages: &[PackageLockPackage],
    new_packages: &[PackageLockPackage],
) -> Vec<PackageLockPackageChange> {
    let old_packages = locked_packages_by_name(old_packages);
    let new_packages = locked_packages_by_name(new_packages);
    let names = old_packages
        .keys()
        .chain(new_packages.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for name in names {
        match (old_packages.get(&name), new_packages.get(&name)) {
            (None, Some(new)) => changes.push(PackageLockPackageChange {
                name,
                before_version: None,
                after_version: Some(new.version.clone()),
                risk: PackageRisk::Elevated,
                changes: vec![PackageLockFieldChange {
                    field: "package".to_string(),
                    before: None,
                    after: Some("added".to_string()),
                    risk: PackageRisk::Elevated,
                }],
            }),
            (Some(old), None) => changes.push(PackageLockPackageChange {
                name,
                before_version: Some(old.version.clone()),
                after_version: None,
                risk: PackageRisk::High,
                changes: vec![PackageLockFieldChange {
                    field: "package".to_string(),
                    before: Some("present".to_string()),
                    after: None,
                    risk: PackageRisk::High,
                }],
            }),
            (Some(old), Some(new)) => {
                let field_changes = compare_locked_package_fields(old, new);
                if !field_changes.is_empty() {
                    let risk = field_changes
                        .iter()
                        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
                    changes.push(PackageLockPackageChange {
                        name,
                        before_version: Some(old.version.clone()),
                        after_version: Some(new.version.clone()),
                        risk,
                        changes: field_changes,
                    });
                }
            }
            (None, None) => {}
        }
    }
    changes
}

fn locked_packages_by_name(
    packages: &[PackageLockPackage],
) -> BTreeMap<String, &PackageLockPackage> {
    packages
        .iter()
        .map(|package| (package.name.clone(), package))
        .collect()
}

fn compare_locked_package_fields(
    old: &PackageLockPackage,
    new: &PackageLockPackage,
) -> Vec<PackageLockFieldChange> {
    let mut changes = Vec::new();
    push_lock_field_change(
        &mut changes,
        "version",
        Some(old.version.as_str()),
        Some(new.version.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "source",
        Some(old.source.as_str()),
        Some(new.source.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "checksum",
        Some(old.checksum.as_str()),
        Some(new.checksum.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "interface_hash",
        Some(old.interface_hash.as_str()),
        Some(new.interface_hash.as_str()),
        PackageRisk::High,
    );
    push_lock_field_change(
        &mut changes,
        "review_hash",
        Some(old.review_hash.as_str()),
        Some(new.review_hash.as_str()),
        PackageRisk::Elevated,
    );
    push_lock_field_change(
        &mut changes,
        "native_hash",
        old.native_hash.as_deref(),
        new.native_hash.as_deref(),
        PackageRisk::High,
    );
    let old_features = super::feature_values_label(&old.features);
    let new_features = super::feature_values_label(&new.features);
    push_lock_field_change(
        &mut changes,
        "features",
        Some(old_features.as_str()),
        Some(new_features.as_str()),
        package_lock_feature_selection_risk(&old.features, &new.features),
    );
    changes
}

fn package_lock_feature_selection_risk(old: &[String], new: &[String]) -> PackageRisk {
    if old == new {
        return PackageRisk::Low;
    }

    if old
        .iter()
        .chain(new.iter())
        .any(|feature| package_feature_may_change_boundary_risk(feature, &[]))
    {
        PackageRisk::High
    } else {
        PackageRisk::Elevated
    }
}

fn push_lock_field_change(
    changes: &mut Vec<PackageLockFieldChange>,
    field: &str,
    before: Option<&str>,
    after: Option<&str>,
    risk: PackageRisk,
) {
    if before != after {
        changes.push(PackageLockFieldChange {
            field: field.to_string(),
            before: before.map(str::to_string),
            after: after.map(str::to_string),
            risk,
        });
    }
}

pub(super) fn package_lock_diff_reasons(changes: &[PackageLockPackageChange]) -> Vec<String> {
    let mut reasons = Vec::new();
    if changes
        .iter()
        .flat_map(|change| &change.changes)
        .any(|change| change.field == "package" && change.after.as_deref() == Some("added"))
    {
        reasons.push("RSScript package added to lockfile".to_string());
    }
    if changes
        .iter()
        .flat_map(|change| &change.changes)
        .any(|change| change.field == "package" && change.after.is_none())
    {
        reasons.push("RSScript package removed from lockfile".to_string());
    }
    if lock_field_changed(changes, "version") {
        reasons.push("RSScript package version changed".to_string());
    }
    if lock_field_changed(changes, "interface_hash") {
        reasons.push(".rssi interface hash changed".to_string());
    }
    if lock_field_changed(changes, "review_hash") {
        reasons.push("review metadata hash changed".to_string());
    }
    if lock_field_changed(changes, "native_hash") {
        reasons.push("native wrapper source hash changed".to_string());
    }
    if lock_field_changed(changes, "checksum") {
        reasons.push("package checksum changed".to_string());
    }
    if lock_field_changed(changes, "features") {
        reasons.push("package feature selection changed".to_string());
    }
    if lock_field_changed(changes, "source") {
        reasons.push("package source changed".to_string());
    }
    reasons
}

fn lock_field_changed(changes: &[PackageLockPackageChange], field: &str) -> bool {
    changes
        .iter()
        .flat_map(|change| &change.changes)
        .any(|change| change.field == field)
}

pub(super) fn package_checksum(package: &LoadedPackage, native_hash: Option<&str>) -> String {
    let mut input = String::new();
    input.push_str("manifest\n");
    input.push_str(&package.manifest_source);
    input.push_str("\nsources\n");
    append_sources_hash_input(&mut input, &package.sources);
    if let Some(native_hash) = native_hash {
        input.push_str("\nnative\n");
        input.push_str(native_hash);
    }
    sha256_label(input.as_bytes())
}

pub(super) fn package_archive_files(package_dir: &Path) -> Result<Vec<PackageArchiveFile>, String> {
    let mut paths = Vec::new();
    collect_package_archive_paths(package_dir, package_dir, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| package_archive_file(package_dir, &path))
        .collect()
}

fn collect_package_archive_paths(
    root: &Path,
    path: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
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
        let name = entry.file_name();
        if should_skip_archive_entry(root, &path, &name.to_string_lossy()) {
            continue;
        }
        if path.is_dir() || path.is_file() {
            collect_package_archive_paths(root, &path, files)?;
        }
    }
    Ok(())
}

fn should_skip_archive_entry(root: &Path, path: &Path, name: &str) -> bool {
    if matches!(name, ".git" | "target" | "vendor" | ".DS_Store") {
        return true;
    }
    let relative = relative_path(root, path);
    matches!(
        relative.as_str(),
        "review/package-review.json" | "vendor/rss-vendor.json"
    )
}

fn package_archive_file(root: &Path, path: &Path) -> Result<PackageArchiveFile, String> {
    let contents =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(PackageArchiveFile {
        path: relative_path(root, path),
        size: contents.len() as u64,
        sha256: sha256_label(&contents),
    })
}

pub(super) fn package_archive_hash(files: &[PackageArchiveFile]) -> String {
    let mut input = String::new();
    input.push_str("rss.package.archive.v1\n");
    for file in files {
        input.push_str(&file.path);
        input.push('\n');
        input.push_str(&file.size.to_string());
        input.push('\n');
        input.push_str(&file.sha256);
        input.push('\n');
    }
    sha256_label(input.as_bytes())
}

pub(super) fn effective_interface_hash(sources: &[PackageSource], features: &[String]) -> String {
    let filtered = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .cloned()
        .collect::<Vec<_>>();
    let mut features = features.to_vec();
    features.sort();
    features.dedup();
    let mut input = String::new();
    input.push_str("features\n");
    for feature in features {
        input.push_str(&feature);
        input.push('\n');
    }
    input.push_str("interfaces\n");
    append_sources_hash_input(&mut input, &filtered);
    sha256_label(input.as_bytes())
}

fn append_sources_hash_input(input: &mut String, sources: &[PackageSource]) {
    let mut sources = sources.iter().collect::<Vec<_>>();
    sources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    for source in sources {
        input.push_str(&source.relative_path);
        input.push('\n');
        if source.kind == PackageReviewFileKind::Interface {
            input.push_str(&format_source(&source.path, &source.contents));
        } else {
            input.push_str(&source.contents);
        }
        input.push('\n');
    }
}

fn package_review_hash(review: &PackageReview) -> String {
    let mut input = String::new();
    input.push_str(&review.package.name);
    input.push('\n');
    input.push_str(&review.package.version);
    input.push('\n');
    input.push_str(&review.package.edition);
    input.push('\n');
    input.push_str(package_risk_label(review.risk));
    input.push('\n');
    for reason in &review.reasons {
        input.push_str(reason);
        input.push('\n');
    }
    for feature in &review.features {
        input.push_str(feature);
        input.push('\n');
    }
    input.push_str(&format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.diagnostics,
        review.summary.errors,
        review.summary.dependencies,
        review.summary.dev_dependencies,
        review.summary.package_features,
        review.summary.public_types,
        review.summary.public_functions,
        review.summary.public_apis,
        review.summary.mutating_apis,
        review.summary.retaining_apis,
        review.summary.resource_apis,
        review.summary.fresh_returning_apis,
        review.summary.native_apis,
        review.summary.unsafe_apis,
        review.summary.unknown_apis
    ));
    for export in &review.exports {
        input.push_str(&export.kind);
        input.push('\n');
        input.push_str(&export.name);
        input.push('\n');
        input.push_str(&export.classification);
        input.push('\n');
        for reason in &export.reasons {
            input.push_str(reason);
            input.push('\n');
        }
    }
    if let Some(native) = &review.native_rust {
        input.push_str(&native.path);
        input.push('\n');
        input.push_str(native.crate_name.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(native.build_scripts.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(native.proc_macros.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(native.unsafe_policy.as_deref().unwrap_or(""));
        input.push('\n');
        for link in &native.links {
            input.push_str(link);
            input.push('\n');
        }
    }
    for diagnostic in &review.diagnostics {
        input.push_str(&diagnostic.code);
        input.push('\n');
        input.push_str(&diagnostic.summary);
        input.push('\n');
    }
    sha256_label(input.as_bytes())
}

pub(super) fn package_native_hash(
    package_dir: &Path,
    native: Option<&ManifestNativeRust>,
) -> Result<Option<String>, String> {
    let Some(native) = native.filter(|native| native.enabled) else {
        return Ok(None);
    };
    let native_root = package_dir.join(native.path.as_deref().unwrap_or("native/rust"));
    let mut input = String::new();
    input.push_str(native.path.as_deref().unwrap_or("native/rust"));
    input.push('\n');
    input.push_str(native.crate_name.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.build_scripts.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.proc_macros.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.unsafe_policy.as_deref().unwrap_or(""));
    input.push('\n');
    for link in &native.links {
        input.push_str(link);
        input.push('\n');
    }

    if native_root.exists() {
        let mut files = Vec::new();
        collect_regular_files(&native_root, &mut files)?;
        files.sort();
        for file in files {
            input.push_str(&relative_path(package_dir, &file));
            input.push('\n');
            let contents = fs::read(&file)
                .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
            input.push_str(&sha256_label(&contents));
            input.push('\n');
        }
    }
    let binding_manifest = package_dir.join("native/bindings.rssbind.toml");
    if binding_manifest.exists() {
        input.push_str(&relative_path(package_dir, &binding_manifest));
        input.push('\n');
        let contents = fs::read(&binding_manifest)
            .map_err(|error| format!("failed to read {}: {error}", binding_manifest.display()))?;
        input.push_str(&sha256_label(&contents));
        input.push('\n');
    }

    Ok(Some(sha256_label(input.as_bytes())))
}

fn sha256_label(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::from("sha256:");
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
