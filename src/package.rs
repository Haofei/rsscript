use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::analyzer::{
    analyze_source_with_interfaces, analyze_sources_with_interfaces, core_interfaces,
};
use crate::diagnostic::{Diagnostic, code};
use crate::lint::lint_source;
use crate::review::{
    ReviewFinding, ReviewMap, ReviewMapClassification, ReviewRisk, review_map_sources,
    review_sources,
};
use crate::syntax::ast::TypeKind;

mod contract;
mod format;
mod graph;
mod lock;
mod native;
mod policy;
mod source_set;
mod types;

use contract::{
    PackageFunctionContract, collect_package_function_contracts, collect_package_type_contracts,
    package_added_function_contract_is_high_risk, package_added_type_contract_is_high_risk,
    package_contract_has_resource_boundary, package_function_contract_boundary_changed,
    package_function_contracts_for_source, package_function_contracts_match,
    package_interface_contract_diagnostics, package_interface_diagnostic_exports,
    package_interface_environment_diagnostics, package_review_exports,
    package_type_contract_boundary_changed, package_type_contracts_for_source,
    package_type_contracts_match,
};
pub use format::*;
use graph::check_package_graph;
pub use graph::package_tree;
use lock::{
    compare_locked_packages, lock_package_entry, package_archive_files, package_archive_hash,
    package_checksum, package_lock_diff_reasons, package_native_hash, read_package_lock,
};
pub use lock::{diff_package_locks, lock_package_dir};
use native::{
    check_package_native_rust, manifest_native_enabled, manifest_native_unsafe_boundary,
    native_binding_interface_sources, native_effective_build_policy,
    package_native_binding_diagnostics, package_native_bindings, package_native_rust_dependencies,
};
use policy::{
    collect_manifest_review_policy_diagnostics, collect_manifest_review_policy_violations,
    package_review_policy_has_high_risk_violation, package_review_policy_ok,
};
use source_set::{
    LoadedPackage, Manifest, ManifestNativeRust, PackageSource, load_package,
    load_package_manifest, load_package_with_features, resolve_package_features,
    selected_root_package_features,
};
pub use types::*;

