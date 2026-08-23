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

use crate::AotLoweringInput;
use crate::diagnostic::Diagnostic;
use crate::interfaces::{default_interfaces, standard_package_interfaces};
use crate::syntax::ast::Program;
use crate::syntax::parse_source;
use rsscript_aot_model::coverage_bucket;
use rsscript_semantics::{CompilationSession, ValidatedProgram};

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
pub use rsscript_aot_model::LowerCoverageReport;
pub use runtime_diagnostics::parse_runtime_diagnostics;
pub use rustc_remap::{remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines};
pub use source_map::parse_source_map_json;
pub use types::{GeneratedRustPackage, LoweredRust};

use helpers::{
    cargo_package_name, rust_package_main, toml_string, validate_executable_declarations,
};
use lowerer::RustLowerer;

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
    let runtime_all = crate::runtime_intrinsic_signatures();
    let runtime_supported = crate::default_runtime_intrinsic_supported_signatures()
        .expect("experiment-owned AOT runtime should be readable")
        .into_iter()
        .collect::<BTreeSet<_>>();

    LowerCoverageReport {
        runtime_intrinsics: coverage_bucket(runtime_all, runtime_supported),
        ast_statements: coverage_bucket(
            AST_STMT_VARIANTS.iter().map(|item| (*item).to_string()),
            RUST_LOWER_SUPPORTED_AST_STMT_VARIANTS
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
        ),
        ast_expressions: coverage_bucket(
            AST_EXPR_VARIANTS.iter().map(|item| (*item).to_string()),
            RUST_LOWER_SUPPORTED_AST_EXPR_VARIANTS
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
        ),
        function_kinds: coverage_bucket(
            FUNCTION_KINDS.iter().map(|item| (*item).to_string()),
            RUST_LOWER_SUPPORTED_FUNCTION_KINDS
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
        ),
    }
}

pub fn lower_source_to_rust(file: &str, source: &str) -> Result<String, Vec<Diagnostic>> {
    lower_source_to_rust_with_map(file, source).map(|lowered| lowered.rust_source)
}

pub fn lower_source_to_rust_with_map(
    file: &str,
    source: &str,
) -> Result<LoweredRust, Vec<Diagnostic>> {
    let validated = validated_session_sources(&[(file.to_string(), source.to_string())], &[])?;
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
    lower_aot_input(&AotLoweringInput {
        sources: sources.to_vec(),
        package_name: package_name.to_string(),
        runtime_path: runtime_path.to_string(),
        interfaces: interfaces.to_vec(),
        native_dependencies: native_dependencies.to_vec(),
    })
}

/// Compatibility lowering bridge. The experiment-owned input model ensures
/// callers do not couple generated-Rust lowering to compiler-local package
/// state while the implementation migrates out of this crate.
pub fn lower_aot_input(input: &AotLoweringInput) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    let validated = validated_session_sources(&input.sources, &input.interfaces)?;
    let database = validated.database();
    let program = database.program();
    let external_bindings = input
        .native_dependencies
        .iter()
        .flat_map(|dependency| dependency.bindings.iter())
        .map(|(symbol, target)| (symbol.clone(), target.clone()))
        .collect::<BTreeMap<_, _>>();
    let lowering_diagnostics = validate_executable_declarations(&program, &external_bindings);
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    let lowered = lower_validated_program_to_rust_with_map(database, external_bindings);
    let package_name = cargo_package_name(&input.package_name);
    let native_dependency_toml = input
        .native_dependencies
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
        toml_string(&input.runtime_path),
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

/// AOT is experimental, but it must consume the same immutable frontend
/// snapshot and validation boundary as the supported compiler paths. Core
/// interfaces are session-owned; callers supply only package/standard extras.
fn validated_session_sources(
    sources: &[(String, String)],
    interfaces: &[(String, String)],
) -> Result<ValidatedProgram, Vec<Diagnostic>> {
    let mut session = CompilationSession::default();
    for (path, contents) in interfaces {
        session
            .set_interface(path, contents)
            .expect("AOT inputs use normalized unique interface paths");
    }
    for (path, contents) in sources {
        session
            .set_file(path, contents)
            .expect("AOT inputs use normalized unique source paths");
    }
    session.workspace_validated()
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
    RustLowerer::new_validated(
        database.program(),
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

    use super::GeneratedRustPackage;

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
    fn runtime_string_arguments_follow_borrowed_str_abi() {
        let source = r#"
fn main(args: read List<String>) -> Int {
    let raw = Arguments.get_or_default(
        args: read args,
        index: 0,
        default: String.from_int(value: 7),
    )
    let parsed = String.parse_int(value: raw)
    Output.write(message: String.from_int(value: 9))
    match parsed {
        Some(value) => { return value }
        None => { return 0 }
    }
}
"#;
        let rust = super::lower_source_to_rust("runtime-string-abi.rss", source)
            .expect("runtime String arguments should lower");

        assert!(
            rust.contains(
                "arguments_get_or_default(args, 0i64, (rsscript_runtime::string_from_int(7i64)).as_str())"
            ),
            "{rust}"
        );
        assert!(
            rust.contains("rsscript_runtime::string_parse_int(raw.as_str())"),
            "{rust}"
        );
        assert!(
            rust.contains(
                "rsscript_runtime::log_write((rsscript_runtime::string_from_int(9i64)).as_str())"
            ),
            "{rust}"
        );
    }

    #[test]
    fn native_scalar_loop_package_uses_runtime_string_borrow_adaptation() {
        let source = include_str!("../../../../benchmarks/vm-jit/kernels/native_scalar_loop.rss");
        let runtime_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../aot-runtime")
            .to_string_lossy()
            .into_owned();
        let package = super::lower_source_to_rust_package(
            "native_scalar_loop.rss",
            source,
            "rsscript-aot-string-borrow-smoke",
            &runtime_path,
        )
        .expect("native scalar loop package should lower");

        assert!(
            package
                .lib_rs
                .contains("arguments_get_or_default(args, 0i64, (rsscript_runtime::string_from_int(default)).as_str())"),
            "{}",
            package.lib_rs
        );
        assert!(
            package
                .lib_rs
                .contains("rsscript_runtime::string_parse_int(raw.as_str())"),
            "{}",
            package.lib_rs
        );
        assert!(
            package.lib_rs.contains(
                "rsscript_runtime::log_write((rsscript_runtime::string_from_int(result)).as_str())"
            ),
            "{}",
            package.lib_rs
        );
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

        crate::write_generated_rust_package(&out_dir, &package)
            .expect("initial write should succeed");
        let lib_rs = out_dir.join("src/lib.rs");
        let mut permissions = fs::metadata(&lib_rs)
            .expect("lib.rs metadata should exist")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&lib_rs, permissions).expect("lib.rs should become readonly");

        crate::write_generated_rust_package(&out_dir, &package)
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
