use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use rsscript_project::{
    ProjectManifestGraph, ProjectManifestGraphLimits, capture_project_manifest_graph,
    resolve_project_path_dependency,
};

use super::NativeRustDependency;

use super::dependency::package_dependency_spec;
use super::source_set::{
    parse_package_manifest_source, resolve_package_features, selected_root_package_features,
};
use super::{
    Manifest, ManifestNativeRust, PackageNativeRustAuthorDeclaration, PackageNativeRustCheck,
    PackageNativeRustReview, PackageNativeRustSemanticReview, PackageNativeRustSourceScan,
    PackageNativeRustUnsafePolicies, PackageRisk, PackageSource, canonical_path_label,
    read_utf8_file_bounded,
};

mod bindings;

pub(super) use bindings::{
    native_binding_interface_sources, package_external_bindings, package_native_binding_diagnostics,
};

const NATIVE_MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

pub(super) fn package_native_rust_dependencies(
    package_dir: &Path,
) -> Result<Vec<NativeRustDependency>, String> {
    let manifest_graph =
        capture_project_manifest_graph(package_dir, ProjectManifestGraphLimits::default())?;
    let manifest = captured_manifest(&manifest_graph, package_dir)?;
    let mut visited = BTreeSet::new();
    let mut dependencies = Vec::new();
    let selected_features = selected_root_package_features(&manifest);
    collect_package_native_rust_dependencies(
        package_dir,
        &manifest,
        &selected_features,
        &manifest_graph,
        &mut visited,
        &mut dependencies,
    )?;
    dedup_native_rust_dependencies(dependencies)
}

pub(crate) fn package_native_plugin_build_dependencies(
    package_dir: &Path,
) -> Result<Vec<super::NativePluginBuildDependency>, String> {
    let manifest_graph =
        capture_project_manifest_graph(package_dir, ProjectManifestGraphLimits::default())?;
    let manifest = captured_manifest(&manifest_graph, package_dir)?;
    let mut visited = BTreeSet::new();
    let mut dependencies = Vec::new();
    let selected_features = selected_root_package_features(&manifest);
    collect_package_native_plugin_build_dependencies(
        package_dir,
        &manifest,
        &selected_features,
        &manifest_graph,
        &mut visited,
        &mut dependencies,
    )?;
    dedup_native_plugin_build_dependencies(dependencies)
}

fn collect_package_native_plugin_build_dependencies(
    package_dir: &Path,
    manifest: &Manifest,
    selected_features: &[String],
    manifest_graph: &ProjectManifestGraph,
    visited: &mut BTreeSet<String>,
    dependencies: &mut Vec<super::NativePluginBuildDependency>,
) -> Result<(), String> {
    let canonical = canonical_path_label(package_dir);
    if !visited.insert(canonical) {
        return Ok(());
    }
    if let Some(native) = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .filter(|native| native.enabled)
    {
        let crate_name = native
            .crate_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                "native.rust enabled packages must declare `crate` before native loading."
                    .to_string()
            })?;
        let native_path = native.path.as_deref().unwrap_or("native/rust");
        let native_root = confined_native_rust_path(package_dir, native_path)?;
        dependencies.push(super::NativePluginBuildDependency {
            crate_name: crate_name.to_string(),
            path: native_root.display().to_string(),
            cargo_features: selected_native_cargo_features_for_package_features(
                native,
                selected_features,
            ),
            default_features: native.default_features,
            bindings: package_external_bindings(package_dir)?,
        });
    }
    for (name, value) in &manifest.dependencies {
        let spec = package_dependency_spec(name, value);
        let Some(path) = &spec.path else {
            continue;
        };
        let Some(dependency_dir) = resolve_project_path_dependency(package_dir, path)? else {
            continue;
        };
        let dependency_manifest = captured_manifest(manifest_graph, &dependency_dir)?;
        let selected_features = resolve_package_features(&dependency_manifest, &spec.features);
        collect_package_native_plugin_build_dependencies(
            &dependency_dir,
            &dependency_manifest,
            &selected_features.selected,
            manifest_graph,
            visited,
            dependencies,
        )?;
    }
    Ok(())
}