#[derive(Debug, Clone)]
struct PackageDependencySpec {
    name: String,
    requirement: Option<String>,
    path: Option<String>,
    git: Option<String>,
    features: Vec<String>,
}

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    let package = load_package(package_dir)?;
    let manifest = &package.manifest;
    let sources = &package.sources;
    let dependency_interfaces = collect_dependency_interface_sources(package_dir, manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces = native_binding_interface_sources(sources, &native_bindings);

    let interface_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let dependency_interface_refs = dependency_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let core_interface_refs = core_interfaces().to_vec();
    let contract_external_interfaces = dependency_interface_refs.clone();
    let mut external_interfaces = core_interface_refs;
    external_interfaces.extend(dependency_interface_refs);
    let mut combined_interfaces = contract_external_interfaces.clone();
    combined_interfaces.extend(interface_refs.clone());
    let native_binding_interface_refs = native_binding_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let mut source_interfaces = external_interfaces.clone();
    source_interfaces.extend(native_binding_interface_refs);
    let source_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let interface_frontend_diagnostics = interface_refs
        .iter()
        .flat_map(|(path, contents)| {
            analyze_source_with_interfaces(path, contents, &contract_external_interfaces)
        })
        .collect::<Vec<_>>();
    let interface_diagnostic_exports =
        package_interface_diagnostic_exports(sources, &interface_frontend_diagnostics);
    let mut diagnostics = package_interface_environment_diagnostics(&combined_interfaces);
    diagnostics.extend(package_feature_resolution_diagnostics(
        package_dir,
        manifest,
    )?);
    diagnostics.extend(interface_frontend_diagnostics);
    diagnostics.extend(analyze_sources_with_interfaces(
        &source_refs,
        &source_interfaces,
    ));
    diagnostics.extend(package_interface_contract_diagnostics(
        sources,
        &native_bindings,
    ));
    diagnostics.extend(package_native_binding_diagnostics(
        package_dir,
        sources,
        &native_bindings,
        manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
    ));
    diagnostics.extend(package_lint_diagnostics(sources));
    dedup_diagnostics(&mut diagnostics);
    let review_map = review_map_sources(
        sources
            .iter()
            .map(|source| (source.path.as_str(), source.contents.as_str()))
            .collect(),
    );

    let native_rust = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .filter(|native| native.enabled)
        .map(|native| PackageNativeRustReview {
            path: native
                .path
                .clone()
                .unwrap_or_else(|| "native/rust".to_string()),
            crate_name: native.crate_name.clone(),
            build_scripts: native_effective_build_policy(manifest, native.build_scripts.as_deref()),
            proc_macros: native_effective_build_policy(manifest, native.proc_macros.as_deref()),
            unsafe_policy: native.unsafe_policy.clone(),
            links: native.links.clone(),
        });

    let mut reasons = Vec::new();
    collect_manifest_review_reasons(manifest, &mut reasons);
    collect_native_reasons(native_rust.as_ref(), &mut reasons);
    collect_review_map_reasons(&review_map, &mut reasons);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        reasons.push("package contains frontend errors".to_string());
    } else if !diagnostics.is_empty() {
        reasons.push("package contains frontend warnings".to_string());
    }
    if !interface_diagnostic_exports.is_empty() {
        reasons.push("public .rssi contract contains frontend errors".to_string());
    }
    reasons.sort();
    reasons.dedup();

    let api_summary = package_api_effect_summary(sources, &review_map);
    let risk = if interface_diagnostic_exports.is_empty() {
        package_risk(
            manifest,
            native_rust.as_ref(),
            &review_map,
            &diagnostics,
            api_summary.native_apis,
        )
    } else {
        PackageRisk::Unknown
    };
    let summary = PackageReviewSummary {
        interface_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Interface)
            .count(),
        source_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Source)
            .count(),
        diagnostics: diagnostics.len(),
        errors: diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity.is_error())
            .count(),
        dependencies: manifest.dependencies.len(),
        dev_dependencies: manifest.dev_dependencies.len(),
        package_features: manifest.features.len(),
        public_types: api_summary.public_types,
        public_functions: api_summary.public_functions,
        public_apis: api_summary.public_apis,
        mutating_apis: api_summary.mutating_apis,
        retaining_apis: api_summary.retaining_apis,
        resource_apis: api_summary.resource_apis,
        fresh_returning_apis: api_summary.fresh_returning_apis,
        native_apis: api_summary.native_apis,
        unsafe_apis: api_summary.unsafe_apis,
        unknown_apis: api_summary.unknown_apis + interface_diagnostic_exports.len(),
    };
    let files = sources
        .iter()
        .map(|source| PackageReviewFile {
            path: source.path.clone(),
            kind: source.kind,
        })
        .collect();
    let features = manifest.features.keys().cloned().collect::<Vec<_>>();
    let mut exports = package_review_exports(sources, &review_map);
    exports.extend(interface_diagnostic_exports);
    exports.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(PackageReview {
        package: PackageIdentity {
            name: manifest.package.name.clone(),
            version: manifest.package.version.clone(),
            edition: manifest.package.edition.clone(),
        },
        manifest_path: package.manifest_path.display().to_string(),
        risk,
        reasons,
        features,
        summary,
        files,
        exports,
        native_rust,
        review_map,
        diagnostics,
    })
}

fn package_lint_diagnostics(sources: &[PackageSource]) -> Vec<Diagnostic> {
    sources
        .iter()
        .flat_map(|source| lint_source(&source.path, &source.contents))
        .collect()
}

