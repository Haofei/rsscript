use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fs2::FileExt;

use crate::package::{
    CARGO_BUILD_TIMEOUT, CARGO_OUTPUT_MAX_BYTES, configure_reduced_build_environment,
    run_bounded_command,
};
use crate::syntax::ast::{DataEffect, Param, TypeRef};

use super::super::shim_gen::{
    ShimBinding, ShimDependency, ShimReturn, ShimType, generate_shim_crate,
};
use super::cache::{
    create_private_dir, file_sha256, open_private_lock, shim_cache_key, shim_crate_name,
    verified_cached_library,
};

/// Build a shim binding spec. `mut` parameters are supported: their positions are
/// recorded so the shim passes them by `&mut` and returns the mutated values for
/// the host to write back.
pub(super) fn build_binding(
    symbol: &str,
    rust_path: &str,
    params: &[Param],
    return_ty: Option<&TypeRef>,
) -> Result<ShimBinding, String> {
    let mut param_types = Vec::with_capacity(params.len());
    let mut mut_indices = Vec::new();
    for (index, param) in params.iter().enumerate() {
        if param.effect == Some(DataEffect::Mut) {
            mut_indices.push(index);
        }
        param_types.push(shim_type(&param.ty).map_err(|reason| {
            format!(
                "native binding `{symbol}` parameter `{}` has unsupported type `{}`: {reason}",
                param.name, param.ty.name
            )
        })?);
    }
    Ok(ShimBinding {
        symbol: symbol.to_string(),
        rust_path: rust_path.to_string(),
        params: param_types,
        ret: shim_return(return_ty).map_err(|reason| {
            let name = return_ty.map_or("Unit", |ty| ty.name.as_str());
            format!("native binding `{symbol}` has unsupported return type `{name}`: {reason}")
        })?,
        mut_indices,
    })
}

/// Map an RSS interface type to a shim value shape, if supported. Container types
/// recurse into their element type, so `List<Int>`, `Option<String>`, etc. work.
fn shim_type(ty: &TypeRef) -> Result<ShimType, String> {
    match ty.name.as_str() {
        "Unit" => Ok(ShimType::Unit),
        "String" => Ok(ShimType::String),
        "Int" => Ok(ShimType::Int),
        "Float" => Ok(ShimType::Float),
        "Bool" => Ok(ShimType::Bool),
        "Bytes" => Ok(ShimType::Bytes),
        "Path" => Ok(ShimType::Path),
        "List" | "Option" => {
            if ty.args.len() != 1 {
                return Err(format!("{} requires exactly one type argument", ty.name));
            }
            let inner = Box::new(shim_type(&ty.args[0])?);
            if ty.name == "List" {
                Ok(ShimType::List(inner))
            } else {
                Ok(ShimType::Option(inner))
            }
        }
        _ => Err("the native value bridge supports only Unit, String, Int, Float, Bool, Bytes, Path, List<T>, and Option<T>".to_string()),
    }
}

/// Map a binding's return type. `Result<T, String>` becomes [`ShimReturn::Result`];
/// anything else is a plain value. `None` if the (Ok) type is unsupported.
fn shim_return(return_ty: Option<&TypeRef>) -> Result<ShimReturn, String> {
    let Some(ty) = return_ty else {
        return Ok(ShimReturn::Plain(ShimType::Unit));
    };
    if ty.name == "Result" {
        if ty.args.len() != 2 || ty.args[1].name != "String" || !ty.args[1].args.is_empty() {
            return Err("Result returns must have the shape Result<T, String>".to_string());
        }
        return Ok(ShimReturn::Result(shim_type(&ty.args[0])?));
    }
    Ok(ShimReturn::Plain(shim_type(ty)?))
}