fn dedup_native_plugin_build_dependencies(
    dependencies: Vec<super::NativePluginBuildDependency>,
) -> Result<Vec<super::NativePluginBuildDependency>, String> {
    let mut by_crate: BTreeMap<String, usize> = BTreeMap::new();
    let mut deduped: Vec<super::NativePluginBuildDependency> = Vec::new();
    for mut dependency in dependencies {
        if let Some(existing_index) = by_crate.get(&dependency.crate_name).copied() {
            let existing = &mut deduped[existing_index];
            if existing.path != dependency.path {
                return Err(format!(
                    "native Rust dependency crate `{}` is provided by both `{}` and `{}`.",
                    dependency.crate_name, existing.path, dependency.path
                ));
            }
            if existing.default_features != dependency.default_features {
                return Err(format!(
                    "native Rust dependency crate `{}` has conflicting `default-features` settings.",
                    dependency.crate_name
                ));
            }
            existing
                .cargo_features
                .append(&mut dependency.cargo_features);
            existing.cargo_features.sort();
            existing.cargo_features.dedup();
            existing.bindings.append(&mut dependency.bindings);
            continue;
        }
        dependency.cargo_features.sort();
        dependency.cargo_features.dedup();
        by_crate.insert(dependency.crate_name.clone(), deduped.len());
        deduped.push(dependency);
    }
    Ok(deduped)
}

fn collect_package_native_rust_dependencies(
    package_dir: &Path,
    manifest: &Manifest,
    selected_features: &[String],
    manifest_graph: &ProjectManifestGraph,
    visited: &mut BTreeSet<String>,
    dependencies: &mut Vec<NativeRustDependency>,
) -> Result<(), String> {
    let canonical = canonical_path_label(package_dir);
    if !visited.insert(canonical) {
        return Ok(());
    }
    dependencies.extend(package_own_native_rust_dependencies(
        package_dir,
        manifest,
        selected_features,
    )?);
    for (name, value) in &manifest.dependencies {
        let spec = package_dependency_spec(name, value);
        let Some(path) = &spec.path else {
            continue;
        };
        let Some(dependency_dir) = resolve_project_path_dependency(package_dir, path)? else {
            continue;
        };
        let dependency_manifest = captured_manifest(manifest_graph, &dependency_dir)?;
        let selected_features = resolve_package_features(&dependency_manifest, &spec.features);
        collect_package_native_rust_dependencies(
            &dependency_dir,
            &dependency_manifest,
            &selected_features.selected,
            manifest_graph,
            visited,
            dependencies,
        )?;
    }
    Ok(())
}

/// Decode package semantics from bytes admitted by the project capture graph.
/// This helper intentionally has no filesystem fallback: a dependency absent
/// from the graph was not captured and cannot be consulted by compiler-native
/// compatibility resolution.
fn captured_manifest(
    manifest_graph: &ProjectManifestGraph,
    package_dir: &Path,
) -> Result<Manifest, String> {
    let source = manifest_graph.manifest_source(package_dir).ok_or_else(|| {
        format!(
            "project manifest graph omitted native dependency root {}",
            package_dir.display()
        )
    })?;
    parse_package_manifest_source(package_dir, source)
}

fn package_own_native_rust_dependencies(
    package_dir: &Path,
    manifest: &Manifest,
    selected_features: &[String],
) -> Result<Vec<NativeRustDependency>, String> {
    let Some(native) = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
    else {
        return Ok(Vec::new());
    };
    if !native.enabled {
        return Ok(Vec::new());
    }
    let crate_name = native
        .crate_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            "native.rust enabled packages must declare `crate` before Rust lowering.".to_string()
        })?;
    let native_path = native.path.as_deref().unwrap_or("native/rust");
    let native_root = confined_native_rust_path(package_dir, native_path)?;
    let bindings = package_external_bindings(package_dir)?;
    let cargo_features =
        selected_native_cargo_features_for_package_features(native, selected_features);
    Ok(vec![NativeRustDependency {
        crate_name: crate_name.to_string(),
        path: native_root.display().to_string(),
        cargo_features,
        default_features: native.default_features,
        bindings,
    }])
}