pub fn package_metadata(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageMetadataReport, String> {
    let review = review_package_dir(package_dir)?;
    let metadata_path = package_dir.join("review").join("package-review.json");
    let metadata = package_review_metadata_from_review(&review);
    let ok = review.summary.errors == 0 && review.risk != PackageRisk::Unknown;

    if !dry_run {
        let parent = metadata_path
            .parent()
            .ok_or_else(|| format!("metadata path has no parent: {}", metadata_path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        let json = serde_json::to_string_pretty(&metadata)
            .expect("package metadata JSON serialization should not fail");
        fs::write(&metadata_path, json)
            .map_err(|error| format!("failed to write {}: {error}", metadata_path.display()))?;
    }

    Ok(PackageMetadataReport {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        metadata_path: metadata_path.display().to_string(),
        dry_run,
        written: !dry_run,
        ok,
        risk: review.risk,
        reasons: review.reasons,
        metadata,
    })
}

pub fn package_lowering_input(package_dir: &Path) -> Result<PackageLoweringInput, String> {
    let package = load_package(package_dir)?;
    let dependency_interfaces =
        collect_dependency_interface_sources(package_dir, &package.manifest)?;
    let native_dependencies = package_native_rust_dependencies(package_dir, &package.manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces =
        native_binding_interface_sources(&package.sources, &native_bindings);
    let interfaces = dependency_interfaces
        .iter()
        .chain(native_binding_interfaces.iter())
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();

    let source_files = package
        .sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .collect::<Vec<_>>();
    let source = select_package_runnable_source(&source_files)?;
    let sources = source_files
        .iter()
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();
    Ok(PackageLoweringInput {
        package: PackageIdentity {
            name: package.manifest.package.name.clone(),
            version: package.manifest.package.version.clone(),
            edition: package.manifest.package.edition.clone(),
        },
        package_dir: package_dir.display().to_string(),
        source_path: source.path.clone(),
        source_relative_path: source.relative_path.clone(),
        source: source.contents.clone(),
        sources,
        interfaces,
        native_dependencies,
    })
}

pub fn diff_package_dirs(old_dir: &Path, new_dir: &Path) -> Result<PackageDiff, String> {
    let old_package = load_package(old_dir)?;
    let new_package = load_package(new_dir)?;
    let old_review = review_package_dir(old_dir)?;
    let new_review = review_package_dir(new_dir)?;

    let mut manifest_changes = Vec::new();
    compare_package_identity(
        &old_package.manifest,
        &new_package.manifest,
        &mut manifest_changes,
    );
    compare_value_maps(
        "dependency",
        &old_package.manifest.dependencies,
        &new_package.manifest.dependencies,
        PackageRisk::High,
        &mut manifest_changes,
    );
    compare_value_maps(
        "dev-dependency",
        &old_package.manifest.dev_dependencies,
        &new_package.manifest.dev_dependencies,
        PackageRisk::Elevated,
        &mut manifest_changes,
    );
    compare_feature_maps(
        &old_package.manifest.features,
        &new_package.manifest.features,
        &mut manifest_changes,
    );
    compare_native_rust(
        old_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
        new_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
        &mut manifest_changes,
    );

    let interface_changes = compare_interface_sources(&old_package.sources, &new_package.sources);
    let mut reasons = package_diff_reasons(&manifest_changes, &interface_changes);
    if old_review.risk != new_review.risk {
        reasons.push(format!(
            "package risk changed from {} to {}",
            package_risk_label(old_review.risk),
            package_risk_label(new_review.risk)
        ));
    }
    reasons.sort();
    reasons.dedup();
    let risk = package_diff_risk(
        &manifest_changes,
        &interface_changes,
        old_review.risk,
        new_review.risk,
    );

    Ok(PackageDiff {
        old_package: package_identity(&old_package.manifest),
        new_package: package_identity(&new_package.manifest),
        risk,
        reasons,
        manifest_changes,
        interface_changes,
        old_review: old_review.summary,
        new_review: new_review.summary,
    })
}

pub fn check_package_dir(package_dir: &Path) -> Result<PackageCheck, String> {
    let package = load_package(package_dir)?;
    let review = review_package_dir(package_dir)?;
    let current_lock = lock_package_dir(package_dir)?;
    let graph = check_package_graph(package_dir)?;
    let lock = check_package_lock(package_dir, &current_lock)?;
    let native_rust = check_package_native_rust(package_dir, review.native_rust.as_ref())?;

    let mut reasons = review.reasons.clone();
    reasons.extend(graph.reasons.clone());
    reasons.extend(lock.reasons.clone());
    if let Some(native) = &native_rust {
        reasons.extend(native.reasons.clone());
    }
    collect_manifest_review_policy_violations(
        &package.manifest,
        &review,
        native_rust.as_ref(),
        &mut reasons,
    );
    reasons.sort();
    reasons.dedup();

    let mut diagnostics = review.diagnostics.clone();
    diagnostics.extend(collect_manifest_review_policy_diagnostics(
        &package.manifest,
        package_dir,
        &review,
        native_rust.as_ref(),
        &package.sources,
    ));
    dedup_diagnostics(&mut diagnostics);
    let diagnostics_have_errors = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error());
    let native_ok = native_rust
        .as_ref()
        .is_none_or(|native_check| native_check.ok);
    let policy_ok = package_review_policy_ok(&package.manifest, &review, native_rust.as_ref());
    let ok = !diagnostics_have_errors && policy_ok && graph.ok && lock.matches && native_ok;
    let mut risk = review.risk.max(graph.risk).max(lock.risk);
    if let Some(native) = &native_rust {
        risk = risk.max(native.risk);
    }
    if diagnostics_have_errors {
        risk = risk.max(PackageRisk::High);
    }
    if package_review_policy_has_high_risk_violation(
        &package.manifest,
        &review,
        native_rust.as_ref(),
    ) {
        risk = risk.max(PackageRisk::High);
    }

    let mut summary = review.summary;
    summary.diagnostics = diagnostics.len();
    summary.errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
        .count();

    Ok(PackageCheck {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        ok,
        risk,
        reasons,
        summary,
        graph,
        lock,
        native_rust,
        diagnostics,
    })
}

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
    let root_lock_entry = lock_package_entry(package_dir, &package, root_features)?;

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

fn select_package_runnable_source<'a>(
    source_files: &[&'a PackageSource],
) -> Result<&'a PackageSource, String> {
    if source_files.is_empty() {
        return Err("rss run requires one package source file under `src`.".to_string());
    }

    let main_sources = source_files
        .iter()
        .copied()
        .filter(|source| source.relative_path == "src/main.rss")
        .collect::<Vec<_>>();
    if source_files.len() == 1 {
        return Ok(source_files[0]);
    }
    if main_sources.len() == 1 {
        return Ok(main_sources[0]);
    }

    Err(
        "rss run package lowering requires `src/main.rss` when a package has multiple `.rss` source files."
            .to_string(),
    )
}

fn collect_dependency_interface_sources(
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

fn package_feature_resolution_diagnostics(
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
        package_dependency_span(package_dir, dependency),
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PackageApiSummary {
    public_types: usize,
    public_functions: usize,
    public_apis: usize,
    mutating_apis: usize,
    retaining_apis: usize,
    resource_apis: usize,
    fresh_returning_apis: usize,
    native_apis: usize,
    unsafe_apis: usize,
    unknown_apis: usize,
}

fn package_api_effect_summary(
    sources: &[PackageSource],
    review_map: &ReviewMap,
) -> PackageApiSummary {
    let interface_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_contracts;
    let source_type_contracts;
    let contracts = if interface_contracts.is_empty() {
        source_contracts =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_contracts
    } else {
        &interface_contracts
    };
    let type_contracts = if interface_type_contracts.is_empty() {
        source_type_contracts =
            collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_type_contracts
    } else {
        &interface_type_contracts
    };
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();

    PackageApiSummary {
        public_types: type_contracts.len(),
        public_functions: contracts.len(),
        public_apis: type_contracts.len() + contracts.len(),
        mutating_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .params
                    .iter()
                    .any(|param| param.effect == Some("mut"))
            })
            .count(),
        retaining_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.starts_with("retains("))
            })
            .count(),
        resource_apis: contracts
            .values()
            .filter(|contract| package_contract_has_resource_boundary(contract, &resource_types))
            .count(),
        fresh_returning_apis: contracts
            .values()
            .filter(|contract| contract.returns_fresh)
            .count(),
        native_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "native")
            })
            .count(),
        unsafe_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "unsafe")
            })
            .count(),
        unknown_apis: package_unknown_api_count(contracts, review_map),
    }
}

