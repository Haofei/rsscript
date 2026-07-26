use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::formatter::format_source;

use super::dependency::{DependencyResolutionScope, resolve_dependency_graph};
use super::review::review_package_dir_with_features;
use super::source_set::{ManifestNativeRust, load_package_with_features};
use super::{
    LoadedPackage, PackageArchiveFile, PackageLock, PackageLockDiff, PackageLockFieldChange,
    PackageLockMetadata, PackageLockPackage, PackageLockPackageChange, PackageReview,
    PackageReviewAwaitBoundary, PackageReviewFileKind, PackageRisk, PackageSource,
    collect_regular_files, ensure_package_path_within_root,
    package_feature_may_change_boundary_risk, package_path_metadata, package_risk_label,
    relative_path,
};

pub fn lock_package_dir(package_dir: &Path) -> Result<PackageLock, String> {
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Development)?;
    let root = &graph.nodes[&graph.root];
    let package = load_package_with_features(&root.package_dir, Some(&root.features))?;
    let mut packages = vec![lock_package_entry(
        &root.package_dir,
        &package,
        root.features.clone(),
    )?];
    for node in graph.dependency_order() {
        let package = load_package_with_features(&node.package_dir, Some(&node.features))?;
        packages.push(lock_package_entry(
            &node.package_dir,
            &package,
            node.features.clone(),
        )?);
    }
    validate_locked_package_identities(&packages)?;

    Ok(PackageLock {
        version: 1,
        packages,
        metadata: PackageLockMetadata {
            rsscript_version: env!("CARGO_PKG_VERSION").to_string(),
            created_by: "rsscript pkg".to_string(),
        },
    })
}

pub(super) fn lock_package_entry(
    package_dir: &Path,
    package: &LoadedPackage,
    features: Vec<String>,
) -> Result<PackageLockPackage, String> {
    let review = review_package_dir_with_features(package_dir, Some(&features))?;
    let native = package
        .manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref());
    let native_hash = package_native_hash(package_dir, native)?;

    Ok(PackageLockPackage {
        name: package.manifest.package.name.clone(),
        version: package.manifest.package.version.clone(),
        source: super::package_path_source(package_dir),
        checksum: package_checksum(package, native_hash.as_deref()),
        interface_hash: effective_interface_hash(&package.sources, &features),
        review_hash: package_review_hash(&review),
        native_hash,
        features,
    })
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
    let lock: PackageLock = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    validate_locked_package_identities(&lock.packages)
        .map_err(|error| format!("invalid package lock {}: {error}", path.display()))?;
    Ok(lock)
}

pub(super) fn compare_locked_packages(
    old_packages: &[PackageLockPackage],
    new_packages: &[PackageLockPackage],
) -> Vec<PackageLockPackageChange> {
    let mut old_matched = vec![false; old_packages.len()];
    let mut new_matched = vec![false; new_packages.len()];
    let mut pairs = Vec::new();

    for (old_index, old) in old_packages.iter().enumerate() {
        let Some(new_index) = new_packages.iter().enumerate().find_map(|(index, new)| {
            (!new_matched[index] && locked_package_identity(old) == locked_package_identity(new))
                .then_some(index)
        }) else {
            continue;
        };
        old_matched[old_index] = true;
        new_matched[new_index] = true;
        pairs.push((old_index, new_index));
    }

    let mut unmatched_by_name_source =
        BTreeMap::<(String, String), (Vec<usize>, Vec<usize>)>::new();
    for (index, package) in old_packages.iter().enumerate() {
        if !old_matched[index] {
            unmatched_by_name_source
                .entry((package.name.clone(), package.source.clone()))
                .or_default()
                .0
                .push(index);
        }
    }
    for (index, package) in new_packages.iter().enumerate() {
        if !new_matched[index] {
            unmatched_by_name_source
                .entry((package.name.clone(), package.source.clone()))
                .or_default()
                .1
                .push(index);
        }
    }
    for (_, (old_indices, new_indices)) in unmatched_by_name_source {
        if let ([old_index], [new_index]) = (old_indices.as_slice(), new_indices.as_slice()) {
            old_matched[*old_index] = true;
            new_matched[*new_index] = true;
            pairs.push((*old_index, *new_index));
        }
    }

    let mut changes = Vec::new();
    for (old_index, new_index) in pairs {
        let old = &old_packages[old_index];
        let new = &new_packages[new_index];
        let field_changes = compare_locked_package_fields(old, new);
        if !field_changes.is_empty() {
            let risk = field_changes
                .iter()
                .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
            changes.push(PackageLockPackageChange {
                name: old.name.clone(),
                before_version: Some(old.version.clone()),
                after_version: Some(new.version.clone()),
                risk,
                changes: field_changes,
            });
        }
    }
    for (index, old) in old_packages.iter().enumerate() {
        if !old_matched[index] {
            changes.push(removed_locked_package_change(old));
        }
    }
    for (index, new) in new_packages.iter().enumerate() {
        if !new_matched[index] {
            changes.push(added_locked_package_change(new));
        }
    }
    changes.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.before_version.cmp(&right.before_version))
            .then(left.after_version.cmp(&right.after_version))
            .then_with(|| lock_change_source(left).cmp(&lock_change_source(right)))
    });
    changes
}

