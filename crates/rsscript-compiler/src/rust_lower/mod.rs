#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::derivable_impls,
    clippy::doc_lazy_continuation,
    clippy::if_same_then_else,
    clippy::items_after_test_module,
    clippy::let_and_return,
    clippy::manual_contains,
    clippy::manual_slice_fill,
    clippy::mutable_key_type,
    clippy::needless_borrow,
    clippy::needless_lifetimes,
    clippy::needless_range_loop,
    clippy::nonminimal_bool,
    clippy::op_ref,
    clippy::ptr_arg,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_lazy_evaluations,
    clippy::useless_conversion
)]
// Experimental Rust AOT lowering keeps its lint debt local to this backend.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::analyzer::{validate_source, validate_sources_with_interfaces_without_core};
use crate::diagnostic::Diagnostic;
use crate::interfaces::{builtin_interfaces, default_interfaces, standard_package_interfaces};
use crate::runtime_abi;
use crate::syntax::ast::Program;
use crate::syntax::parse_source;
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageBucket {
    pub all: Vec<String>,
    pub supported: Vec<String>,
    pub missing: Vec<String>,
}

impl CoverageBucket {
    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn supported_count(&self) -> usize {
        self.supported.len()
    }

    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }
}

mod helpers;
mod intrinsics;
mod lower_async;
mod lower_decls;
mod lower_managed;
mod lower_match;
mod lower_types;
mod lowerer;
mod runtime_diagnostics;
mod rustc_remap;
mod source_map;
mod types;

pub use crate::package::NativeRustDependency;
pub use runtime_diagnostics::parse_runtime_diagnostics;
pub use rustc_remap::{remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines};
pub use source_map::parse_source_map_json;
pub use types::{GeneratedRustPackage, LoweredRust, RemappedRustcDiagnostic, RustSourceMapEntry};

use helpers::{
    cargo_package_name, rust_package_main, toml_string, validate_executable_declarations,
};
use lowerer::RustLowerer;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LowerCoverageReport {
    pub runtime_intrinsics: CoverageBucket,
    pub ast_statements: CoverageBucket,
    pub ast_expressions: CoverageBucket,
    pub function_kinds: CoverageBucket,
}

const AST_STMT_VARIANTS: &[&str] = &[
    "Let",
    "Return",
    "With",
    "MalformedWith",
    "If",
    "MalformedIf",
    "Loop",
    "MalformedLoop",
    "For",
    "MalformedFor",
    "Match",
    "MalformedMatch",
    "TaskGroup",
    "Select",
    "Break",
    "Continue",
    "LetElse",
    "Assign",
    "Expr",
    "Unknown",
];

const RUST_LOWER_SUPPORTED_AST_STMT_VARIANTS: &[&str] = &[
    "Let",
    "Return",
    "With",
    "If",
    "Loop",
    "For",
    "Match",
    "TaskGroup",
    "Select",
    "Break",
    "Continue",
    "LetElse",
    "Assign",
    "Expr",
];

const AST_EXPR_VARIANTS: &[&str] = &[
    "Ident",
    "Number",
    "String",
    "CharLiteral",
    "MultilineString",
    "ObjectLiteral",
    "MapLiteral",
    "ArrayLiteral",
    "Binary",
    "Field",
    "Index",
    "Call",
    "Effect",
    "Manage",
    "Spawn",
    "Await",
    "Try",
    "Closure",
    "Match",
    "Unknown",
];

const RUST_LOWER_SUPPORTED_AST_EXPR_VARIANTS: &[&str] = &[
    "Ident",
    "Number",
    "String",
    "CharLiteral",
    "MultilineString",
    "ObjectLiteral",
    "MapLiteral",
    "ArrayLiteral",
    "Binary",
    "Field",
    "Index",
    "Call",
    "Effect",
    "Manage",
    "Await",
    "Try",
    "Closure",
    "Match",
];

