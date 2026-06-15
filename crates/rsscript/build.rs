use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct CorePackageIndex {
    schema: &'static str,
    generated_by: &'static str,
    default_core: Vec<CoreInterfaceEntry>,
    packages: Vec<PackageEntry>,
}

#[derive(Debug, Serialize)]
struct CoreInterfaceEntry {
    path: String,
    module: String,
    functions: Vec<String>,
    types: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PackageEntry {
    kind: PackageKind,
    name: String,
    version: String,
    path: String,
    interface_files: Vec<String>,
    source_files: Vec<String>,
    native_rust: Option<NativeRustEntry>,
    virtual_package: Option<VirtualEntry>,
    dependencies: Vec<String>,
    dev_dependencies: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PackageKind {
    Core,
    Adapter,
    Package,
}

#[derive(Debug, Serialize)]
struct NativeRustEntry {
    crate_name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct VirtualEntry {
    has_default: bool,
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    package: ManifestPackage,
    #[serde(default)]
    interfaces: ManifestPathSection,
    #[serde(default)]
    sources: ManifestPathSection,
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    native: Option<ManifestNative>,
    #[serde(default, rename = "virtual")]
    virtual_package: Option<ManifestVirtual>,
}

#[derive(Debug, Deserialize)]
struct ManifestPackage {
    name: String,
    version: String,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestPathSection {
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestNative {
    #[serde(default)]
    rust: Option<ManifestNativeRust>,
}

#[derive(Debug, Deserialize)]
struct ManifestNativeRust {
    #[serde(default)]
    enabled: bool,
    path: Option<String>,
    #[serde(rename = "crate")]
    crate_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestVirtual {
    #[serde(default)]
    has_default: bool,
    provider: Option<String>,
}

fn main() {
    if let Err(error) = write_core_package_index() {
        panic!("{error}");
    }
    if let Err(error) = write_reg_vm_runtime_intrinsics() {
        panic!("{error}");
    }
}

fn write_core_package_index() -> Result<(), String> {
    println!("cargo:rerun-if-changed=../../stdlib");
    println!("cargo:rerun-if-changed=../../packages");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = workspace_root(&manifest_dir)?;
    let index = CorePackageIndex {
        schema: "rss.core_package_index.v1",
        generated_by: "build.rs",
        default_core: default_core_entries(&workspace_root)?,
        packages: package_entries(&workspace_root)?,
    };
    let json = serde_json::to_string_pretty(&index).expect("core package index should serialize");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(
        out_dir.join("rss-core-package-index.json"),
        format!("{json}\n"),
    )
    .map_err(|error| format!("core package index should be written: {error}"))?;
    Ok(())
}

fn write_reg_vm_runtime_intrinsics() -> Result<(), String> {
    println!("cargo:rerun-if-changed=src/reg_vm/mod.rs");
    println!("cargo:rerun-if-changed=src/runtime_abi.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source_path = manifest_dir.join("src/reg_vm/mod.rs");
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let resolver_signatures = collect_reg_vm_resolver_signatures(&source)?;
    let runtime_abi_path = manifest_dir.join("src/runtime_abi.rs");
    let runtime_abi_source = fs::read_to_string(&runtime_abi_path)
        .map_err(|error| format!("failed to read {}: {error}", runtime_abi_path.display()))?;
    let runtime_abi_signatures = collect_runtime_abi_signatures(&runtime_abi_source);
    let signatures = resolver_signatures
        .iter()
        .filter(|signature| runtime_abi_signatures.contains(*signature))
        .cloned()
        .collect::<Vec<_>>();
    let special_forms = resolver_signatures
        .into_iter()
        .filter(|signature| !runtime_abi_signatures.contains(signature))
        .collect::<Vec<_>>();
    let generated_runtime = format!(
        "&[\n{}\n]\n",
        signatures
            .iter()
            .map(|signature| format!("    {signature:?},"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let generated_special_forms = format!(
        "&[\n{}\n]\n",
        special_forms
            .iter()
            .map(|signature| format!("    {signature:?},"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(
        out_dir.join("rss-reg-vm-runtime-intrinsics.rs"),
        generated_runtime,
    )
    .map_err(|error| format!("reg VM runtime intrinsic index should be written: {error}"))?;
    fs::write(
        out_dir.join("rss-reg-vm-special-forms.rs"),
        generated_special_forms,
    )
    .map_err(|error| format!("reg VM special form index should be written: {error}"))?;
    Ok(())
}

fn workspace_root(manifest_dir: &Path) -> Result<PathBuf, String> {
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "could not derive workspace root from manifest dir {}",
                manifest_dir.display()
            )
        })
}

fn collect_reg_vm_resolver_signatures(source: &str) -> Result<Vec<String>, String> {
    // The name->intrinsic lowering table lives in two regions of reg_vm/mod.rs:
    //   1. `qualified_intrinsic(...)` — the pure `(ns, name) => RegIntrinsic`
    //      mappings, extracted into a standalone helper.
    //   2. the `else { match (namespace_root, name_root) { ... } }` in `call`
    //      — the special arms with inline lowering logic.
    // Both are scanned so the generated runtime/special-form indices stay
    // complete regardless of which region a signature lives in.
    let mut signatures = BTreeMap::<String, ()>::new();

    let pure_marker = "fn qualified_intrinsic(namespace: &str, name: &str) -> Option<RegIntrinsic> {";
    let pure_start = source
        .find(pure_marker)
        .ok_or_else(|| "reg VM intrinsic resolver helper was not found".to_string())?;
    let pure_region = &source[pure_start + pure_marker.len()..];
    let pure_end = pure_region
        .find("\n        _ => None,")
        .ok_or_else(|| "reg VM intrinsic resolver helper fallback was not found".to_string())?;
    collect_signature_pairs(&pure_region[..pure_end], &mut signatures);

    let special_marker = "let intrinsic = if let Some(intrinsic) =";
    let special_start = source
        .find(special_marker)
        .ok_or_else(|| "reg VM intrinsic resolver match was not found".to_string())?;
    let special_region = &source[special_start + special_marker.len()..];
    let special_end = special_region
        .find("\n                    _ => {")
        .ok_or_else(|| "reg VM intrinsic resolver fallback was not found".to_string())?;
    collect_signature_pairs(&special_region[..special_end], &mut signatures);

    Ok(signatures.into_keys().collect())
}

fn collect_signature_pairs(resolver: &str, signatures: &mut BTreeMap<String, ()>) {
    let bytes = resolver.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] != b'(' || bytes[index + 1] != b'"' {
            index += 1;
            continue;
        }
        let Some((namespace, after_namespace)) = parse_quoted_ascii(resolver, index + 1) else {
            index += 1;
            continue;
        };
        let mut cursor = after_namespace;
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b',') {
            index += 1;
            continue;
        }
        cursor += 1;
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'"') {
            index += 1;
            continue;
        }
        let Some((name, after_name)) = parse_quoted_ascii(resolver, cursor) else {
            index += 1;
            continue;
        };
        signatures.insert(format!("{namespace}.{name}"), ());
        index = after_name;
    }
}

fn collect_runtime_abi_signatures(source: &str) -> BTreeSet<String> {
    let mut signatures = BTreeSet::new();
    let mut index = 0;
    while let Some(relative) = source[index..].find("runtime_intrinsic") {
        index += relative + "runtime_intrinsic".len();
        let bytes = source.as_bytes();
        let mut cursor = index;
        if source[cursor..].starts_with("_with_handles") {
            cursor += "_with_handles".len();
        }
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'(') {
            continue;
        }
        cursor += 1;
        cursor = skip_ascii_whitespace(bytes, cursor);
        let Some((namespace, after_namespace)) = parse_quoted_ascii(source, cursor) else {
            continue;
        };
        cursor = skip_ascii_whitespace(bytes, after_namespace);
        if bytes.get(cursor) != Some(&b',') {
            continue;
        }
        cursor += 1;
        cursor = skip_ascii_whitespace(bytes, cursor);
        let Some((name, after_name)) = parse_quoted_ascii(source, cursor) else {
            continue;
        };
        signatures.insert(format!("{namespace}.{name}"));
        index = after_name;
    }
    signatures
}

fn parse_quoted_ascii(source: &str, quote_index: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    if bytes.get(quote_index) != Some(&b'"') {
        return None;
    }
    let mut end = quote_index + 1;
    while end < bytes.len() && bytes[end] != b'"' {
        if bytes[end] == b'\\' {
            return None;
        }
        end += 1;
    }
    (end < bytes.len()).then(|| (source[quote_index + 1..end].to_string(), end + 1))
}

fn default_core_entries(root: &Path) -> Result<Vec<CoreInterfaceEntry>, String> {
    let core_dir = root.join("stdlib");
    let mut files = Vec::new();
    collect_files_with_extension(&core_dir, "rssi", &mut files)?;
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let relative = relative_path(root, &path);
            Ok(CoreInterfaceEntry {
                module: relative
                    .trim_start_matches("stdlib/")
                    .trim_end_matches(".rssi")
                    .replace('/', "."),
                path: relative,
                functions: collect_functions(&source),
                types: collect_types(&source),
            })
        })
        .collect()
}

fn package_entries(root: &Path) -> Result<Vec<PackageEntry>, String> {
    let packages_dir = root.join("packages");
    let mut manifests = Vec::new();
    collect_named_files(&packages_dir, "rsspkg.toml", &mut manifests)?;
    manifests.sort();
    manifests
        .into_iter()
        .map(|manifest_path| package_entry(root, &manifest_path))
        .collect()
}

fn package_entry(root: &Path, manifest_path: &Path) -> Result<PackageEntry, String> {
    let package_dir = manifest_path.parent().ok_or_else(|| {
        format!(
            "package manifest has no parent: {}",
            manifest_path.display()
        )
    })?;
    let source = fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;
    let relative_dir = relative_path(root, package_dir);
    let native_rust = manifest
        .native
        .and_then(|native| native.rust)
        .and_then(|rust| {
            rust.enabled.then_some(NativeRustEntry {
                crate_name: rust.crate_name,
                path: rust.path,
            })
        });
    Ok(PackageEntry {
        kind: package_kind(&relative_dir),
        name: manifest.package.name,
        version: manifest.package.version,
        path: relative_dir,
        interface_files: package_files(
            package_dir,
            &manifest.interfaces.paths,
            "interface",
            "rssi",
        )?,
        source_files: package_files(package_dir, &manifest.sources.paths, "src", "rss")?,
        native_rust,
        virtual_package: manifest
            .virtual_package
            .map(|virtual_package| VirtualEntry {
                has_default: virtual_package.has_default,
                provider: virtual_package.provider,
            }),
        dependencies: manifest.dependencies.keys().cloned().collect(),
        dev_dependencies: manifest.dev_dependencies.keys().cloned().collect(),
    })
}

fn package_kind(path: &str) -> PackageKind {
    if path.starts_with("packages/core/") {
        PackageKind::Core
    } else if path.starts_with("packages/adapters/") {
        PackageKind::Adapter
    } else {
        PackageKind::Package
    }
}

fn package_files(
    package_dir: &Path,
    roots: &[String],
    default_root: &str,
    extension: &str,
) -> Result<Vec<String>, String> {
    let roots = if roots.is_empty() {
        vec![default_root.to_string()]
    } else {
        roots.to_vec()
    };
    let mut files = Vec::new();
    for root in roots {
        collect_files_with_extension(&package_dir.join(root), extension, &mut files)?;
    }
    files.sort();
    Ok(files
        .into_iter()
        .map(|path| relative_path(package_dir, &path))
        .collect())
}

fn collect_functions(source: &str) -> Vec<String> {
    let mut functions = collect_symbols_after_keywords(source, &["fn"]);
    functions.sort();
    functions.dedup();
    functions
}

fn collect_types(source: &str) -> Vec<String> {
    let mut types =
        collect_symbols_after_keywords(source, &["struct", "resource", "sum", "protocol"]);
    types.sort();
    types.dedup();
    types
}

fn collect_symbols_after_keywords(source: &str, keywords: &[&str]) -> Vec<String> {
    let mut symbols = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if starts_line_comment(bytes, index) {
            index = skip_line_comment(bytes, index + 2);
            continue;
        }
        if bytes[index] == b'"' {
            index = skip_string_literal(bytes, index + 1);
            continue;
        }
        if !is_ident_start(bytes[index]) {
            index += 1;
            continue;
        }
        let token_start = index;
        index += 1;
        while index < bytes.len() && is_ident_continue(bytes[index]) {
            index += 1;
        }
        let token = &source[token_start..index];
        if keywords.contains(&token) {
            index = skip_ascii_whitespace(bytes, index);
            if index < bytes.len() && is_symbol_start(bytes[index]) {
                let symbol_start = index;
                index += 1;
                while index < bytes.len() && is_symbol_continue(bytes[index]) {
                    index += 1;
                }
                symbols.push(source[symbol_start..index].to_string());
            }
        }
    }
    symbols
}

fn starts_line_comment(bytes: &[u8], index: usize) -> bool {
    bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'/')
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn skip_string_literal(bytes: &[u8], mut index: usize) -> usize {
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            break;
        }
    }
    index
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_symbol_start(byte: u8) -> bool {
    is_ident_start(byte)
}

fn is_symbol_continue(byte: u8) -> bool {
    is_ident_continue(byte) || byte == b'.'
}

fn collect_named_files(
    path: &Path,
    file_name: &str,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        collect_named_files(&entry.path(), file_name, files)?;
    }
    Ok(())
}

fn collect_files_with_extension(
    path: &Path,
    extension: &str,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        collect_files_with_extension(&entry.path(), extension, files)?;
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