fn dedup_native_rust_dependencies(
    dependencies: Vec<NativeRustDependency>,
) -> Result<Vec<NativeRustDependency>, String> {
    let mut by_crate: BTreeMap<String, usize> = BTreeMap::new();
    let mut deduped: Vec<NativeRustDependency> = Vec::new();
    for mut dependency in dependencies {
        if let Some(existing_index) = by_crate.get(&dependency.crate_name).copied() {
            if deduped[existing_index].path != dependency.path {
                return Err(format!(
                    "native Rust dependency crate `{}` is provided by both `{}` and `{}`.",
                    dependency.crate_name, deduped[existing_index].path, dependency.path
                ));
            }
            if deduped[existing_index].default_features != dependency.default_features {
                return Err(format!(
                    "native Rust dependency crate `{}` has conflicting `default-features` settings.",
                    dependency.crate_name
                ));
            }
            deduped[existing_index]
                .cargo_features
                .append(&mut dependency.cargo_features);
            deduped[existing_index].cargo_features.sort();
            deduped[existing_index].cargo_features.dedup();
            continue;
        }
        dependency.cargo_features.sort();
        dependency.cargo_features.dedup();
        by_crate.insert(dependency.crate_name.clone(), deduped.len());
        deduped.push(dependency);
    }
    Ok(deduped)
}

pub(super) fn check_package_native_rust(
    package_dir: &Path,
    native: Option<&PackageNativeRustReview>,
) -> Result<Option<PackageNativeRustCheck>, String> {
    let Some(native) = native else {
        return Ok(None);
    };
    let native_root = confined_native_rust_path(package_dir, &native.path)?;
    let cargo_toml = native_root.join("Cargo.toml");
    let cargo_toml_present = cargo_toml.exists();
    let mut files = Vec::new();
    let mut reasons = Vec::new();
    let mut scan_complete = native.semantic.source_scan_best_effort.complete;
    if native_root.exists() {
        if let Err(error) = super::collect_regular_files(&native_root, &mut files) {
            scan_complete = false;
            reasons.push(format!(
                "native Rust source enumeration incomplete: {error}"
            ));
        }
    }
    let unsafe_detected = native.semantic.source_scan_best_effort.unsafe_detected;
    let build_risk = match native_build_script_risks(&files) {
        Ok(risk) => risk,
        Err(error) => {
            scan_complete = false;
            reasons.push(format!("native Rust build-script scan incomplete: {error}"));
            NativeBuildScriptRisk::default()
        }
    };
    if !native_root.exists() {
        reasons.push("native Rust path missing".to_string());
    }
    if !cargo_toml_present {
        reasons.push("native Rust Cargo.toml missing".to_string());
    }
    if native
        .crate_name
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        reasons.push("native Rust crate name missing".to_string());
    }
    if files.is_empty() {
        reasons.push("native Rust source files missing".to_string());
    }
    if unsafe_detected && native.unsafe_policies.wrapper_unsafe_blocks.as_deref() == Some("forbid")
    {
        reasons.push("native Rust unsafe usage detected".to_string());
    }
    if !scan_complete {
        reasons.push("native Rust semantic source scan incomplete".to_string());
    }
    if native.build_scripts.as_deref() == Some("forbid") {
        if build_risk.env_detected {
            reasons.push("native Rust build script reads environment".to_string());
        }
        if build_risk.download_detected {
            reasons.push("native Rust build script may download code".to_string());
        }
    }
    let metadata = if cargo_toml_present {
        scan_native_cargo_metadata(&cargo_toml, native, &mut reasons)?
    } else {
        NativeCargoMetadataScan::default()
    };
    let ok = reasons.is_empty();
    let risk = if !scan_complete {
        PackageRisk::Unknown
    } else if ok {
        PackageRisk::Elevated
    } else {
        PackageRisk::High
    };

    Ok(Some(PackageNativeRustCheck {
        path: native.path.clone(),
        cargo_toml_present,
        cargo_metadata_ok: metadata.ok,
        cargo_package_name: metadata.package_name,
        target_kinds: metadata.target_kinds,
        unsafe_detected,
        linked_libraries: native.links.clone(),
        build_env_detected: build_risk.env_detected,
        build_download_detected: build_risk.download_detected,
        file_count: files.len(),
        ok,
        risk,
        reasons,
    }))
}