const FUNCTION_KINDS: &[&str] = &["sync", "async", "native"];
const RUST_LOWER_SUPPORTED_FUNCTION_KINDS: &[&str] = &["sync", "async", "native"];

pub fn lower_coverage_report() -> LowerCoverageReport {
    let runtime_all = runtime_abi::runtime_intrinsic_signatures();
    let runtime_supported = runtime_abi::runtime_intrinsic_supported_signatures()
        .into_iter()
        .collect::<BTreeSet<_>>();

    LowerCoverageReport {
        runtime_intrinsics: coverage_bucket_from_owned(runtime_all, runtime_supported),
        ast_statements: coverage_bucket(AST_STMT_VARIANTS, RUST_LOWER_SUPPORTED_AST_STMT_VARIANTS),
        ast_expressions: coverage_bucket(AST_EXPR_VARIANTS, RUST_LOWER_SUPPORTED_AST_EXPR_VARIANTS),
        function_kinds: coverage_bucket(FUNCTION_KINDS, RUST_LOWER_SUPPORTED_FUNCTION_KINDS),
    }
}

fn coverage_bucket(all: &[&str], supported: &[&str]) -> CoverageBucket {
    coverage_bucket_from_owned(
        all.iter().map(|item| (*item).to_string()).collect(),
        supported.iter().map(|item| (*item).to_string()).collect(),
    )
}

fn coverage_bucket_from_owned(mut all: Vec<String>, supported: BTreeSet<String>) -> CoverageBucket {
    all.sort();
    all.dedup();
    let all_set = all.iter().cloned().collect::<BTreeSet<_>>();
    let mut supported = supported
        .into_iter()
        .filter(|item| all_set.contains(item))
        .collect::<Vec<_>>();
    supported.sort();
    let supported_set = supported.iter().cloned().collect::<BTreeSet<_>>();
    let missing = all
        .iter()
        .filter(|item| !supported_set.contains(*item))
        .cloned()
        .collect();
    CoverageBucket {
        all,
        supported,
        missing,
    }
}

pub fn lower_source_to_rust(file: &str, source: &str) -> Result<String, Vec<Diagnostic>> {
    lower_source_to_rust_with_map(file, source).map(|lowered| lowered.rust_source)
}

pub fn lower_source_to_rust_with_map(
    file: &str,
    source: &str,
) -> Result<LoweredRust, Vec<Diagnostic>> {
    let validated = validate_source(file, source)?;
    let database = validated.database();
    let program = database.program();
    let lowering_diagnostics = validate_executable_declarations(&program, &BTreeMap::new());
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    Ok(lower_validated_program_to_rust_with_map(
        database,
        BTreeMap::new(),
    ))
}

pub fn lower_source_to_rust_package(
    file: &str,
    source: &str,
    package_name: &str,
    runtime_path: &str,
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    lower_source_to_rust_package_with_interfaces(file, source, package_name, runtime_path, &[])
}

pub fn lower_source_to_rust_package_with_interfaces(
    file: &str,
    source: &str,
    package_name: &str,
    runtime_path: &str,
    interfaces: &[(String, String)],
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    lower_sources_to_rust_package_with_interfaces(
        &[(file.to_string(), source.to_string())],
        package_name,
        runtime_path,
        interfaces,
    )
}

pub fn lower_sources_to_rust_package_with_interfaces(
    sources: &[(String, String)],
    package_name: &str,
    runtime_path: &str,
    interfaces: &[(String, String)],
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    let mut defaulted_interfaces = standard_package_interfaces()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect::<Vec<_>>();
    defaulted_interfaces.extend(interfaces.iter().cloned());
    defaulted_interfaces.sort_by(|left, right| left.0.cmp(&right.0));
    defaulted_interfaces.dedup_by(|left, right| left.0 == right.0);
    lower_sources_to_rust_package_with_options(
        sources,
        package_name,
        runtime_path,
        &defaulted_interfaces,
        &[],
    )
}