/// Generate (if needed) and cargo-build the shim cdylib, returning its path.
pub(super) fn build_shim_library(
    _snapshot_root: &Path,
    abi_path: &Path,
    native_deps: &[ShimDependency],
    bindings: &[ShimBinding],
) -> Result<PathBuf, String> {
    let crate_name = shim_crate_name(abi_path, native_deps, bindings)?;
    let library_file_name = library_file_name(&crate_name);

    let abi_path_label = abi_path.to_string_lossy();
    let shim = generate_shim_crate(&crate_name, native_deps, &abi_path_label, bindings);
    let cache_key = shim_cache_key(&shim.lib_rs, native_deps, &abi_path_label)?;
    let cache_root = std::env::var_os("RSS_NATIVE_PLUGIN_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rss-native-plugins-v2"));
    create_private_dir(&cache_root)?;
    create_private_dir(&cache_root.join("locks"))?;
    create_private_dir(&cache_root.join("entries"))?;
    create_private_dir(&cache_root.join("staging"))?;

    let lock_path = cache_root.join("locks").join(format!("{cache_key}.lock"));
    let lock = open_private_lock(&lock_path)?;
    lock.lock_exclusive()
        .map_err(|error| format!("failed to lock shim cache entry: {error}"))?;

    let published = cache_root.join("entries").join(&cache_key);
    if let Some(library) = verified_cached_library(&published, &library_file_name)? {
        return Ok(library);
    }
    if fs::symlink_metadata(&published).is_ok() {
        fs::remove_dir_all(&published)
            .map_err(|error| format!("failed to remove invalid shim cache entry: {error}"))?;
    }

    let staging = cache_root.join("staging").join(format!(
        "{cache_key}.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    create_private_dir(&staging)?;
    create_private_dir(&staging.join("src"))?;
    fs::write(staging.join("Cargo.toml"), &shim.cargo_toml)
        .map_err(|error| format!("failed to write shim manifest: {error}"))?;
    fs::write(staging.join("src/lib.rs"), &shim.lib_rs)
        .map_err(|error| format!("failed to write shim source: {error}"))?;

    let manifest = staging.join("Cargo.toml");
    let mut lock_command = Command::new("cargo");
    lock_command
        .arg("generate-lockfile")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(&manifest);
    configure_reduced_build_environment(&mut lock_command);
    let lock_output = run_bounded_command(
        &mut lock_command,
        "native shim offline lock snapshot preparation",
        CARGO_BUILD_TIMEOUT,
        CARGO_OUTPUT_MAX_BYTES,
    )
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;
    if !lock_output.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to prepare native shim lock snapshot offline:\n{}",
            String::from_utf8_lossy(&lock_output.stderr)
        ));
    }
    let mut metadata_command = Command::new("cargo");
    metadata_command
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--frozen")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(&manifest);
    configure_reduced_build_environment(&mut metadata_command);
    let metadata_output = run_bounded_command(
        &mut metadata_command,
        "native shim cargo metadata",
        CARGO_BUILD_TIMEOUT,
        CARGO_OUTPUT_MAX_BYTES,
    )
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;
    if !metadata_output.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "reviewed native shim lock is not complete for frozen offline build:\n{}",
            String::from_utf8_lossy(&metadata_output.stderr)
        ));
    }
    let mut build_command = Command::new("cargo");
    build_command
        .arg("build")
        .arg("--frozen")
        .arg("--offline")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest);
    configure_reduced_build_environment(&mut build_command);
    let output = run_bounded_command(
        &mut build_command,
        "native shim cargo build",
        CARGO_BUILD_TIMEOUT,
        CARGO_OUTPUT_MAX_BYTES,
    )
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to build native shim crate:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let library_path = staging.join("target/release").join(&library_file_name);
    if !library_path.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "native shim built but library was not found at {}",
            library_path.display()
        ));
    }
    let digest = file_sha256(&library_path)?;
    fs::write(staging.join("artifact.sha256"), format!("{digest}\n"))
        .map_err(|error| format!("failed to write shim artifact digest: {error}"))?;
    fs::rename(&staging, &published)
        .map_err(|error| format!("failed to publish shim cache entry atomically: {error}"))?;
    Ok(published.join("target/release").join(library_file_name))
}

fn library_file_name(crate_name: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("{crate_name}.dll")
    }
    #[cfg(target_os = "macos")]
    {
        format!("lib{crate_name}.dylib")
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        format!("lib{crate_name}.so")
    }
}
