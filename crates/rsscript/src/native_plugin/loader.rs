//! Dynamically loads the native host bindings a package needs to run in the VM.
//!
//! For a package with native dependencies, this reads the binding symbols and
//! their RSS signatures, generates a cdylib shim crate (see [`super::shim_gen`]),
//! builds it with cargo, `dlopen`s the result, and pulls out a
//! `(symbol, NativeInterpreterFn)` table the VM can call. Nothing is hard-coded
//! per package: the shim is derived entirely from the package's interface and
//! `bindings.rssbind.toml`.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use rss_native_abi::NativeInterpreterFn;

use crate::package::package_lowering_input;
use crate::syntax::ast::{DataEffect, Item, Param, TypeRef};
use crate::syntax::parse_source;

use super::shim_gen::{ShimBinding, ShimReturn, ShimType, generate_shim_crate};

/// Load (generating and building the shim cdylib on demand) the native host
/// bindings for `package_dir`. Returns an empty list for pure-RSS packages.
///
/// The loaded library is leaked so the returned function pointers stay valid for
/// the lifetime of the process.
pub fn load_package_native_bindings(
    package_dir: &Path,
) -> Result<Vec<(String, NativeInterpreterFn)>, String> {
    let input = package_lowering_input(package_dir)?;
    if input.native_dependencies.is_empty() {
        return Ok(Vec::new());
    }

    // Collect the native-fn signatures declared across the package interfaces.
    let mut signatures: BTreeMap<String, (Vec<Param>, Option<TypeRef>)> = BTreeMap::new();
    for (path, contents) in &input.interfaces {
        let program = parse_source(path, contents);
        for item in &program.items {
            if let Item::Function(decl) = item {
                signatures.insert(
                    decl.name.clone(),
                    (decl.params.clone(), decl.return_ty.clone()),
                );
            }
        }
    }

    // Build the shim binding specs plus the set of native crate deps to link.
    let mut bindings = Vec::new();
    let mut native_deps: Vec<(String, String)> = Vec::new();
    for dependency in &input.native_dependencies {
        native_deps.push((dependency.crate_name.clone(), dependency.path.clone()));
        for (symbol, rust_path) in &dependency.bindings {
            let (params, return_ty) = signatures.get(symbol).ok_or_else(|| {
                format!("native binding `{symbol}` has no interface signature for the VM shim.")
            })?;
            // Skip bindings the bridge can't represent (e.g. `mut` params, which
            // can't propagate in-place mutation across the value bridge, or
            // unsupported types). They simply aren't registered, so the program
            // works unless it actually calls one — then the VM reports
            // `no host binding for ...`.
            if let Some(binding) = try_build_binding(symbol, rust_path, params, return_ty.as_ref())
            {
                bindings.push(binding);
            }
        }
    }
    native_deps.sort();
    native_deps.dedup();
    bindings.sort_by(|left, right| left.symbol.cmp(&right.symbol));

    let library_path = build_shim_library(package_dir, &native_deps, &bindings)?;
    rss_native_abi::load_registry(&library_path)
}

/// Build a shim binding spec, or `None` if the bridge can't represent a
/// parameter or return type. `mut` parameters are supported: their positions are
/// recorded so the shim passes them by `&mut` and returns the mutated values for
/// the host to write back.
fn try_build_binding(
    symbol: &str,
    rust_path: &str,
    params: &[Param],
    return_ty: Option<&TypeRef>,
) -> Option<ShimBinding> {
    let mut param_types = Vec::with_capacity(params.len());
    let mut mut_indices = Vec::new();
    for (index, param) in params.iter().enumerate() {
        if param.effect == Some(DataEffect::Mut) {
            mut_indices.push(index);
        }
        param_types.push(shim_type(&param.ty)?);
    }
    Some(ShimBinding {
        symbol: symbol.to_string(),
        rust_path: rust_path.to_string(),
        params: param_types,
        ret: shim_return(return_ty)?,
        mut_indices,
    })
}

/// Map an RSS interface type to a shim value shape, if supported. Container types
/// recurse into their element type, so `List<Int>`, `Option<String>`, etc. work.
fn shim_type(ty: &TypeRef) -> Option<ShimType> {
    match ty.name.as_str() {
        "Unit" => Some(ShimType::Unit),
        "String" => Some(ShimType::String),
        "Int" => Some(ShimType::Int),
        "Float" => Some(ShimType::Float),
        "Bool" => Some(ShimType::Bool),
        "Bytes" => Some(ShimType::Bytes),
        "Path" => Some(ShimType::Path),
        "List" => ty
            .args
            .first()
            .and_then(shim_type)
            .map(|inner| ShimType::List(Box::new(inner))),
        "Option" => ty
            .args
            .first()
            .and_then(shim_type)
            .map(|inner| ShimType::Option(Box::new(inner))),
        _ => None,
    }
}

/// Map a binding's return type. `Result<T, String>` becomes [`ShimReturn::Result`];
/// anything else is a plain value. `None` if the (Ok) type is unsupported.
fn shim_return(return_ty: Option<&TypeRef>) -> Option<ShimReturn> {
    let Some(ty) = return_ty else {
        return Some(ShimReturn::Plain(ShimType::Unit));
    };
    if ty.name == "Result" {
        let ok = shim_type(ty.args.first()?)?;
        return Some(ShimReturn::Result(ok));
    }
    Some(ShimReturn::Plain(shim_type(ty)?))
}

/// Generate (if needed) and cargo-build the shim cdylib, returning its path.
fn build_shim_library(
    package_dir: &Path,
    native_deps: &[(String, String)],
    bindings: &[ShimBinding],
) -> Result<PathBuf, String> {
    let abi_path = format!("{}/../native-abi", env!("CARGO_MANIFEST_DIR"));

    // Stable per-package shim crate name (valid Rust identifier).
    let mut hasher = DefaultHasher::new();
    package_dir.hash(&mut hasher);
    let crate_name = format!("rss_shim_{:016x}", hasher.finish());

    let shim = generate_shim_crate(&crate_name, native_deps, &abi_path, bindings);

    let crate_dir = std::env::temp_dir()
        .join("rss-native-plugins")
        .join(&crate_name);
    fs::create_dir_all(crate_dir.join("src"))
        .map_err(|error| format!("failed to create shim crate dir: {error}"))?;
    write_if_changed(&crate_dir.join("Cargo.toml"), &shim.cargo_toml)?;
    write_if_changed(&crate_dir.join("src/lib.rs"), &shim.lib_rs)?;

    let manifest = crate_dir.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .map_err(|error| format!("failed to run cargo to build native shim: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to build native shim crate:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let library_path = crate_dir
        .join("target/release")
        .join(library_file_name(&crate_name));
    if !library_path.exists() {
        return Err(format!(
            "native shim built but library was not found at {}",
            library_path.display()
        ));
    }
    Ok(library_path)
}

/// Write `contents` only if the file differs, so an unchanged shim does not bump
/// mtimes and force a cargo rebuild.
fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
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