pub(super) fn package_native_rust_review(
    package_dir: &Path,
    manifest: &Manifest,
    sources: &[PackageSource],
    native: &ManifestNativeRust,
) -> Result<PackageNativeRustReview, String> {
    let path = native
        .path
        .clone()
        .unwrap_or_else(|| "native/rust".to_string());
    let native_root = confined_native_rust_path(package_dir, &path)?;
    let cargo_toml = native_root.join("Cargo.toml");
    let cargo_source = if cargo_toml.exists() {
        read_utf8_file_bounded(
            &cargo_toml,
            NATIVE_MANIFEST_MAX_BYTES,
            "native Cargo.toml review read",
        )?
    } else {
        String::new()
    };
    let cargo_features = selected_native_cargo_features(manifest, native);
    let scan = scan_native_rust_semantics(&native_root, &cargo_source);
    let unsafe_policies = native.effective_unsafe_policies();
    let author_parallel = package_declares_parallel_native_api(sources);
    let backend = scan.native_parallel_backends.first().cloned();
    let mut risk_reasons = Vec::new();
    if author_parallel {
        risk_reasons.push("native API declares parallel worker execution".to_string());
    }
    if let Some(backend) = &backend {
        risk_reasons.push(format!("native parallel backend `{backend}` detected"));
    }
    if !cargo_features.is_empty() {
        risk_reasons.push("native Cargo features selected".to_string());
    }

    Ok(PackageNativeRustReview {
        path,
        crate_name: native.crate_name.clone(),
        build_scripts: native_effective_build_policy(manifest, native.effective_build_scripts()),
        proc_macros: native_effective_build_policy(manifest, native.effective_proc_macros()),
        unsafe_policies: PackageNativeRustUnsafePolicies {
            rss_unsafe_apis: unsafe_policies.rss_unsafe_apis.map(str::to_string),
            wrapper_unsafe_blocks: unsafe_policies.wrapper_unsafe_blocks.map(str::to_string),
            transitive_unsafe_blocks: unsafe_policies.transitive_unsafe_blocks.map(str::to_string),
        },
        native_links_policy: native.effective_native_links().map(str::to_string),
        ffi_policy: native.effective_ffi().map(str::to_string),
        links: native.links.clone(),
        cargo_features,
        semantic: PackageNativeRustSemanticReview {
            author_declaration: PackageNativeRustAuthorDeclaration {
                worker_thread_parallelism: author_parallel,
                native_parallel_backend: backend,
                risk_reasons,
            },
            source_scan_best_effort: scan,
        },
    })
}

fn native_path_escapes_package(path: &str) -> bool {
    Path::new(path).components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    })
}

pub(super) fn confined_native_rust_path(
    package_dir: &Path,
    configured_path: &str,
) -> Result<PathBuf, String> {
    if configured_path.trim().is_empty() {
        return Err("native Rust wrapper path must not be empty".to_string());
    }
    if native_path_escapes_package(configured_path) {
        return Err(format!(
            "native Rust wrapper path `{configured_path}` escapes the package root"
        ));
    }

    let package_root = fs::canonicalize(package_dir).map_err(|error| {
        format!(
            "failed to canonicalize package root {}: {error}",
            package_dir.display()
        )
    })?;
    let candidate = package_root.join(configured_path);
    let mut existing = candidate.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            format!("native Rust wrapper path `{configured_path}` has no existing package ancestor")
        })?;
    }
    let canonical_existing = fs::canonicalize(existing).map_err(|error| {
        format!(
            "failed to canonicalize native Rust path ancestor {}: {error}",
            existing.display()
        )
    })?;
    if !canonical_existing.starts_with(&package_root) {
        return Err(format!(
            "native Rust wrapper path `{configured_path}` resolves outside the package root"
        ));
    }

    let absent_suffix = candidate
        .strip_prefix(existing)
        .map_err(|_| format!("failed to normalize native Rust wrapper path `{configured_path}`"))?;
    Ok(canonical_existing.join(absent_suffix))
}

fn package_declares_parallel_native_api(_sources: &[PackageSource]) -> bool {
    false
}

fn selected_native_cargo_features(manifest: &Manifest, native: &ManifestNativeRust) -> Vec<String> {
    let selected_features = selected_root_package_features(manifest);
    selected_native_cargo_features_for_package_features(native, &selected_features)
}