fn lock_change_source(change: &PackageLockPackageChange) -> (Option<&str>, Option<&str>) {
    change
        .changes
        .iter()
        .find(|field| field.field == "source")
        .map(|field| (field.before.as_deref(), field.after.as_deref()))
        .unwrap_or((None, None))
}

fn locked_package_identity(package: &PackageLockPackage) -> (&str, &str, &str) {
    (&package.name, &package.version, &package.source)
}

fn validate_locked_package_identities(packages: &[PackageLockPackage]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for package in packages {
        let identity = (
            package.name.clone(),
            package.version.clone(),
            package.source.clone(),
        );
        if !seen.insert(identity) {
            return Err(format!(
                "duplicate package identity `{}@{}` from `{}`",
                package.name, package.version, package.source
            ));
        }
    }
    Ok(())
}

fn added_locked_package_change(package: &PackageLockPackage) -> PackageLockPackageChange {
    PackageLockPackageChange {
        name: package.name.clone(),
        before_version: None,
        after_version: Some(package.version.clone()),
        risk: PackageRisk::Elevated,
        changes: vec![
            PackageLockFieldChange {
                field: "package".to_string(),
                before: None,
                after: Some("added".to_string()),
                risk: PackageRisk::Elevated,
            },
            PackageLockFieldChange {
                field: "source".to_string(),
                before: None,
                after: Some(package.source.clone()),
                risk: PackageRisk::Elevated,
            },
        ],
    }
}

fn removed_locked_package_change(package: &PackageLockPackage) -> PackageLockPackageChange {
    PackageLockPackageChange {
        name: package.name.clone(),
        before_version: Some(package.version.clone()),
        after_version: None,
        risk: PackageRisk::High,
        changes: vec![
            PackageLockFieldChange {
                field: "package".to_string(),
                before: Some("present".to_string()),
                after: None,
                risk: PackageRisk::High,
            },
            PackageLockFieldChange {
                field: "source".to_string(),
                before: Some(package.source.clone()),
                after: None,
                risk: PackageRisk::High,
            },
        ],
    }
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
    let root = std::fs::canonicalize(package_dir)
        .map_err(|error| format!("failed to canonicalize {}: {error}", package_dir.display()))?;
    collect_package_archive_paths(package_dir, &root, package_dir, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| package_archive_file(package_dir, &path))
        .collect()
}

