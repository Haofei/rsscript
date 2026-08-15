//! Experiment-owned operations for generated Rust/AOT packages.

use std::path::Path;

use rsscript_aot_model::GeneratedRustPackage;
use rsscript_artifact_store::{
    GeneratedRustPackageFiles, write_generated_rust_package as write_generated_rust_files,
};

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

    use super::write_generated_rust_package;

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
}
