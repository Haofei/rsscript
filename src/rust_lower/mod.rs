use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::analyzer::{analyze_source_with_core, analyze_sources_with_interfaces_without_core};
use crate::diagnostic::Diagnostic;
use crate::interfaces::{builtin_interfaces, default_interfaces, standard_package_interfaces};
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
    let interface_programs = default_interfaces()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    Ok(lower_program_to_rust_with_map_with_interfaces(
        &program,
        &interface_programs,
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
    let diagnostics = analyze_sources_with_interfaces_without_core(&source_refs, &interface_refs);
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
    let interface_programs = interface_refs
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let lowered = lower_program_to_rust_with_map_with_native_bindings(
        &program,
        native_bindings,
        &interface_programs,
    );
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
    let serde_dependency_toml = if program_uses_serde_derives(&program) {
        "serde = { version = \"1\", features = [\"derive\"] }\n"
    } else {
        ""
    };
    let cargo_toml = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nrsscript-runtime = {{ path = \"{}\" }}\n{serde_dependency_toml}{native_dependency_toml}",
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{GeneratedRustPackage, write_generated_rust_package};

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

pub fn lower_program_to_rust_with_map(program: &Program) -> LoweredRust {
    let interface_programs = default_interfaces()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    lower_program_to_rust_with_map_with_native_bindings(
        program,
        BTreeMap::new(),
        &interface_programs,
    )
}

fn lower_program_to_rust_with_map_with_native_bindings(
    program: &Program,
    native_bindings: BTreeMap<String, String>,
    interface_programs: &[Program],
) -> LoweredRust {
    RustLowerer::new(program, native_bindings, interface_programs).lower()
}

fn lower_program_to_rust_with_map_with_interfaces(
    program: &Program,
    interface_programs: &[Program],
) -> LoweredRust {
    lower_program_to_rust_with_map_with_native_bindings(
        program,
        BTreeMap::new(),
        interface_programs,
    )
}