pub fn lower_sources_to_rust_package_with_options(
    sources: &[(String, String)],
    package_name: &str,
    runtime_path: &str,
    interfaces: &[(String, String)],
    native_dependencies: &[NativeRustDependency],
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    let mut interface_refs = builtin_interfaces().collect::<Vec<_>>();
    interface_refs.extend(
        interfaces
            .iter()
            .map(|(path, contents)| (path.as_str(), contents.as_str())),
    );
    let source_refs = sources
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let validated = validate_sources_with_interfaces_without_core(&source_refs, &interface_refs)?;
    let database = validated.database();
    let program = database.program();
    let external_bindings = native_dependencies
        .iter()
        .flat_map(|dependency| dependency.bindings.iter())
        .map(|(symbol, target)| (symbol.clone(), target.clone()))
        .collect::<BTreeMap<_, _>>();
    let lowering_diagnostics = validate_executable_declarations(&program, &external_bindings);
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    let lowered = lower_validated_program_to_rust_with_map(database, external_bindings);
    let package_name = cargo_package_name(package_name);
    let native_dependency_toml = native_dependencies
        .iter()
        .map(|dependency| {
            let feature_toml = if dependency.cargo_features.is_empty() {
                String::new()
            } else {
                format!(
                    ", features = [{}]",
                    dependency
                        .cargo_features
                        .iter()
                        .map(|feature| format!("\"{}\"", toml_string(feature)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!(
                "\"{}\" = {{ path = \"{}\", default-features = {}{} }}\n",
                toml_string(&dependency.crate_name),
                toml_string(&dependency.path),
                dependency.default_features,
                feature_toml
            )
        })
        .collect::<String>();
    let serde_dependency_toml = if program_uses_serde_derives(&program) {
        "serde = { version = \"1\", features = [\"derive\"] }\n"
    } else {
        ""
    };
    // Build-speed tuning for AOT ship/verify builds (P2): keep `overflow-checks`
    // on for release correctness, but split into many codegen units, disable LTO,
    // and enable incremental compilation so repeated builds are fast. The `dev`
    // profile gets a light opt-level for runnable-but-quick debug builds.
    let cargo_toml = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[profile.release]\noverflow-checks = true\ncodegen-units = 256\nlto = false\nincremental = true\n\n[profile.dev]\nopt-level = 1\nincremental = true\ncodegen-units = 256\n\n[dependencies]\nrsscript-runtime = {{ package = \"rsscript-aot-runtime\", path = \"{}\" }}\n{serde_dependency_toml}{native_dependency_toml}",
        toml_string(runtime_path),
    );
    let main_rs = rust_package_main(&program, &package_name);
    let source_map_json =
        serde_json::to_string_pretty(&lowered.source_map).expect("source map should serialize");

    Ok(GeneratedRustPackage {
        package_name,
        cargo_toml,
        lib_rs: lowered.rust_source,
        main_rs,
        source_map_json,
    })
}

fn program_uses_serde_derives(program: &crate::syntax::ast::Program) -> bool {
    program.items.iter().any(|item| match item {
        crate::syntax::ast::Item::Type(decl) => decl
            .derives
            .iter()
            .any(|derive| matches!(derive.as_str(), "JsonEncode" | "JsonDecode")),
        crate::syntax::ast::Item::SumType(sum) => sum
            .derives
            .iter()
            .any(|derive| matches!(derive.as_str(), "JsonEncode" | "JsonDecode")),
        _ => false,
    })
}

pub fn write_generated_rust_package(
    out_dir: &Path,
    package: &GeneratedRustPackage,
) -> Result<(), String> {
    let src_dir = out_dir.join("src");
    fs::create_dir_all(&src_dir)
        .map_err(|error| format!("failed to create {}: {error}", src_dir.display()))?;
    write_if_changed(&out_dir.join("Cargo.toml"), &package.cargo_toml)?;
    write_if_changed(&src_dir.join("lib.rs"), &package.lib_rs)?;
    if let Some(main_rs) = &package.main_rs {
        write_if_changed(&src_dir.join("main.rs"), main_rs)?;
    } else {
        remove_if_exists(&src_dir.join("main.rs"))?;
    }
    write_if_changed(
        &out_dir.join("rsscript-source-map.json"),
        &package.source_map_json,
    )?;
    Ok(())
}

fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("failed to read {}: {error}", path.display()));
        }
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to remove {}: {error}", path.display())),
    }
}

pub fn lower_program_to_rust(program: &Program) -> String {
    lower_program_to_rust_with_map(program).rust_source
}

pub fn lower_program_to_rust_with_map(program: &Program) -> LoweredRust {
    let interface_programs = default_interfaces()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    lower_program_to_rust_with_map_with_external_bindings(
        program,
        BTreeMap::new(),
        &interface_programs,
    )
}

fn lower_program_to_rust_with_map_with_external_bindings(
    program: &Program,
    external_bindings: BTreeMap<String, String>,
    interface_programs: &[Program],
) -> LoweredRust {
    // Apply module namespace isolation so the emitted Rust symbols match the
    // names the checker validated (single application per lowering run).
    let mut program = program.clone();
    crate::syntax::isolate_module_namespaces(&mut program);
    RustLowerer::new(&program, external_bindings, interface_programs).lower()
}

fn lower_validated_program_to_rust_with_map(
    database: &crate::semantic::SemanticDatabase,
    external_bindings: BTreeMap<String, String>,
) -> LoweredRust {
    let executable = rsscript_lowering::lower_validated_hir(database.hir());
    RustLowerer::new_validated(
        database.program(),
        &executable,
        database.hir().semantic_types(),
        external_bindings,
        database.interface_programs(),
    )
    .lower()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{GeneratedRustPackage, write_generated_rust_package};

    #[test]
    fn validated_lowering_uses_structural_generic_signature_facts() {
        let source = r#"

fn choose<U, W>(left: read U, right: take W) -> W {
    return right
}

fn main() -> Int {
    local right = 2
    return choose(left: read 1, right: take right)
}
"#;
        let rust = super::lower_source_to_rust("structural-lowering.rss", source)
            .expect("arbitrary generic parameter names should lower");

        assert!(rust.contains("fn choose<U: Clone, W: Clone>"), "{rust}");
        assert!(rust.contains("-> W"), "{rust}");
        assert!(rust.contains("choose(&(1i64), right)"), "{rust}");
    }

    #[test]
    fn generated_package_write_skips_unchanged_files() {
        let out_dir = unique_temp_dir("rsscript-write-generated");
        let package = GeneratedRustPackage {
            package_name: "rsscript_test".to_string(),
            cargo_toml: "[package]\nname = \"rsscript_test\"\n".to_string(),
            lib_rs: "pub fn value() -> i64 { 1 }\n".to_string(),
            main_rs: Some("fn main() {}\n".to_string()),
            source_map_json: "[]\n".to_string(),
        };

        write_generated_rust_package(&out_dir, &package).expect("initial write should succeed");
        let lib_rs = out_dir.join("src/lib.rs");
        let mut permissions = fs::metadata(&lib_rs)
            .expect("lib.rs metadata should exist")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&lib_rs, permissions).expect("lib.rs should become readonly");

        write_generated_rust_package(&out_dir, &package)
            .expect("unchanged readonly lib.rs should not be rewritten");

        let mut permissions = fs::metadata(&lib_rs)
            .expect("lib.rs metadata should exist")
            .permissions();
        // Restore writability only so the temp dir can be removed; the broad
        // permissions clippy warns about are irrelevant for a throwaway file.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&lib_rs, permissions).expect("lib.rs should become writable");
        fs::remove_dir_all(out_dir).expect("temp generated package should clean up");
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp directory should create");
        path
    }
}