fn selected_native_cargo_features_for_package_features(
    native: &ManifestNativeRust,
    selected_features: &[String],
) -> Vec<String> {
    let mut features = native.cargo_features.clone();
    for package_feature in selected_features {
        if let Some(mapping) = native.feature_map.get(package_feature.as_str()) {
            features.extend(mapping.cargo_features.iter().cloned());
        }
    }
    features.sort();
    features.dedup();
    features
}

fn scan_native_rust_semantics(
    native_root: &Path,
    cargo_source: &str,
) -> PackageNativeRustSourceScan {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    if native_root.exists() {
        if let Err(error) = super::collect_regular_files(native_root, &mut files) {
            errors.push(format!("failed to enumerate native Rust sources: {error}"));
        }
    }
    let mut scan = NativeSemanticScanAccumulator {
        native_parallel_backends: native_parallel_backends_from_cargo(cargo_source),
        build_script_present: files
            .iter()
            .any(|file| file.file_name().and_then(|name| name.to_str()) == Some("build.rs")),
        ..NativeSemanticScanAccumulator::default()
    };
    for file in files {
        if file.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        match fs::read_to_string(&file) {
            Ok(source) => scan_source_semantics(&source, &mut scan),
            Err(error) => errors.push(format!(
                "failed to read native Rust source {}: {error}",
                file.display()
            )),
        }
    }
    scan.native_parallel_backends.sort();
    scan.native_parallel_backends.dedup();
    let worker_thread_parallelism_detected =
        !scan.native_parallel_backends.is_empty() || scan.thread_detected;
    PackageNativeRustSourceScan {
        tool: "rss-native-source-scan".to_string(),
        selected_graph: "package-native-rust".to_string(),
        worker_thread_parallelism_detected,
        native_parallel_backends: scan.native_parallel_backends,
        unsafe_detected: scan.unsafe_detected,
        ffi_detected: scan.ffi_detected,
        filesystem_detected: scan.filesystem_detected,
        network_detected: scan.network_detected,
        build_script_present: scan.build_script_present,
        complete: errors.is_empty(),
        errors,
    }
}

#[derive(Default)]
struct NativeSemanticScanAccumulator {
    native_parallel_backends: Vec<String>,
    thread_detected: bool,
    unsafe_detected: bool,
    ffi_detected: bool,
    filesystem_detected: bool,
    network_detected: bool,
    build_script_present: bool,
}

fn native_parallel_backends_from_cargo(cargo_source: &str) -> Vec<String> {
    let Ok(value) = cargo_source.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut backends = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(dependencies) = value.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for dependency in dependencies.keys() {
            if dependency.contains("parallel") || dependency.contains("worker") {
                backends.push(dependency.clone());
            }
        }
    }
    backends
}

