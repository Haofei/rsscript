use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::analyzer::{analyze_source_with_core, analyze_sources_with_interfaces};
use crate::diagnostic::Diagnostic;
use crate::interfaces::builtin_interfaces;
use crate::syntax::ast::{Program, merge_programs};
use crate::syntax::parse_source;

mod backend_check;
mod helpers;
mod lowerer;
mod runtime_diagnostics;
mod source_map;
mod types;

pub use backend_check::check_generated_rust_package;
pub use runtime_diagnostics::parse_runtime_diagnostics;
pub use source_map::{
    parse_source_map_json, remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines,
};
pub use types::{
    GeneratedRustPackage, LoweredRust, NativeRustDependency, RemappedRustcDiagnostic,
    RustBackendCheckResult, RustSourceMapEntry,
};

use helpers::{
    cargo_package_name, rust_package_main, toml_string, validate_executable_declarations,
};
use lowerer::RustLowerer;

pub fn lower_source_to_rust(file: &str, source: &str) -> Result<String, Vec<Diagnostic>> {
    lower_source_to_rust_with_map(file, source).map(|lowered| lowered.rust_source)
}

pub fn lower_source_to_rust_with_map(
    file: &str,
    source: &str,
) -> Result<LoweredRust, Vec<Diagnostic>> {
    let diagnostics = analyze_source_with_core(file, source);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return Err(diagnostics);
    }

    let program = parse_source(file, source);
    let lowering_diagnostics = validate_executable_declarations(&program, &BTreeMap::new());
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    Ok(lower_program_to_rust_with_map(&program))
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
    lower_sources_to_rust_package_with_options(sources, package_name, runtime_path, interfaces, &[])
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
    let diagnostics = analyze_sources_with_interfaces(&source_refs, &interface_refs);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return Err(diagnostics);
    }

    let program = merge_programs(
        sources
            .iter()
            .map(|(path, source)| parse_source(path, source)),
    );
    let native_bindings = native_dependencies
        .iter()
        .flat_map(|dependency| dependency.bindings.iter())
        .map(|(symbol, target)| (symbol.clone(), target.clone()))
        .collect::<BTreeMap<_, _>>();
    let lowering_diagnostics = validate_executable_declarations(&program, &native_bindings);
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    let lowered = lower_program_to_rust_with_map_with_native_bindings(&program, native_bindings);
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
                "\"{}\" = {{ path = \"{}\"{} }}\n",
                toml_string(&dependency.crate_name),
                toml_string(&dependency.path),
                feature_toml
            )
        })
        .collect::<String>();
    let cargo_toml = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nrsscript-runtime = {{ path = \"{}\" }}\n{native_dependency_toml}",
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

pub fn write_generated_rust_package(
    out_dir: &Path,
    package: &GeneratedRustPackage,
) -> Result<(), String> {
    let src_dir = out_dir.join("src");
    fs::create_dir_all(&src_dir)
        .map_err(|error| format!("failed to create {}: {error}", src_dir.display()))?;
    fs::write(out_dir.join("Cargo.toml"), &package.cargo_toml).map_err(|error| {
        format!(
            "failed to write {}: {error}",
            out_dir.join("Cargo.toml").display()
        )
    })?;
    fs::write(src_dir.join("lib.rs"), &package.lib_rs).map_err(|error| {
        format!(
            "failed to write {}: {error}",
            src_dir.join("lib.rs").display()
        )
    })?;
    if let Some(main_rs) = &package.main_rs {
        fs::write(src_dir.join("main.rs"), main_rs).map_err(|error| {
            format!(
                "failed to write {}: {error}",
                src_dir.join("main.rs").display()
            )
        })?;
    }
    fs::write(
        out_dir.join("rsscript-source-map.json"),
        &package.source_map_json,
    )
    .map_err(|error| {
        format!(
            "failed to write {}: {error}",
            out_dir.join("rsscript-source-map.json").display()
        )
    })?;
    Ok(())
}

pub fn lower_program_to_rust(program: &Program) -> String {
    lower_program_to_rust_with_map(program).rust_source
}

pub fn lower_program_to_rust_with_map(program: &Program) -> LoweredRust {
    lower_program_to_rust_with_map_with_native_bindings(program, BTreeMap::new())
}

fn lower_program_to_rust_with_map_with_native_bindings(
    program: &Program,
    native_bindings: BTreeMap<String, String>,
) -> LoweredRust {
    RustLowerer::new(program, native_bindings).lower()
}