fn package_unknown_api_count(
    contracts: &BTreeMap<String, PackageFunctionContract>,
    review_map: &ReviewMap,
) -> usize {
    contracts
        .keys()
        .filter(|function| {
            review_map.files.iter().any(|file| {
                file.regions.iter().any(|region| {
                    &region.function == *function
                        && region.classification == ReviewMapClassification::Unknown
                })
            })
        })
        .count()
}

fn dedup_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
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

fn package_review_metadata_from_review(review: &PackageReview) -> PackageReviewMetadata {
    PackageReviewMetadata {
        schema: "rss.review.package.v1".to_string(),
        package: review.package.clone(),
        risk: review.risk,
        reasons: review.reasons.clone(),
        features: review.features.clone(),
        summary: review.summary.clone(),
        files: review.files.clone(),
        exports: review.exports.clone(),
        native_rust: review.native_rust.clone(),
        review_map: review.review_map.clone(),
        diagnostics: review.diagnostics.clone(),
    }
}

fn check_package_lock(
    package_dir: &Path,
    current_lock: &PackageLock,
) -> Result<PackageCheckLock, String> {
    let lock_path = package_dir.join("rsspkg.lock");
    if !lock_path.exists() {
        return Ok(PackageCheckLock {
            path: lock_path.display().to_string(),
            present: false,
            matches: false,
            risk: PackageRisk::Elevated,
            reasons: vec!["rsspkg.lock missing".to_string()],
            package_changes: Vec::new(),
        });
    }

    let locked = read_package_lock(&lock_path)?;
    let package_changes = compare_locked_packages(&locked.packages, &current_lock.packages);
    let mut reasons = package_lock_diff_reasons(&package_changes);
    if locked.version != current_lock.version {
        reasons.push("lockfile format version changed".to_string());
    }
    reasons.sort();
    reasons.dedup();
    let mut risk = package_changes
        .iter()
        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
    if locked.version != current_lock.version {
        risk = risk.max(PackageRisk::Elevated);
    }

    Ok(PackageCheckLock {
        path: lock_path.display().to_string(),
        present: true,
        matches: reasons.is_empty(),
        risk,
        reasons,
        package_changes,
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

fn package_dependency_spec(name: &str, value: &toml::Value) -> PackageDependencySpec {
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

fn package_registry_index_entry(
    package: &LoadedPackage,
    lock_entry: &PackageLockPackage,
    native_check: Option<&PackageNativeRustCheck>,
    risk: PackageRisk,
    archive_hash: &str,
) -> PackageRegistryIndexEntry {
    PackageRegistryIndexEntry {
        schema: "rss.registry.index.v1".to_string(),
        name: package.manifest.package.name.clone(),
        version: package.manifest.package.version.clone(),
        checksum: archive_hash.to_string(),
        interface_hash: lock_entry.interface_hash.clone(),
        review_hash: lock_entry.review_hash.clone(),
        native_hash: lock_entry.native_hash.clone(),
        risk,
        native: package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref())
            .is_some_and(|native| native.enabled),
        unsafe_boundary: package_index_unsafe_boundary(&package.manifest, native_check),
        dependencies: package_index_dependencies(&package.manifest.dependencies),
    }
}

fn package_registry_publish_target(
    registry_dir: &Path,
    package_name: &str,
    package_version: &str,
) -> PackageRegistryPublishTarget {
    let package_component = sanitize_vendor_path_component(package_name);
    let version_component = sanitize_vendor_path_component(package_version);
    PackageRegistryPublishTarget {
        registry_dir: registry_dir.display().to_string(),
        index_path: registry_dir
            .join("index")
            .join(&package_component)
            .join(format!("{version_component}.json"))
            .display()
            .to_string(),
        archive_manifest_path: registry_dir
            .join("archives")
            .join(&package_component)
            .join(&version_component)
            .join("archive-manifest.json")
            .display()
            .to_string(),
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
        .and_then(|native| native.unsafe_policy.as_deref())
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
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

fn vendor_package_dir_name(identity: &PackageIdentity) -> String {
    format!(
        "{}-{}",
        sanitize_vendor_path_component(&identity.name),
        sanitize_vendor_path_component(&identity.version)
    )
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

fn collect_manifest_review_reasons(manifest: &Manifest, reasons: &mut Vec<String>) {
    if !manifest.features.is_empty() {
        reasons.push("package declares selectable package features".to_string());
    }
    collect_package_feature_boundary_reasons(&manifest.features, reasons);
    if let Some(review) = &manifest.review {
        if review.expect.risk.as_deref() == Some("unknown") {
            reasons.push("manifest declares unknown package risk".to_string());
        }
        if review.policy.deny_unknown == Some(true) {
            reasons.push("package policy denies unknown review risk".to_string());
        }
        if review.policy.deny_native == Some(true) {
            reasons.push("package policy denies native boundaries".to_string());
        }
        if review.policy.deny_unsafe_apis == Some(true) {
            reasons.push("package policy denies unsafe APIs".to_string());
        }
    }
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

fn collect_native_reasons(native: Option<&PackageNativeRustReview>, reasons: &mut Vec<String>) {
    let Some(native) = native else {
        return;
    };
    reasons.push("native Rust wrapper enabled".to_string());
    if native
        .build_scripts
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust build scripts require review".to_string());
    }
    if native
        .proc_macros
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust proc macros require review".to_string());
    }
    if native
        .unsafe_policy
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust unsafe policy requires review".to_string());
    }
    if !native.links.is_empty() {
        reasons.push("native Rust links external libraries".to_string());
    }
}

fn collect_review_map_reasons(review_map: &ReviewMap, reasons: &mut Vec<String>) {
    if review_map.summary.unknown.functions > 0 {
        reasons.push("review map contains unknown functions".to_string());
    }
    if review_map.summary.review_required.functions > 0 {
        reasons.push("review map contains must-review functions".to_string());
    }
}

fn package_identity(manifest: &Manifest) -> PackageIdentity {
    PackageIdentity {
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        edition: manifest.package.edition.clone(),
    }
}

fn compare_package_identity(
    old: &Manifest,
    new: &Manifest,
    changes: &mut Vec<PackageManifestChange>,
) {
    if old.package.name != new.package.name {
        changes.push(manifest_change(
            "package",
            "name",
            Some(old.package.name.clone()),
            Some(new.package.name.clone()),
            PackageRisk::High,
        ));
    }
    if old.package.version != new.package.version {
        changes.push(manifest_change(
            "package",
            "version",
            Some(old.package.version.clone()),
            Some(new.package.version.clone()),
            PackageRisk::Elevated,
        ));
    }
    if old.package.edition != new.package.edition {
        changes.push(manifest_change(
            "package",
            "edition",
            Some(old.package.edition.clone()),
            Some(new.package.edition.clone()),
            PackageRisk::Elevated,
        ));
    }
}

fn compare_value_maps(
    kind: &str,
    old: &BTreeMap<String, toml::Value>,
    new: &BTreeMap<String, toml::Value>,
    risk: PackageRisk,
    changes: &mut Vec<PackageManifestChange>,
) {
    let names = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.get(&name).map(toml_value_label);
        let after = new.get(&name).map(toml_value_label);
        if before != after {
            changes.push(manifest_change(kind, name, before, after, risk));
        }
    }
}

fn compare_feature_maps(
    old: &BTreeMap<String, Vec<String>>,
    new: &BTreeMap<String, Vec<String>>,
    changes: &mut Vec<PackageManifestChange>,
) {
    let names = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.get(&name).map(|values| feature_values_label(values));
        let after = new.get(&name).map(|values| feature_values_label(values));
        if before != after {
            let risk = new
                .get(&name)
                .or_else(|| old.get(&name))
                .map_or(PackageRisk::Elevated, |values| {
                    package_feature_risk(&name, values)
                });
            changes.push(manifest_change(
                "package-feature",
                name,
                before,
                after,
                risk,
            ));
        }
    }
}

fn package_feature_risk(name: &str, values: &[String]) -> PackageRisk {
    if package_feature_may_change_boundary_risk(name, values) {
        PackageRisk::High
    } else {
        PackageRisk::Elevated
    }
}

fn compare_native_rust(
    old: Option<&ManifestNativeRust>,
    new: Option<&ManifestNativeRust>,
    changes: &mut Vec<PackageManifestChange>,
) {
    let old_enabled = old.is_some_and(|native| native.enabled);
    let new_enabled = new.is_some_and(|native| native.enabled);
    if old_enabled != new_enabled {
        changes.push(manifest_change(
            "native-rust",
            "enabled",
            Some(old_enabled.to_string()),
            Some(new_enabled.to_string()),
            PackageRisk::High,
        ));
    }
    compare_optional_native_field(
        "path",
        old.and_then(|native| native.path.as_deref()),
        new.and_then(|native| native.path.as_deref()),
        PackageRisk::Elevated,
        changes,
    );
    compare_optional_native_field(
        "crate",
        old.and_then(|native| native.crate_name.as_deref()),
        new.and_then(|native| native.crate_name.as_deref()),
        PackageRisk::Elevated,
        changes,
    );
    compare_optional_native_field(
        "build_scripts",
        old.and_then(|native| native.build_scripts.as_deref()),
        new.and_then(|native| native.build_scripts.as_deref()),
        PackageRisk::High,
        changes,
    );
    compare_optional_native_field(
        "proc_macros",
        old.and_then(|native| native.proc_macros.as_deref()),
        new.and_then(|native| native.proc_macros.as_deref()),
        PackageRisk::High,
        changes,
    );
    compare_optional_native_field(
        "unsafe",
        old.and_then(|native| native.unsafe_policy.as_deref()),
        new.and_then(|native| native.unsafe_policy.as_deref()),
        PackageRisk::High,
        changes,
    );
    let old_links = old.map(|native| native.links.join(", "));
    let new_links = new.map(|native| native.links.join(", "));
    if old_links != new_links {
        changes.push(manifest_change(
            "native-rust",
            "links",
            old_links,
            new_links,
            PackageRisk::High,
        ));
    }
}

fn compare_optional_native_field(
    name: &str,
    old: Option<&str>,
    new: Option<&str>,
    risk: PackageRisk,
    changes: &mut Vec<PackageManifestChange>,
) {
    if old != new {
        changes.push(manifest_change(
            "native-rust",
            name,
            old.map(str::to_string),
            new.map(str::to_string),
            risk,
        ));
    }
}

fn compare_interface_sources(
    old_sources: &[PackageSource],
    new_sources: &[PackageSource],
) -> Vec<PackageInterfaceChange> {
    let old_interfaces = interface_sources_by_relative_path(old_sources);
    let new_interfaces = interface_sources_by_relative_path(new_sources);
    let files = old_interfaces
        .keys()
        .chain(new_interfaces.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for file in files {
        match (old_interfaces.get(&file), new_interfaces.get(&file)) {
            (Some(old), Some(new)) if old.contents != new.contents => {
                let findings = review_sources(&old.path, &old.contents, &new.path, &new.contents);
                let risk = modified_interface_change_risk(old, new, new_sources, &findings);
                changes.push(PackageInterfaceChange {
                    file,
                    change: PackageInterfaceChangeKind::Modified,
                    risk,
                    findings,
                });
            }
            (None, Some(_)) => {
                let risk = added_interface_change_risk(new_sources, &file);
                changes.push(PackageInterfaceChange {
                    file,
                    change: PackageInterfaceChangeKind::Added,
                    risk,
                    findings: Vec::new(),
                });
            }
            (Some(_), None) => changes.push(PackageInterfaceChange {
                file,
                change: PackageInterfaceChangeKind::Removed,
                risk: PackageRisk::High,
                findings: Vec::new(),
            }),
            _ => {}
        }
    }
    changes
}

fn interface_sources_by_relative_path(
    sources: &[PackageSource],
) -> BTreeMap<String, &PackageSource> {
    sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.relative_path.clone(), source))
        .collect()
}

fn package_diff_reasons(
    manifest_changes: &[PackageManifestChange],
    interface_changes: &[PackageInterfaceChange],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if manifest_changes
        .iter()
        .any(|change| change.kind == "dependency")
    {
        reasons.push("RSScript dependencies changed".to_string());
    }
    if manifest_changes
        .iter()
        .any(|change| change.kind == "package-feature")
    {
        reasons.push("package features changed".to_string());
    }
    if manifest_changes
        .iter()
        .any(|change| change.kind == "native-rust")
    {
        reasons.push("native Rust wrapper metadata changed".to_string());
    }
    if !interface_changes.is_empty() {
        reasons.push("public .rssi semantic contract changed".to_string());
    }
    if interface_changes
        .iter()
        .any(|change| change.risk == PackageRisk::High)
    {
        reasons.push("high-risk interface change detected".to_string());
    }
    reasons
}

fn package_diff_risk(
    manifest_changes: &[PackageManifestChange],
    interface_changes: &[PackageInterfaceChange],
    old_risk: PackageRisk,
    new_risk: PackageRisk,
) -> PackageRisk {
    let mut risk = old_risk.max(new_risk);
    for change in manifest_changes {
        risk = risk.max(change.risk);
    }
    for change in interface_changes {
        risk = risk.max(change.risk);
    }
    risk
}

fn interface_change_risk(findings: &[ReviewFinding]) -> PackageRisk {
    if findings
        .iter()
        .any(|finding| finding.code == code::REVIEW_PROTOCOL_IMPL_CHANGED)
    {
        return PackageRisk::High;
    }
    if findings.iter().any(|finding| {
        matches!(
            finding.risk,
            ReviewRisk::Unsafe | ReviewRisk::Effect | ReviewRisk::Boundary | ReviewRisk::Guarantee
        )
    }) {
        PackageRisk::High
    } else if findings.is_empty() {
        PackageRisk::Low
    } else {
        PackageRisk::Elevated
    }
}

fn modified_interface_change_risk(
    old: &PackageSource,
    new: &PackageSource,
    new_sources: &[PackageSource],
    findings: &[ReviewFinding],
) -> PackageRisk {
    interface_change_risk(findings).max(modified_contracts_in_interface_risk(old, new, new_sources))
}

fn added_interface_change_risk(new_sources: &[PackageSource], file: &str) -> PackageRisk {
    let Some(source_path) = new_sources
        .iter()
        .find(|source| {
            source.kind == PackageReviewFileKind::Interface && source.relative_path == file
        })
        .map(|source| source.path.as_str())
    else {
        return PackageRisk::Low;
    };
    let type_contracts =
        collect_package_type_contracts(new_sources, PackageReviewFileKind::Interface);
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut saw_contract = false;

    for contract in type_contracts.values() {
        if contract.span.file != source_path {
            continue;
        }
        saw_contract = true;
        if contract.kind == TypeKind::Resource
            || contract
                .fields
                .iter()
                .any(|field| field.is_handle || field.is_weak)
        {
            return PackageRisk::High;
        }
    }

    let function_contracts =
        collect_package_function_contracts(new_sources, PackageReviewFileKind::Interface);
    for contract in function_contracts.values() {
        if contract.span.file != source_path {
            continue;
        }
        saw_contract = true;
        if package_added_function_contract_is_high_risk(contract, &resource_types) {
            return PackageRisk::High;
        }
    }

    if saw_contract {
        PackageRisk::Elevated
    } else {
        PackageRisk::Low
    }
}

fn modified_contracts_in_interface_risk(
    old: &PackageSource,
    new: &PackageSource,
    new_sources: &[PackageSource],
) -> PackageRisk {
    let old_type_contracts = package_type_contracts_for_source(old);
    let new_type_contracts = package_type_contracts_for_source(new);
    let old_function_contracts = package_function_contracts_for_source(old);
    let new_function_contracts = package_function_contracts_for_source(new);
    let mut resource_types =
        collect_package_type_contracts(new_sources, PackageReviewFileKind::Interface)
            .values()
            .filter(|contract| contract.kind == TypeKind::Resource)
            .map(|contract| contract.name.clone())
            .collect::<BTreeSet<_>>();
    resource_types.extend(
        old_type_contracts
            .values()
            .filter(|contract| contract.kind == TypeKind::Resource)
            .map(|contract| contract.name.clone()),
    );
    let resource_type_refs = resource_types
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut risk = PackageRisk::Low;

    for name in old_type_contracts.keys() {
        if !new_type_contracts.contains_key(name) {
            return PackageRisk::High;
        }
    }

    for name in old_function_contracts.keys() {
        if !new_function_contracts.contains_key(name) {
            return PackageRisk::High;
        }
    }

    for (name, contract) in &new_type_contracts {
        match old_type_contracts.get(name) {
            Some(old_contract) => {
                if package_type_contracts_match(old_contract, contract) {
                    continue;
                }
                if package_type_contract_boundary_changed(old_contract, contract) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::High);
            }
            None => {
                if package_added_type_contract_is_high_risk(contract) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::Elevated);
            }
        }
    }

    for (name, contract) in &new_function_contracts {
        match old_function_contracts.get(name) {
            Some(old_contract) => {
                if package_function_contracts_match(old_contract, contract) {
                    continue;
                }
                if package_function_contract_boundary_changed(
                    old_contract,
                    contract,
                    &resource_type_refs,
                ) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::Elevated);
            }
            None => {
                if package_added_function_contract_is_high_risk(contract, &resource_type_refs) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::Elevated);
            }
        }
    }

    risk
}