fn scan_source_semantics(source: &str, scan: &mut NativeSemanticScanAccumulator) {
    let stripped = source_without_rust_comments(source);
    if stripped.contains("std::thread")
        || stripped.contains("thread::spawn")
        || stripped.contains(".spawn(")
    {
        scan.thread_detected = true;
    }
    if source_contains_rust_unsafe_keyword(&stripped) {
        scan.unsafe_detected = true;
    }
    if stripped.contains("extern \"C\"") || stripped.contains("extern \"system\"") {
        scan.ffi_detected = true;
    }
    if stripped.contains("std::fs") || stripped.contains("fs::") || stripped.contains("File::") {
        scan.filesystem_detected = true;
    }
    let lower = stripped.to_ascii_lowercase();
    if [
        "reqwest",
        "ureq",
        "curl",
        "tcpstream",
        "udpsocket",
        "http://",
        "https://",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
    {
        scan.network_detected = true;
    }
}

#[derive(Debug, Default)]
struct NativeCargoMetadataScan {
    ok: bool,
    package_name: Option<String>,
    target_kinds: Vec<String>,
}

fn scan_native_cargo_metadata(
    cargo_toml: &Path,
    native: &PackageNativeRustReview,
    reasons: &mut Vec<String>,
) -> Result<NativeCargoMetadataScan, String> {
    let Some(_expected_name) = native
        .crate_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return Ok(NativeCargoMetadataScan::default());
    };
    let source = read_utf8_file_bounded(
        cargo_toml,
        NATIVE_MANIFEST_MAX_BYTES,
        "native Cargo.toml metadata read",
    )?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", cargo_toml.display()))?;
    let package_name = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let Some(package_name) = package_name else {
        reasons.push("native Rust Cargo.toml has no package name".to_string());
        return Ok(NativeCargoMetadataScan::default());
    };

    if let Some(expected) = native.crate_name.as_deref().map(str::trim)
        && !expected.is_empty()
        && expected != package_name
    {
        reasons.push(format!(
            "native Rust crate name `{expected}` does not match Cargo package `{}`",
            package_name
        ));
    }

    let native_root = cargo_toml.parent().unwrap_or_else(|| Path::new("."));
    let mut target_kinds = BTreeSet::new();
    if native_root.join("src/lib.rs").is_file() || manifest.get("lib").is_some() {
        target_kinds.insert("lib".to_string());
    }
    if native_root.join("build.rs").is_file()
        || manifest
            .get("package")
            .and_then(|package| package.get("build"))
            .is_some()
    {
        target_kinds.insert("custom-build".to_string());
    }
    if manifest
        .get("lib")
        .and_then(|library| library.get("proc-macro"))
        .and_then(toml::Value::as_bool)
        == Some(true)
    {
        target_kinds.insert("proc-macro".to_string());
    }
    if manifest.get("bin").is_some() || native_root.join("src/main.rs").is_file() {
        target_kinds.insert("bin".to_string());
    }
    let target_kinds = target_kinds.into_iter().collect::<Vec<_>>();

    if target_kinds.iter().any(|kind| kind == "custom-build")
        && native.build_scripts.as_deref() == Some("forbid")
    {
        reasons.push("native Rust build script target present".to_string());
    }
    if target_kinds.iter().any(|kind| kind == "proc-macro")
        && native.proc_macros.as_deref() == Some("forbid")
    {
        reasons.push("native Rust proc macro target present".to_string());
    }

    Ok(NativeCargoMetadataScan {
        ok: true,
        package_name: Some(package_name),
        target_kinds,
    })
}

pub(super) fn reviewed_native_cargo_lock(
    native_root: &Path,
    crate_name: &str,
) -> Result<Option<PathBuf>, String> {
    let own_lock = native_root.join("Cargo.lock");
    if own_lock.is_file() {
        return Ok(Some(own_lock));
    }

    let expected_name = format!("name = {crate_name:?}");
    for ancestor in native_root.ancestors().skip(1) {
        let workspace_manifest = ancestor.join("Cargo.toml");
        let workspace_lock = ancestor.join("Cargo.lock");
        if !workspace_manifest.is_file() || !workspace_lock.is_file() {
            continue;
        }
        let manifest = read_utf8_file_bounded(
            &workspace_manifest,
            NATIVE_MANIFEST_MAX_BYTES,
            "native workspace manifest review",
        )?;
        if !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("[workspace]"))
        {
            continue;
        }
        let lock = read_utf8_file_bounded(
            &workspace_lock,
            64 * 1024 * 1024,
            "native workspace Cargo.lock review",
        )?;
        if lock.lines().any(|line| line.trim() == expected_name) {
            return Ok(Some(workspace_lock));
        }
    }

    let manifest = read_utf8_file_bounded(
        &native_root.join("Cargo.toml"),
        NATIVE_MANIFEST_MAX_BYTES,
        "native Cargo.toml lock review",
    )?;
    let parsed: toml::Value = toml::from_str(&manifest).map_err(|error| {
        format!(
            "native build denied: invalid Cargo.toml {}: {error}",
            native_root.join("Cargo.toml").display()
        )
    })?;
    let has_dependencies = ["dependencies", "build-dependencies"].iter().any(|key| {
        parsed
            .get(*key)
            .and_then(toml::Value::as_table)
            .is_some_and(|dependencies| !dependencies.is_empty())
    });
    if !has_dependencies {
        return Ok(None);
    }

    Err(format!(
        "native build denied: reviewed Cargo.lock is required for `{crate_name}` at {} or in a containing workspace",
        native_root.display()
    ))
}

