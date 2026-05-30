use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::diagnostic::{Diagnostic, code};
use crate::formatter::format_program;
use crate::rust_lower::NativeRustDependency;
use crate::syntax::ast::{EffectDecl, FileFeature, Item, Program};
use crate::syntax::parse_source;

use super::{
    Manifest, ManifestNativeRust, PackageNativeRustCheck, PackageNativeRustReview,
    PackageReviewFileKind, PackageRisk, PackageSource,
};

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataTarget {
    kind: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NativeBindingsManifest {
    #[serde(default)]
    bindings: BTreeMap<String, String>,
}

pub(super) fn package_native_rust_dependencies(
    package_dir: &Path,
    manifest: &Manifest,
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
    let bindings = package_native_bindings(package_dir)?;
    Ok(vec![NativeRustDependency {
        crate_name: crate_name.to_string(),
        path: absolute_package_path(package_dir)
            .join(native_path)
            .display()
            .to_string(),
        bindings,
    }])
}

fn absolute_package_path(package_dir: &Path) -> PathBuf {
    if package_dir.is_absolute() {
        return package_dir.to_path_buf();
    }
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(package_dir)
}

pub(super) fn package_native_bindings(
    package_dir: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let path = package_dir.join("native/bindings.rssbind.toml");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let manifest: NativeBindingsManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    Ok(manifest.bindings)
}

pub(super) fn native_binding_interface_sources(
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
) -> Vec<PackageSource> {
    if native_bindings.is_empty() {
        return Vec::new();
    }
    let source_type_names = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .flat_map(|source| parse_source(&source.path, &source.contents).items)
        .filter_map(|item| match item {
            Item::Type(type_decl) => Some(type_decl.name),
            Item::Function(_) => None,
        })
        .collect::<BTreeSet<_>>();

    sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .filter_map(|source| {
            let mut selected_items = Vec::new();
            for item in parse_source(&source.path, &source.contents).items {
                match item {
                    Item::Type(type_decl) if !source_type_names.contains(&type_decl.name) => {
                        selected_items.push(Item::Type(type_decl));
                    }
                    Item::Function(function)
                        if function
                            .effects
                            .contains(&EffectDecl::Name("native".to_string()))
                            && native_bindings.contains_key(&function.name) =>
                    {
                        selected_items.push(Item::Function(function));
                    }
                    _ => {}
                }
            }
            if !selected_items
                .iter()
                .any(|item| matches!(item, Item::Function(_)))
            {
                return None;
            }
            let program = Program {
                features: vec![FileFeature::Native],
                unknown_features: Vec::new(),
                duplicate_features: Vec::new(),
                feature_spans: Vec::new(),
                profile_spans: Vec::new(),
                unknown_top_level_spans: Vec::new(),
                malformed_declaration_spans: Vec::new(),
                protocols: Vec::new(),
                protocol_impls: Vec::new(),
                items: selected_items,
            };
            Some(PackageSource {
                path: format!("{}#native-bindings", source.path),
                relative_path: format!("{}#native-bindings", source.relative_path),
                contents: format_program(&program),
                kind: PackageReviewFileKind::Interface,
            })
        })
        .collect()
}

pub(super) fn package_native_binding_diagnostics(
    package_dir: &Path,
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
    native: Option<&ManifestNativeRust>,
) -> Vec<Diagnostic> {
    if native_bindings.is_empty() {
        return Vec::new();
    }
    let interface_function_contracts =
        super::collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let crate_name = native
        .and_then(|native| native.crate_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let mut diagnostics = Vec::new();
    let native_enabled = native.is_some_and(|native| native.enabled);

    if !native_enabled {
        diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_NATIVE_BINDING,
                "native bindings require enabled `[native.rust]` configuration.",
                native_binding_manifest_span(package_dir),
                "native binding without native Rust wrapper",
            )
            .with_cause("A binding manifest maps RSScript native contracts to Rust wrapper functions, so the package must enable a native Rust wrapper crate.")
            .with_fix(
                "enable_native_rust",
                "Add `[native.rust] enabled = true` with a wrapper crate, or remove `native/bindings.rssbind.toml`.",
                "manual",
            ),
        );
    } else if crate_name.is_none() {
        diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_NATIVE_BINDING,
                "native bindings require `[native.rust].crate`.",
                native_binding_manifest_span(package_dir),
                "native binding crate missing",
            )
            .with_cause("Generated Rust must know which native crate owns the binding targets.")
            .with_fix(
                "declare_native_crate",
                "Set `[native.rust] crate = \"...\"` to the Rust wrapper crate name.",
                "manual",
            ),
        );
    }

    for (symbol, target) in native_bindings {
        let span = native_binding_span(package_dir, symbol);
        if symbol.trim().is_empty() || target.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    "native binding entries must have non-empty RSScript symbols and Rust targets.",
                    span,
                    "invalid native binding",
                )
                .with_cause("Binding keys name RSScript native functions; values name Rust wrapper functions.")
                .with_fix(
                    "fix_native_binding",
                    "Write a binding such as `\"Native.echo\" = \"rss_json_native::echo\"`.",
                    "manual",
                ),
            );
            continue;
        }

        let Some(contract) = interface_function_contracts.get(symbol) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` does not match any package interface function."),
                    span,
                    "unknown native binding symbol",
                )
                .with_cause("Native bindings are reviewable only when their RSScript side is declared in a package `.rssi` contract.")
                .with_fix(
                    "declare_native_interface",
                    format!("Declare `native fn {symbol}(...)` in the package interface, or remove this binding."),
                    "manual",
                ),
            );
            continue;
        };

        if !contract.effects.contains("native") {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` points to a non-native interface function."),
                    span.clone(),
                    "non-native binding symbol",
                )
                .with_cause("Only interface functions declared with the native boundary can be implemented by native wrapper bindings.")
                .with_fix(
                    "mark_native_interface",
                    format!("Declare `{symbol}` as `native fn` or add `effects(native)`, or remove this binding."),
                    "manual",
                ),
            );
        }

        if let Some(crate_name) = crate_name
            && !target.starts_with(&format!("{crate_name}::"))
        {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!(
                        "native binding `{symbol}` targets `{target}`, outside configured native crate `{crate_name}`."
                    ),
                    span,
                    "native binding crate mismatch",
                )
                .with_cause("The generated Cargo package only wires the configured native Rust crate as a dependency for this package.")
                .with_fix(
                    "use_configured_native_crate",
                    format!("Use a Rust path starting with `{crate_name}::`, or update `[native.rust].crate`."),
                    "manual",
                ),
            );
        }
    }

    diagnostics
}