fn collect_package_archive_paths(
    root: &Path,
    canonical_root: &Path,
    path: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let metadata = package_path_metadata(path, "package archive")?;
    ensure_package_path_within_root(canonical_root, path, "package archive")?;
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
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
        collect_package_archive_paths(root, canonical_root, &path, files)?;
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
    ) || relative.starts_with("review/reir/")
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
    for implementation in &review.implements {
        input.push_str(&implementation.interface_package);
        input.push('\n');
        input.push_str(implementation.version.as_deref().unwrap_or(""));
        input.push('\n');
        for feature in &implementation.interface_features {
            input.push_str(feature);
            input.push('\n');
        }
        input.push_str(
            implementation
                .interface_effective_hash
                .as_deref()
                .unwrap_or(""),
        );
        input.push('\n');
    }
    input.push_str(&format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.diagnostics,
        review.summary.errors,
        review.summary.dependencies,
        review.summary.dev_dependencies,
        review.summary.package_features,
        review.summary.public_types,
        review.summary.public_sum_types,
        review.summary.public_type_aliases,
        review.summary.public_consts,
        review.summary.public_functions,
        review.summary.public_apis,
        review.summary.mutating_apis,
        review.summary.retaining_apis,
        review.summary.resource_apis,
        review.summary.fresh_returning_apis,
        review.summary.native_apis,
        review.summary.async_apis,
        review.summary.await_sites,
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
        input.push_str(export.function_kind.as_deref().unwrap_or(""));
        input.push('\n');
        for effect in &export.normalized_effects {
            input.push_str(effect);
            input.push('\n');
        }
        for reason in &export.reasons {
            input.push_str(reason);
            input.push('\n');
        }
    }
    for await_site in &review.await_sites {
        input.push_str(&await_site.function);
        input.push('\n');
        input.push_str(await_site.callee.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(await_boundary_hash_label(await_site.boundary));
        input.push('\n');
        for live_value in &await_site.live_across_await {
            input.push_str(live_value);
            input.push('\n');
        }
        input.push_str(&await_site.span.file);
        input.push('\n');
        input.push_str(&await_site.span.line.to_string());
        input.push(':');
        input.push_str(&await_site.span.column.to_string());
        input.push(':');
        input.push_str(&await_site.span.length.to_string());
        input.push('\n');
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
    // Capabilities (incl. provider/service/action/resource) are part of the
    // review identity: a provider swap or re-classification that keeps the same
    // summary counts must still change the review hash, so the lock catches it.
    for capability in &review.capabilities {
        input.push_str(&capability.binding_symbol);
        input.push('\n');
        input.push_str(&capability.category);
        input.push('\n');
        input.push_str(capability.provider.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(capability.service.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(capability.action.as_deref().unwrap_or(""));
        input.push('\n');
        input.push_str(capability.resource.as_deref().unwrap_or(""));
        input.push('\n');
    }
    sha256_label(input.as_bytes())
}

fn await_boundary_hash_label(boundary: PackageReviewAwaitBoundary) -> &'static str {
    match boundary {
        PackageReviewAwaitBoundary::RuntimePending => "runtime_pending",
        PackageReviewAwaitBoundary::NativePending => "native_pending",
        PackageReviewAwaitBoundary::RssCall => "rss_call",
        PackageReviewAwaitBoundary::Unknown => "unknown",
    }
}

pub(super) fn package_native_hash(
    package_dir: &Path,
    native: Option<&ManifestNativeRust>,
) -> Result<Option<String>, String> {
    let Some(native) = native.filter(|native| native.enabled) else {
        return Ok(None);
    };
    let native_root = super::native::confined_native_rust_path(
        package_dir,
        native.path.as_deref().unwrap_or("native/rust"),
    )?;
    let mut input = String::new();
    input.push_str(native.path.as_deref().unwrap_or("native/rust"));
    input.push('\n');
    input.push_str(native.crate_name.as_deref().unwrap_or(""));
    input.push('\n');
    for feature in &native.cargo_features {
        input.push_str(feature);
        input.push('\n');
    }
    for (feature, mapping) in &native.feature_map {
        input.push_str(feature);
        input.push('\n');
        for cargo_feature in &mapping.cargo_features {
            input.push_str(cargo_feature);
            input.push('\n');
        }
    }
    input.push_str(native.build_scripts.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.policy.build_scripts.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.proc_macros.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.policy.proc_macros.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.unsafe_policy.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.policy.rss_unsafe_apis.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.policy.wrapper_unsafe_blocks.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(
        native
            .policy
            .transitive_unsafe_blocks
            .as_deref()
            .unwrap_or(""),
    );
    input.push('\n');
    input.push_str(native.policy.native_links.as_deref().unwrap_or(""));
    input.push('\n');
    input.push_str(native.policy.ffi.as_deref().unwrap_or(""));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn locked(name: &str, version: &str, source: &str) -> PackageLockPackage {
        PackageLockPackage {
            name: name.to_string(),
            version: version.to_string(),
            source: source.to_string(),
            checksum: format!("checksum:{source}"),
            interface_hash: "interface".to_string(),
            review_hash: "review".to_string(),
            native_hash: None,
            features: Vec::new(),
        }
    }

    #[test]
    fn lock_diff_preserves_same_name_multi_source_instances() {
        let first = locked("shared", "1.0.0", "path+/first");
        let second = locked("shared", "2.0.0", "path+/second");

        let changes = compare_locked_packages(
            &[first.clone(), second.clone()],
            std::slice::from_ref(&first),
        );

        assert_eq!(changes.len(), 1, "{changes:#?}");
        assert_eq!(changes[0].name, "shared");
        assert_eq!(changes[0].before_version.as_deref(), Some("2.0.0"));
        assert_eq!(changes[0].after_version, None);
        assert!(changes[0].changes.iter().any(|change| {
            change.field == "source"
                && change.before.as_deref() == Some("path+/second")
                && change.after.is_none()
        }));
    }

    #[test]
    fn lock_diff_matches_exact_identity_before_pairing_version_changes() {
        let stable = locked("shared", "1.0.0", "path+/stable");
        let old_changed = locked("shared", "1.0.0", "path+/changing");
        let new_changed = locked("shared", "2.0.0", "path+/changing");

        let changes =
            compare_locked_packages(&[stable.clone(), old_changed], &[new_changed, stable]);

        assert_eq!(changes.len(), 1, "{changes:#?}");
        assert_eq!(changes[0].before_version.as_deref(), Some("1.0.0"));
        assert_eq!(changes[0].after_version.as_deref(), Some("2.0.0"));
        assert!(changes[0].changes.iter().any(|change| {
            change.field == "version"
                && change.before.as_deref() == Some("1.0.0")
                && change.after.as_deref() == Some("2.0.0")
        }));
        assert!(
            changes[0]
                .changes
                .iter()
                .all(|change| change.field != "source")
        );
    }

    #[test]
    fn lock_diff_keeps_same_name_version_different_sources_distinct() {
        let first = locked("shared", "1.0.0", "path+/first");
        let second = locked("shared", "1.0.0", "path+/second");

        let changes =
            compare_locked_packages(std::slice::from_ref(&first), &[first.clone(), second]);

        assert_eq!(changes.len(), 1, "{changes:#?}");
        assert_eq!(changes[0].before_version, None);
        assert_eq!(changes[0].after_version.as_deref(), Some("1.0.0"));
        assert!(changes[0].changes.iter().any(|change| {
            change.field == "source"
                && change.before.is_none()
                && change.after.as_deref() == Some("path+/second")
        }));
    }

    #[test]
    fn lock_diff_does_not_guess_between_ambiguous_version_changes() {
        let old_first = locked("shared", "1.0.0", "path+/same");
        let old_second = locked("shared", "2.0.0", "path+/same");
        let new = locked("shared", "3.0.0", "path+/same");

        let changes = compare_locked_packages(&[old_first, old_second], &[new]);

        assert_eq!(changes.len(), 3, "{changes:#?}");
        assert!(
            changes
                .iter()
                .all(|change| change.changes.iter().all(|field| field.field != "version")),
            "{changes:#?}"
        );
        assert_eq!(
            changes
                .iter()
                .filter(|change| change.before_version.is_some())
                .count(),
            2
        );
        assert_eq!(
            changes
                .iter()
                .filter(|change| change.after_version.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn lock_reader_rejects_duplicate_exact_identities() {
        let package = locked("duplicate", "1.0.0", "path+/same");
        let lock = PackageLock {
            version: 1,
            packages: vec![package.clone(), package],
            metadata: PackageLockMetadata {
                rsscript_version: "test".to_string(),
                created_by: "test".to_string(),
            },
        };
        let path = std::env::temp_dir().join(format!(
            "rsscript-duplicate-lock-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        fs::write(&path, toml::to_string(&lock).expect("lock serializes"))
            .expect("lock should be written");

        let error = read_package_lock(&path).expect_err("duplicate identity must be rejected");
        let _ = fs::remove_file(path);

        assert!(error.contains("duplicate package identity"), "{error}");
        assert!(error.contains("duplicate@1.0.0"), "{error}");
        assert!(error.contains("path+/same"), "{error}");
    }

    #[test]
    fn lock_identity_allows_same_name_version_from_different_sources() {
        validate_locked_package_identities(&[
            locked("shared", "1.0.0", "path+/first"),
            locked("shared", "1.0.0", "path+/second"),
        ])
        .expect("source is part of the exact lock identity");
    }
}