pub(super) fn prepare_native_cargo_lock(cargo_toml: &Path) -> Result<(), String> {
    let source = read_utf8_file_bounded(
        cargo_toml,
        NATIVE_MANIFEST_MAX_BYTES,
        "dependency-free native Cargo.toml lock generation",
    )?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", cargo_toml.display()))?;
    let package = manifest
        .get("package")
        .ok_or_else(|| format!("{} has no [package] table", cargo_toml.display()))?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("{} has no package name", cargo_toml.display()))?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("{} has no package version", cargo_toml.display()))?;
    let lock = format!(
        "# This file is automatically @generated by RSScript.\nversion = 4\n\n[[package]]\nname = {name:?}\nversion = {version:?}\n"
    );
    let lock_path = cargo_toml
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", cargo_toml.display()))?
        .join("Cargo.lock");
    fs::write(&lock_path, lock)
        .map_err(|error| format!("failed to write {}: {error}", lock_path.display()))
}

#[cfg(test)]
fn isolate_cargo_manifest_from_parent_workspace(cargo_toml: &Path) -> Result<(), String> {
    let manifest = read_utf8_file_bounded(
        cargo_toml,
        NATIVE_MANIFEST_MAX_BYTES,
        "native Cargo.toml workspace isolation read",
    )?;
    if manifest
        .lines()
        .any(|line| line.trim_start().starts_with("[workspace]"))
    {
        return Ok(());
    }
    fs::write(cargo_toml, format!("{manifest}\n[workspace]\n"))
        .map_err(|error| format!("failed to write {}: {error}", cargo_toml.display()))
}

#[derive(Debug, Default)]
struct NativeBuildScriptRisk {
    env_detected: bool,
    download_detected: bool,
}

fn native_build_script_risks(files: &[PathBuf]) -> Result<NativeBuildScriptRisk, String> {
    let mut risk = NativeBuildScriptRisk::default();
    for file in files {
        if file.file_name().and_then(|name| name.to_str()) != Some("build.rs") {
            continue;
        }
        let source = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let stripped = source_without_rust_comments(&source);
        if build_script_reads_environment(&stripped) {
            risk.env_detected = true;
        }
        if build_script_may_download_code(&stripped) {
            risk.download_detected = true;
        }
    }
    Ok(risk)
}

fn build_script_reads_environment(source: &str) -> bool {
    [
        "env::var",
        "env::var_os",
        "std::env::var",
        "std::env::var_os",
        "env!(",
        "option_env!(",
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

fn build_script_may_download_code(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    [
        "http://",
        "https://",
        "reqwest",
        "ureq",
        "curl",
        "wget",
        "git clone",
        "git2",
        "tcpstream",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn source_without_rust_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            _ => {
                out.push(bytes[index] as char);
                index += 1;
            }
        }
    }
    out
}

fn source_contains_rust_unsafe_keyword(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            b'"' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == b'"' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'\'' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == b'\'' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            byte if is_rust_ident_start(byte) => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_rust_ident_continue(bytes[index]) {
                    index += 1;
                }
                if &source[start..index] == "unsafe" {
                    return true;
                }
            }
            _ => index += 1,
        }
    }
    false
}

fn is_rust_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_rust_ident_continue(byte: u8) -> bool {
    is_rust_ident_start(byte) || byte.is_ascii_digit()
}

pub(super) fn native_effective_build_policy(
    manifest: &Manifest,
    value: Option<&str>,
) -> Option<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            manifest
                .review
                .as_ref()
                .and_then(|review| review.policy.build_execution_default.as_deref())
                .filter(|value| matches!(*value, "forbid" | "review" | "allow"))
        })
        .map(str::to_string)
}

pub(super) fn manifest_native_enabled(manifest: &Manifest) -> bool {
    manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .is_some_and(|native| native.enabled)
}

pub(super) fn manifest_native_unsafe_boundary(manifest: &Manifest) -> bool {
    manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .is_some_and(|native| {
            native
                .effective_unsafe_policies()
                .has_non_forbidden_boundary()
        })
}

#[cfg(test)]
mod adapter_binding_tests {
    use super::*;

    #[test]
    fn plugin_build_dependencies_preserve_features_and_default_feature_policy() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-build-spec-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory");
        fs::create_dir_all(root.join("native/rust")).expect("fixture native directory");
        fs::write(
            root.join("rsspkg.toml"),
            r#"[package]
name = "native-build-spec"
version = "0.1.0"
edition = "2026"

[sources]
paths = ["src"]

[features]
fast = []
default = ["fast"]

[native.rust]
enabled = true
path = "native/rust"
crate = "native_build_spec"
cargo_features = ["base"]
default-features = false

[native.rust.feature_map.fast]
cargo_features = ["simd"]
"#,
        )
        .expect("fixture manifest");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("fixture source");