fn native_binding_span(package_dir: &Path, symbol: &str) -> crate::diagnostic::Span {
    let path = package_dir.join("native/bindings.rssbind.toml");
    let file = path.display().to_string();
    let source = fs::read_to_string(&path).unwrap_or_default();
    for (index, line) in source.lines().enumerate() {
        if let Some(column) = line.find(symbol) {
            return crate::diagnostic::Span {
                file,
                line: index + 1,
                column: column + 1,
                length: symbol.len().max(1),
            };
        }
    }
    crate::diagnostic::Span {
        file,
        line: 1,
        column: 1,
        length: symbol.len().max(1),
    }
}

fn native_binding_manifest_span(package_dir: &Path) -> crate::diagnostic::Span {
    crate::diagnostic::Span {
        file: package_dir
            .join("native/bindings.rssbind.toml")
            .display()
            .to_string(),
        line: 1,
        column: 1,
        length: 10,
    }
}

pub(super) fn check_package_native_rust(
    package_dir: &Path,
    native: Option<&PackageNativeRustReview>,
) -> Result<Option<PackageNativeRustCheck>, String> {
    let Some(native) = native else {
        return Ok(None);
    };
    let native_root = package_dir.join(&native.path);
    let cargo_toml = native_root.join("Cargo.toml");
    let cargo_toml_present = cargo_toml.exists();
    let mut files = Vec::new();
    if native_root.exists() {
        super::collect_regular_files(&native_root, &mut files)?;
    }
    let unsafe_detected = native_rust_unsafe_detected(&files)?;
    let build_risk = native_build_script_risks(&files)?;
    let mut reasons = Vec::new();
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
    if unsafe_detected && native.unsafe_policy.as_deref() == Some("forbid") {
        reasons.push("native Rust unsafe usage detected".to_string());
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
    let risk = if ok {
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
    let native_root = cargo_toml
        .parent()
        .ok_or_else(|| format!("native Cargo.toml has no parent: {}", cargo_toml.display()))?;
    let scan_root = native_cargo_metadata_temp_dir(cargo_toml);
    if scan_root.exists() {
        let _ = fs::remove_dir_all(&scan_root);
    }
    super::copy_package_directory(native_root, &scan_root)?;
    let scan_cargo_toml = scan_root.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .arg("--manifest-path")
        .arg(&scan_cargo_toml)
        .output()
        .map_err(|error| {
            format!(
                "failed to run cargo metadata for {}: {error}",
                cargo_toml.display()
            )
        })?;
    let _ = fs::remove_dir_all(&scan_root);
    if !output.status.success() {
        reasons.push("native Rust cargo metadata failed".to_string());
        return Ok(NativeCargoMetadataScan::default());
    }

    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "failed to parse cargo metadata for {}: {error}",
            cargo_toml.display()
        )
    })?;
    let Some(package) = metadata.packages.first() else {
        reasons.push("native Rust cargo metadata reported no packages".to_string());
        return Ok(NativeCargoMetadataScan::default());
    };

    if let Some(expected) = native.crate_name.as_deref().map(str::trim)
        && !expected.is_empty()
        && expected != package.name
    {
        reasons.push(format!(
            "native Rust crate name `{expected}` does not match Cargo package `{}`",
            package.name
        ));
    }

    let mut target_kinds = package
        .targets
        .iter()
        .flat_map(|target| target.kind.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    target_kinds.sort();

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
        package_name: Some(package.name.clone()),
        target_kinds,
    })
}

fn native_cargo_metadata_temp_dir(cargo_toml: &Path) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let name = cargo_toml
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("native");
    env::temp_dir().join(format!(
        "rsscript-native-metadata-{name}-{}-{now}",
        std::process::id()
    ))
}

fn native_rust_unsafe_detected(files: &[PathBuf]) -> Result<bool, String> {
    for file in files {
        if file.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        if source_contains_rust_unsafe_keyword(&source) {
            return Ok(true);
        }
    }
    Ok(false)
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
        .and_then(|native| native.unsafe_policy.as_deref())
        .is_some_and(|policy| policy != "forbid")
}
