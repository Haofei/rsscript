//! Experiment-owned operations for generated Rust/AOT packages.

use std::path::Path;

use rsscript_aot_model::GeneratedRustPackage;
use rsscript_artifact_store::{
    GeneratedRustPackageFiles, write_generated_rust_package as write_generated_rust_files,
};
use rsscript_project::NativeRustDependency;

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
}