        let dependencies = package_native_plugin_build_dependencies(&root)
            .expect("native build specs should resolve");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].cargo_features, ["base", "simd"]);
        assert!(!dependencies[0].default_features);
    }

    #[test]
    fn native_path_rejects_root_and_parent_components() {
        assert!(native_path_escapes_package("/native/rust"));
        assert!(native_path_escapes_package("native/../outside"));
        assert!(!native_path_escapes_package("native/rust"));
    }

    #[cfg(windows)]
    #[test]
    fn native_path_rejects_windows_prefix_components() {
        assert!(native_path_escapes_package(r"C:\native\rust"));
        assert!(native_path_escapes_package(r"C:native\rust"));
        assert!(native_path_escapes_package(r"\\server\share\native"));
    }

    #[test]
    fn confined_native_path_canonicalizes_existing_target() {
        let package_dir = native_path_test_dir("existing");
        let native_dir = package_dir.join("native/rust");
        fs::create_dir_all(&native_dir).expect("native path should be created");

        let confined = confined_native_rust_path(&package_dir, "native/./rust")
            .expect("existing native path should be confined");
        let expected = native_dir
            .canonicalize()
            .expect("existing native path should canonicalize");
        let _ = fs::remove_dir_all(&package_dir);

        assert_eq!(confined, expected);
    }

    #[test]
    fn confined_native_path_represents_absent_path_under_canonical_root() {
        let package_dir = native_path_test_dir("absent");
        fs::create_dir_all(&package_dir).expect("package path should be created");
        let canonical_root = package_dir
            .canonicalize()
            .expect("package root should canonicalize");

        let confined = confined_native_rust_path(&package_dir, "native/rust")
            .expect("absent native path should be safely represented");
        let _ = fs::remove_dir_all(&package_dir);

        assert_eq!(confined, canonical_root.join("native/rust"));
    }

    #[test]
    fn native_binding_manifest_is_bounded_before_parsing() {
        let package_dir = native_path_test_dir("oversized-bindings");
        fs::create_dir_all(package_dir.join("native")).expect("native directory");
        fs::write(
            package_dir.join("native/bindings.rssbind.toml"),
            vec![b'x'; NATIVE_MANIFEST_MAX_BYTES as usize + 1],
        )
        .expect("oversized binding manifest");

        let error = package_external_bindings(&package_dir)
            .expect_err("oversized manifest must be rejected");
        let _ = fs::remove_dir_all(&package_dir);
        assert!(
            error.contains("native binding manifest read exceeded byte limit"),
            "{error}"
        );
    }

    #[test]
    fn cargo_manifest_is_bounded_before_workspace_isolation() {
        let root = native_path_test_dir("oversized-cargo");
        fs::create_dir_all(&root).expect("fixture directory");
        let cargo_toml = root.join("Cargo.toml");
        fs::write(
            &cargo_toml,
            vec![b'x'; NATIVE_MANIFEST_MAX_BYTES as usize + 1],
        )
        .expect("oversized Cargo.toml");

        let error = isolate_cargo_manifest_from_parent_workspace(&cargo_toml)
            .expect_err("oversized Cargo.toml must be rejected");
        let _ = fs::remove_dir_all(&root);
        assert!(
            error.contains("native Cargo.toml workspace isolation read exceeded byte limit"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn confined_native_path_canonicalizes_existing_ancestor_of_absent_path() {
        use std::os::unix::fs::symlink;

        let package_dir = native_path_test_dir("absent-symlink");
        let actual_dir = package_dir.join("actual");
        fs::create_dir_all(&actual_dir).expect("actual native parent should be created");
        symlink(&actual_dir, package_dir.join("native")).expect("native symlink should be created");

        let confined = confined_native_rust_path(&package_dir, "native/rust")
            .expect("absent native path beneath confined symlink should be represented");
        let expected = actual_dir
            .canonicalize()
            .expect("actual native parent should canonicalize")
            .join("rust");
        let _ = fs::remove_dir_all(&package_dir);

        assert_eq!(confined, expected);
    }

    fn native_path_test_dir(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "rsscript-native-path-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ))
    }
}