fn manifest_change(
    kind: impl Into<String>,
    name: impl Into<String>,
    before: Option<String>,
    after: Option<String>,
    risk: PackageRisk,
) -> PackageManifestChange {
    PackageManifestChange {
        kind: kind.into(),
        name: name.into(),
        before,
        after,
        risk,
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

fn package_risk(
    manifest: &Manifest,
    native: Option<&PackageNativeRustReview>,
    review_map: &ReviewMap,
    diagnostics: &[Diagnostic],
    native_apis: usize,
) -> PackageRisk {
    if manifest
        .review
        .as_ref()
        .and_then(|review| review.expect.risk.as_deref())
        == Some("unknown")
        || review_map.summary.unknown.functions > 0
    {
        return PackageRisk::Unknown;
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return PackageRisk::High;
    }
    if manifest
        .features
        .iter()
        .any(|(name, values)| package_feature_may_change_boundary_risk(name, values))
    {
        return PackageRisk::High;
    }
    if let Some(native) = native
        && (native
            .build_scripts
            .as_deref()
            .is_some_and(|policy| policy != "forbid")
            || native
                .proc_macros
                .as_deref()
                .is_some_and(|policy| policy != "forbid")
            || native
                .unsafe_policy
                .as_deref()
                .is_some_and(|policy| policy != "forbid")
            || !native.links.is_empty())
    {
        return PackageRisk::High;
    }
    if native_apis > 0 {
        return package_native_api_risk(manifest);
    }
    if native.is_some() || review_map.summary.review_required.functions > 0 {
        return PackageRisk::Elevated;
    }
    PackageRisk::Low
}

fn package_native_api_risk(manifest: &Manifest) -> PackageRisk {
    match manifest
        .review
        .as_ref()
        .and_then(|review| review.policy.native_api_risk.as_deref())
    {
        Some("high") => PackageRisk::High,
        Some("elevated") => PackageRisk::Elevated,
        _ => PackageRisk::High,
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
