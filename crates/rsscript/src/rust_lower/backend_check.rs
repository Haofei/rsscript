use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::package::{
    CARGO_BUILD_TIMEOUT, CARGO_OUTPUT_MAX_BYTES, read_utf8_file_bounded, run_bounded_command,
};

use super::rustc_remap::remap_rustc_diagnostic_json_lines;
use super::source_map::parse_source_map_json;
use super::types::RustBackendCheckResult;

pub fn check_generated_rust_package(package_dir: &Path) -> Result<RustBackendCheckResult, String> {
    let source_map_path = package_dir.join("rsscript-source-map.json");
    let source_map_json = read_utf8_file_bounded(
        &source_map_path,
        CARGO_OUTPUT_MAX_BYTES as u64,
        "generated Rust source-map read",
    )?;
    let source_map = parse_source_map_json(&source_map_json)?;
    let manifest_path = package_dir.join("Cargo.toml");
    let mut cargo = Command::new("cargo");
    cargo
        .arg("check")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--message-format=json");
    if let Some(target_dir) = generated_target_dir_from_env() {
        cargo.env("CARGO_TARGET_DIR", target_dir);
    }
    let output = run_bounded_command(
        &mut cargo,
        "generated Rust cargo check",
        CARGO_BUILD_TIMEOUT,
        CARGO_OUTPUT_MAX_BYTES,
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let remapped = remap_rustc_diagnostic_json_lines(&source_map, &stdout)?;
    let diagnostics = remapped
        .into_iter()
        .map(|remapped| remapped.diagnostic)
        .collect();

    Ok(RustBackendCheckResult {
        success: output.status.success(),
        diagnostics,
        cargo_status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn generated_target_dir_from_env() -> Option<PathBuf> {
    let path = env::var_os("RSSCRIPT_GENERATED_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| ramdisk_root_dir().map(|root| root.join("rsscript-generated-target")))?;
    let _ = fs::create_dir_all(&path);

    Some(path)
}

fn ramdisk_root_dir() -> Option<PathBuf> {
    env::var_os("RSSCRIPT_RAMDISK_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
