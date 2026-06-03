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

#[derive(Debug, Clone, Copy)]
struct InterpreterIntrinsicSpec {
    namespace: &'static str,
    name: &'static str,
    variant: &'static str,
    eval_kind: InterpreterEvalKind,
}

#[derive(Debug, Clone, Copy)]
enum InterpreterEvalKind {
    IntToString,
    StringConcat,
    StringIsEmpty,
    StringLen,
    LogError,
    LogTrace,
    LogWrite,
}

const INTERPRETER_INTRINSICS: &[InterpreterIntrinsicSpec] = &[
    InterpreterIntrinsicSpec {
        namespace: "Int",
        name: "to_string",
        variant: "IntToString",
        eval_kind: InterpreterEvalKind::IntToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "concat",
        variant: "StringConcat",
        eval_kind: InterpreterEvalKind::StringConcat,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "from_int",
        variant: "StringFromInt",
        eval_kind: InterpreterEvalKind::IntToString,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "is_empty",
        variant: "StringIsEmpty",
        eval_kind: InterpreterEvalKind::StringIsEmpty,
    },
    InterpreterIntrinsicSpec {
        namespace: "String",
        name: "len",
        variant: "StringLen",
        eval_kind: InterpreterEvalKind::StringLen,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "error",
        variant: "LogError",
        eval_kind: InterpreterEvalKind::LogError,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "trace",
        variant: "LogTrace",
        eval_kind: InterpreterEvalKind::LogTrace,
    },
    InterpreterIntrinsicSpec {
        namespace: "Log",
        name: "write",
        variant: "LogWrite",
        eval_kind: InterpreterEvalKind::LogWrite,
    },
];

fn main() {
    if let Err(error) = write_core_package_index() {
        panic!("{error}");
    }
    if let Err(error) = write_interpreter_intrinsics() {
        panic!("{error}");
    }
}

fn write_core_package_index() -> Result<(), String> {
    println!("cargo:rerun-if-changed=core");
    println!("cargo:rerun-if-changed=rss");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let index = CorePackageIndex {
        schema: "rss.core_package_index.v1",
        generated_by: "build.rs",
        default_core: default_core_entries(&manifest_dir)?,
        packages: package_entries(&manifest_dir)?,
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

fn write_interpreter_intrinsics() -> Result<(), String> {
    println!("cargo:rerun-if-changed=core");
    println!("cargo:rerun-if-changed=rss");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    ensure_interpreter_intrinsic_interfaces(&manifest_dir)?;
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(
        out_dir.join("rss-interpreter-intrinsics-enum.rs"),
        generated_interpreter_intrinsic_enum(),
    )
    .map_err(|error| format!("interpreter intrinsic enum should be written: {error}"))?;
    fs::write(
        out_dir.join("rss-interpreter-intrinsics-dispatch.rs"),
        generated_interpreter_intrinsic_dispatch(),
    )
    .map_err(|error| format!("interpreter intrinsic dispatcher should be written: {error}"))?;
    Ok(())
}

fn ensure_interpreter_intrinsic_interfaces(root: &Path) -> Result<(), String> {
    let mut functions = BTreeSet::new();
    let mut files = Vec::new();
    collect_files_with_extension(&root.join("core"), "rssi", &mut files)?;
    collect_files_with_extension(&root.join("rss"), "rssi", &mut files)?;
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        functions.extend(collect_functions(&source));
    }
    for intrinsic in INTERPRETER_INTRINSICS {
        let signature = format!("{}.{}", intrinsic.namespace, intrinsic.name);
        if !functions.contains(&signature) {
            return Err(format!(
                "interpreter intrinsic `{signature}` has no bundled public interface signature"
            ));
        }
    }
    Ok(())
}

fn generated_interpreter_intrinsic_enum() -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n");
    out.push_str("pub(crate) enum InterpreterIntrinsic {\n");
    for intrinsic in INTERPRETER_INTRINSICS {
        out.push_str("    ");
        out.push_str(intrinsic.variant);
        out.push_str(",\n");
    }
    out.push_str("}\n");
    out
}

fn generated_interpreter_intrinsic_dispatch() -> String {
    let mut out = String::new();
    out.push_str(
        "fn eval_generated_runtime_intrinsic(\n    interpreter: &mut Interpreter<'_>,\n    intrinsic: InterpreterIntrinsic,\n    args: &[crate::syntax::ast::CallArg],\n) -> Result<Value, EvalError> {\n    match intrinsic {\n",
    );
    for intrinsic in INTERPRETER_INTRINSICS {
        out.push_str("        InterpreterIntrinsic::");
        out.push_str(intrinsic.variant);
        out.push_str(" => ");
        out.push_str(eval_kind_body(intrinsic.eval_kind));
        out.push_str(",\n");
    }
    out.push_str("    }\n}\n");
    out
}

fn eval_kind_body(kind: InterpreterEvalKind) -> &'static str {
    match kind {
        InterpreterEvalKind::IntToString => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::String(expect_int(value)?.to_string()))\n        }"
        }
        InterpreterEvalKind::StringConcat => {
            "{\n            let left = interpreter.eval_named_or_positional_arg(args, \"left\", 0)?;\n            let right = interpreter.eval_named_or_positional_arg(args, \"right\", 1)?;\n            Ok(Value::String(format!(\"{}{}\", expect_string(left)?, expect_string(right)?)))\n        }"
        }
        InterpreterEvalKind::StringIsEmpty => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Bool(expect_string(value)?.is_empty()))\n        }"
        }
        InterpreterEvalKind::StringLen => {
            "{\n            let value = interpreter.eval_first_arg(args)?;\n            Ok(Value::Int(expect_string(value)?.chars().count() as i64))\n        }"
        }
        InterpreterEvalKind::LogError => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"message\", 0)?;\n            interpreter.stderr.push_str(&expect_string(value)?);\n            interpreter.stderr.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::LogTrace => {
            "{\n            let event = interpreter.eval_named_or_positional_arg(args, \"event\", 0)?;\n            let message = interpreter.eval_named_or_positional_arg(args, \"message\", 1)?;\n            interpreter.stdout.push_str(\"trace \");\n            interpreter.stdout.push_str(&expect_string(event)?);\n            interpreter.stdout.push_str(\": \");\n            interpreter.stdout.push_str(&expect_string(message)?);\n            interpreter.stdout.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
        InterpreterEvalKind::LogWrite => {
            "{\n            let value = interpreter.eval_named_or_positional_arg(args, \"message\", 0)?;\n            interpreter.stdout.push_str(&expect_string(value)?);\n            interpreter.stdout.push('\\n');\n            Ok(Value::Unit)\n        }"
        }
    }
}

fn default_core_entries(root: &Path) -> Result<Vec<CoreInterfaceEntry>, String> {
    let core_dir = root.join("core");
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
                    .trim_start_matches("core/")
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
    let rss_dir = root.join("rss");
    let mut manifests = Vec::new();
    collect_named_files(&rss_dir, "rsspkg.toml", &mut manifests)?;
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
    if path.starts_with("rss/core/") {
        PackageKind::Core
    } else if path.starts_with("rss/adapters/") {
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
