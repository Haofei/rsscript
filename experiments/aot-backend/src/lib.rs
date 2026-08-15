//! Experiment-owned operations for generated Rust/AOT packages.

use std::path::Path;

use rsscript_aot_model::GeneratedRustPackage;
use rsscript_artifact_store::{
    GeneratedRustPackageFiles, write_generated_rust_package as write_generated_rust_files,
};
use rsscript_project::NativeRustDependency;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AotRuntimeIntrinsic {
    namespace: &'static str,
    name: &'static str,
    rust_target: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/rss-aot-runtime-intrinsics.rs"));

/// Returns the experimental generated-Rust runtime target for one neutral
/// intrinsic identity.
pub fn runtime_intrinsic_target(namespace: &str, name: &str) -> Option<&'static str> {
    AOT_RUNTIME_INTRINSICS
        .iter()
        .find(|intrinsic| intrinsic.namespace == namespace && intrinsic.name == name)
        .map(|intrinsic| intrinsic.rust_target)
}

/// All runtime-backed signatures understood by this AOT backend.
pub fn runtime_intrinsic_signatures() -> Vec<String> {
    AOT_RUNTIME_INTRINSICS
        .iter()
        .map(|intrinsic| format!("{}.{}", intrinsic.namespace, intrinsic.name))
        .collect()
}

/// Confirms that every generated-Rust target exists in the supplied runtime
/// source directory. This deliberately lives outside Core compiler code.
pub fn runtime_intrinsic_supported_signatures(runtime_src: &Path) -> Result<Vec<String>, String> {
    let functions = runtime_public_function_names(runtime_src)?;
    Ok(AOT_RUNTIME_INTRINSICS
        .iter()
        .filter_map(|intrinsic| {
            intrinsic
                .rust_target
                .strip_prefix("rsscript_runtime::")
                .filter(|target| functions.contains(*target))
                .map(|_| format!("{}.{}", intrinsic.namespace, intrinsic.name))
        })
        .collect())
}

/// Validates the checked-in experimental runtime associated with this backend.
pub fn default_runtime_intrinsic_supported_signatures() -> Result<Vec<String>, String> {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aot-runtime/src");
    runtime_intrinsic_supported_signatures(&runtime_src)
}

fn runtime_public_function_names(
    runtime_src: &Path,
) -> Result<std::collections::HashSet<String>, String> {
    let mut functions = std::collections::HashSet::new();
    for entry in std::fs::read_dir(runtime_src).map_err(|error| {
        format!(
            "failed to read AOT runtime {}: {error}",
            runtime_src.display()
        )
    })? {
        let path = entry
            .map_err(|error| format!("failed to read AOT runtime entry: {error}"))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).map_err(|error| {
            format!(
                "failed to read AOT runtime source {}: {error}",
                path.display()
            )
        })?;
        collect_runtime_public_functions(&source, &mut functions);
    }
    Ok(functions)
}

fn collect_runtime_public_functions(
    source: &str,
    functions: &mut std::collections::HashSet<String>,
) {
    let bytes = source.as_bytes();
    let mut index = 0;
    while let Some(relative) = source[index..].find("pub ") {
        index += relative + "pub ".len();
        let mut cursor = skip_ascii_whitespace(bytes, index);
        if source[cursor..].starts_with("async ") {
            cursor += "async ".len();
            cursor = skip_ascii_whitespace(bytes, cursor);
        }
        if !source[cursor..].starts_with("fn ") {
            continue;
        }
        cursor += "fn ".len();
        cursor = skip_ascii_whitespace(bytes, cursor);
        let start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor > start {
            functions.insert(source[start..cursor].to_string());
        }
        index = cursor;
    }
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

/// Immutable input consumed by the experimental Rust/AOT lowering path.
///
/// It deliberately contains no directory, VFS, artifact-store, or
/// compiler-private state, so callers can capture files once and pass this
/// value across the Core/experiments boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AotLoweringInput {
    pub sources: Vec<(String, String)>,
    pub package_name: String,
    pub runtime_path: String,
    pub interfaces: Vec<(String, String)>,
    pub native_dependencies: Vec<NativeRustDependency>,
}

impl AotLoweringInput {
    pub fn single_file(
        path: impl Into<String>,
        source: impl Into<String>,
        package_name: impl Into<String>,
        runtime_path: impl Into<String>,
    ) -> Self {
        Self {
            sources: vec![(path.into(), source.into())],
            package_name: package_name.into(),
            runtime_path: runtime_path.into(),
            interfaces: Vec::new(),
            native_dependencies: Vec::new(),
        }
    }
}

/// Publishes a generated Rust package through the confined artifact-store
/// writer. Repeated writes with identical contents are idempotent.
pub fn write_generated_rust_package(
    out_dir: &Path,
    package: &GeneratedRustPackage,
) -> Result<(), String> {
    write_generated_rust_files(
        out_dir,
        GeneratedRustPackageFiles {
            cargo_toml: &package.cargo_toml,
            lib_rs: &package.lib_rs,
            main_rs: package.main_rs.as_deref(),
            source_map_json: &package.source_map_json,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rsscript_aot_model::GeneratedRustPackage;

    use super::{AotLoweringInput, write_generated_rust_package};

    #[test]
    fn publication_is_idempotent() {
        let temp = tempfile::tempdir().expect("temporary output directory");
        let package = GeneratedRustPackage {
            package_name: "demo".to_string(),
            cargo_toml: "[package]\nname = \"demo\"\n".to_string(),
            lib_rs: "pub fn answer() -> i32 { 42 }\n".to_string(),
            main_rs: Some("fn main() {}\n".to_string()),
            source_map_json: "[]".to_string(),
        };
        write_generated_rust_package(temp.path(), &package).expect("first publication");
        write_generated_rust_package(temp.path(), &package).expect("same output is idempotent");
        assert_eq!(
            fs::read_to_string(temp.path().join("Cargo.toml")).expect("published Cargo.toml"),
            package.cargo_toml
        );
    }

    #[test]
    fn lowering_input_is_pure_and_owns_all_text() {
        let input = AotLoweringInput::single_file("main.rss", "fn main() {}", "demo", "/rt");
        assert_eq!(
            input.sources,
            [("main.rss".to_string(), "fn main() {}".to_string())]
        );
        assert!(input.interfaces.is_empty());
        assert!(input.native_dependencies.is_empty());
    }

    #[test]
    fn checked_in_aot_runtime_covers_all_generated_targets() {
        let supported = super::default_runtime_intrinsic_supported_signatures()
            .expect("checked-in AOT runtime should be readable");
        assert_eq!(supported, super::runtime_intrinsic_signatures());
    }
}
