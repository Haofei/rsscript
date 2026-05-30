use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::analyzer::{
    analyze_source_with_interfaces, analyze_sources_with_interfaces, core_interfaces,
};
use crate::diagnostic::{Diagnostic, code};
use crate::lint::lint_source;
use crate::review::{ReviewMap, ReviewMapClassification, review_map_sources};
use crate::syntax::ast::TypeKind;

mod contract;
mod diff;
mod format;
mod graph;
mod lock;
mod metadata;
mod native;
mod policy;
mod publish;
mod source_set;
mod types;
mod vendor;

use contract::{
    PackageFunctionContract, collect_package_function_contracts, collect_package_type_contracts,
    package_contract_has_resource_boundary, package_interface_contract_diagnostics,
    package_interface_diagnostic_exports, package_interface_environment_diagnostics,
    package_review_exports,
};
pub use diff::diff_package_dirs;
pub use format::*;
use graph::check_package_graph;
pub use graph::package_tree;
use lock::{
    compare_locked_packages, package_checksum, package_lock_diff_reasons, package_native_hash,
    read_package_lock,
};
pub use lock::{diff_package_locks, lock_package_dir};
pub use metadata::{package_lowering_input, package_metadata};
use native::{
    check_package_native_rust, manifest_native_enabled, manifest_native_unsafe_boundary,
    native_binding_interface_sources, native_effective_build_policy,
    package_native_binding_diagnostics, package_native_bindings,
};
use policy::{
    collect_manifest_review_policy_diagnostics, collect_manifest_review_policy_violations,
    package_review_policy_has_high_risk_violation, package_review_policy_ok,
};
pub use publish::{publish_package_dry_run, publish_package_dry_run_with_registry};
use source_set::{
    LoadedPackage, Manifest, ManifestNativeRust, PackageSource, load_package,
    load_package_manifest, load_package_with_features, resolve_package_features,
};
pub use types::*;
pub use vendor::vendor_package_dir;

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
