use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

#[derive(Debug, Deserialize)]
struct IntrinsicCatalog {
    schema: u32,
    intrinsic: Vec<IntrinsicId>,
    binding: Vec<IntrinsicBinding>,
}

#[derive(Debug, Deserialize)]
struct IntrinsicId {
    id: String,
    derived_from: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntrinsicBinding {
    namespace: String,
    name: String,
    vm_id: Option<String>,
    runtime_target: Option<String>,
    lowering: IntrinsicLowering,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum IntrinsicLowering {
    Direct,
    Special,
    RuntimeOnly,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestVirtual {
    #[serde(default)]
    has_default: bool,
    provider: Option<String>,
}

pub fn run_compiler_build() {
    if let Err(error) = write_core_package_index() {
        panic!("{error}");
    }
    if let Err(error) = write_reg_vm_runtime_intrinsics() {
        panic!("{error}");
    }
    if let Err(error) = write_compiled_cache_fingerprint() {
        panic!("{error}");
    }
}

/// Generated-Rust parity results are cached on disk by the test harness. Make
/// their key depend on every lowering source so a compiler edit cannot reuse a
/// stale executable result from an earlier build.
pub fn write_compiled_cache_fingerprint() -> Result<(), String> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = workspace_root(&manifest_dir)?;
    let roots = [
        workspace_root.join("crates/rsscript-compiler/src"),
        workspace_root.join("crates/rsscript-sdk/src"),
        workspace_root.join("crates/rsscript-vm/src"),
        workspace_root.join("Cargo.toml"),
        workspace_root.join("Cargo.lock"),
        workspace_root.join("stdlib"),
        workspace_root.join("packages"),
    ];
    let mut files = Vec::new();
    for root in &roots {
        collect_fingerprint_inputs(root, &mut files)?;
    }
    files.sort();
    files.dedup();

    let mut hasher = Sha256::new();
    hasher.update(b"rsscript-compiled-cache-v4\0");
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        hasher.update(
            path.strip_prefix(&workspace_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .as_bytes(),
        );
        hasher.update([0]);
        hasher.update(
            fs::read(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?,
        );
    }
    for name in [
        "TARGET",
        "HOST",
        "PROFILE",
        "RUSTC",
        "RSSCRIPT_SOURCE_REVISION",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(env::var(name).unwrap_or_default().as_bytes());
    }
    let mut features = env::vars()
        .filter(|(name, _)| name.starts_with("CARGO_FEATURE_"))
        .collect::<Vec<_>>();
    features.sort();
    for (name, value) in features {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let rustc_version = std::process::Command::new(rustc)
        .arg("-Vv")
        .output()
        .map_err(|error| format!("failed to inspect rustc: {error}"))?;
    if !rustc_version.status.success() {
        return Err("rustc -Vv failed while computing compiler fingerprint".to_string());
    }
    hasher.update(&rustc_version.stdout);
    let rustc_version_text = String::from_utf8_lossy(&rustc_version.stdout);
    let rustc_release = rustc_version_text
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .unwrap_or("unknown");
    println!("cargo:rustc-env=RSSCRIPT_RUSTC_VERSION={rustc_release}");
    println!(
        "cargo:rustc-env=RSSCRIPT_BUILD_TARGET={}",
        env::var("TARGET").unwrap_or_else(|_| "unknown".to_string())
    );
    let source_revision = env::var("RSSCRIPT_SOURCE_REVISION").unwrap_or_else(|_| {
        let git_head = workspace_root.join(".git/HEAD");
        if git_head.is_file() {
            println!("cargo:rerun-if-changed={}", git_head.display());
            if let Ok(head) = fs::read_to_string(&git_head)
                && let Some(reference) = head.trim().strip_prefix("ref: ")
            {
                let reference_path = workspace_root.join(".git").join(reference);
                if reference_path.is_file() {
                    println!("cargo:rerun-if-changed={}", reference_path.display());
                }
            }
        }
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&workspace_root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|revision| revision.trim().to_string())
            .filter(|revision| !revision.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    });
    println!("cargo:rustc-env=RSSCRIPT_SOURCE_REVISION={source_revision}");
    let fingerprint = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    println!("cargo:rustc-env=RSSCRIPT_COMPILED_CACHE_FINGERPRINT={fingerprint}");
    Ok(())
}

fn collect_fingerprint_inputs(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        if path.file_name().is_some_and(|name| name == "Cargo.lock")
            || path.extension().is_some_and(|extension| {
                matches!(extension.to_str(), Some("rs" | "rss" | "rssi" | "toml"))
            })
        {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    for entry in
        fs::read_dir(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?
    {
        let entry = entry.map_err(|error| format!("failed to read entry: {error}"))?;
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            && matches!(entry.file_name().to_str(), Some("target" | ".git"))
        {
            continue;
        }
        collect_fingerprint_inputs(&entry.path(), files)?;
    }
    Ok(())
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

pub fn write_reg_vm_runtime_intrinsics() -> Result<(), String> {
    println!("cargo:rerun-if-changed=intrinsics.toml");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let local_catalog = manifest_dir.join("intrinsics.toml");
    let catalog_path = if local_catalog.exists() {
        local_catalog
    } else {
        workspace_root(&manifest_dir)?.join("crates/rsscript-compiler/intrinsics.toml")
    };
    let source = fs::read_to_string(&catalog_path)
        .map_err(|error| format!("failed to read {}: {error}", catalog_path.display()))?;
    let catalog: IntrinsicCatalog = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", catalog_path.display()))?;
    validate_intrinsic_catalog(&catalog)?;

    let enum_variants = catalog
        .intrinsic
        .iter()
        .map(|intrinsic| format!("    {},", intrinsic.id))
        .collect::<Vec<_>>()
        .join("\n");
    let generated_enum = format!(
        "#[allow(dead_code)]\n#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]\npub(crate) enum RegIntrinsic {{\n{enum_variants}\n}}\n"
    );
    let direct_arms = catalog
        .binding
        .iter()
        .filter(|binding| binding.lowering == IntrinsicLowering::Direct)
        .map(|binding| {
            let vm_id = binding
                .vm_id
                .as_deref()
                .expect("validated direct intrinsic must have a VM id");
            format!(
                "        ({:?}, {:?}) => Some(RegIntrinsic::{}),",
                binding.namespace, binding.name, vm_id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let generated_lookup = format!(
        "fn qualified_intrinsic(namespace: &str, name: &str) -> Option<RegIntrinsic> {{\n    match (namespace, name) {{\n{direct_arms}\n        _ => None,\n    }}\n}}\n"
    );
    let runtime_entries = catalog
        .binding
        .iter()
        .filter_map(|binding| {
            binding.runtime_target.as_ref().map(|target| {
                format!(
                    "    runtime_intrinsic({:?}, {:?}, {:?}),",
                    binding.namespace, binding.name, target
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    let generated_abi =
        format!("const RUNTIME_INTRINSICS: &[RuntimeIntrinsic] = &[\n{runtime_entries}\n];\n");
    let runtime_signatures = catalog.binding.iter().filter(|binding| {
        binding.runtime_target.is_some()
            && matches!(
                binding.lowering,
                IntrinsicLowering::Direct | IntrinsicLowering::Special
            )
    });
    let special_signatures = catalog.binding.iter().filter(|binding| {
        binding.runtime_target.is_none()
            && matches!(
                binding.lowering,
                IntrinsicLowering::Direct | IntrinsicLowering::Special
            )
    });
    let generated_runtime = signature_slice(runtime_signatures);
    let generated_special_forms = signature_slice(special_signatures);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(out_dir.join("rss-reg-intrinsic-enum.rs"), generated_enum)
        .map_err(|error| format!("reg VM intrinsic enum should be written: {error}"))?;
    fs::write(
        out_dir.join("rss-reg-intrinsic-lookup.rs"),
        generated_lookup,
    )
    .map_err(|error| format!("reg VM intrinsic lookup should be written: {error}"))?;
    fs::write(out_dir.join("rss-runtime-intrinsics.rs"), generated_abi)
        .map_err(|error| format!("runtime intrinsic table should be written: {error}"))?;
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

/// Generate the typed MIR-to-v1 compatibility catalog for direct core-library
/// calls. MIR carries only a [`BuiltinId`]-compatible numeric identity; the
/// legacy v1 bytecode spelling is recovered by the code generator, never by a
/// backend inspecting source syntax.
pub fn write_mir_builtin_catalog() -> Result<(), String> {
    println!("cargo:rerun-if-changed=intrinsics.toml");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let local_catalog = manifest_dir.join("intrinsics.toml");
    let catalog_path = if local_catalog.exists() {
        local_catalog
    } else {
        workspace_root(&manifest_dir)?.join("crates/rsscript-compiler/intrinsics.toml")
    };
    let source = fs::read_to_string(&catalog_path)
        .map_err(|error| format!("failed to read {}: {error}", catalog_path.display()))?;
    let catalog: IntrinsicCatalog = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", catalog_path.display()))?;
    validate_intrinsic_catalog(&catalog)?;

    let direct = catalog
        .binding
        .iter()
        .filter(|binding| binding.lowering == IntrinsicLowering::Direct)
        .collect::<Vec<_>>();
    let lookup_arms = direct
        .iter()
        .enumerate()
        .map(|(index, binding)| {
            format!(
                "        ({:?}, {:?}) => Some(BuiltinId::new({index})),",
                binding.namespace, binding.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let vm_name_arms = direct
        .iter()
        .enumerate()
        .map(|(index, binding)| {
            let vm_id = binding
                .vm_id
                .as_deref()
                .expect("validated direct intrinsic must have a VM id");
            format!("        {index} => Some({vm_id:?}),")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let generated = format!(
        r#"/// Resolve a catalog-owned direct builtin without retaining its source spelling in MIR.
pub fn builtin_id(namespace: &str, name: &str) -> Option<BuiltinId> {{
    match (namespace, name) {{
{lookup_arms}
        _ => None,
    }}
}}

/// Compatibility spelling required only by the v1 bytecode encoder.
pub fn builtin_vm_name(id: BuiltinId) -> Option<&'static str> {{
    match id.index() {{
{vm_name_arms}
        _ => None,
    }}
}}
"#
    );
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(out_dir.join("rss-mir-builtin-catalog.rs"), generated)
        .map_err(|error| format!("MIR builtin catalog should be written: {error}"))?;
    Ok(())
}

fn validate_intrinsic_catalog(catalog: &IntrinsicCatalog) -> Result<(), String> {
    if catalog.schema != 1 {
        return Err(format!(
            "unsupported intrinsic catalog schema {}; expected 1",
            catalog.schema
        ));
    }
    let ids = catalog
        .intrinsic
        .iter()
        .map(|intrinsic| intrinsic.id.as_str())
        .collect::<HashSet<_>>();
    if ids.len() != catalog.intrinsic.len() {
        return Err("intrinsic catalog contains duplicate internal ids".to_string());
    }
    let mut bindings = HashSet::new();
    for binding in &catalog.binding {
        if !bindings.insert((binding.namespace.as_str(), binding.name.as_str())) {
            return Err(format!(
                "intrinsic catalog contains duplicate binding {}.{}",
                binding.namespace, binding.name
            ));
        }
        match binding.lowering {
            IntrinsicLowering::Direct => {
                let Some(vm_id) = binding.vm_id.as_deref() else {
                    return Err(format!(
                        "direct intrinsic {}.{} has no VM id",
                        binding.namespace, binding.name
                    ));
                };
                if !ids.contains(vm_id) {
                    return Err(format!(
                        "intrinsic {}.{} refers to unknown VM id {vm_id}",
                        binding.namespace, binding.name
                    ));
                }
            }
            IntrinsicLowering::Special => {
                if let Some(vm_id) = binding.vm_id.as_deref()
                    && !ids.contains(vm_id)
                {
                    return Err(format!(
                        "special intrinsic {}.{} refers to unknown VM id {vm_id}",
                        binding.namespace, binding.name
                    ));
                }
            }
            IntrinsicLowering::RuntimeOnly => {
                if binding.vm_id.is_some() {
                    return Err(format!(
                        "runtime-only intrinsic {}.{} must not declare a VM id",
                        binding.namespace, binding.name
                    ));
                }
            }
        }
        if binding.lowering == IntrinsicLowering::RuntimeOnly && binding.runtime_target.is_none() {
            return Err(format!(
                "runtime-only intrinsic {}.{} has no runtime target",
                binding.namespace, binding.name
            ));
        }
    }
    for intrinsic in &catalog.intrinsic {
        if let Some(parent) = intrinsic.derived_from.as_deref()
            && (parent == intrinsic.id || !ids.contains(parent))
        {
            return Err(format!(
                "derived intrinsic {} has invalid source {parent}",
                intrinsic.id
            ));
        }
    }
    Ok(())
}

fn signature_slice<'a>(bindings: impl Iterator<Item = &'a IntrinsicBinding>) -> String {
    let entries = bindings
        .map(|binding| {
            format!(
                "    {:?},",
                format!("{}.{}", binding.namespace, binding.name)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("&[\n{entries}\n]\n")
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
