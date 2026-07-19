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
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use fs2::FileExt;
use rss_native_abi::NativeInterpreterFn;
use sha2::{Digest, Sha256};

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
    let canonical_package = package_dir
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize {}: {error}", package_dir.display()))?;

    // Stable per-package shim crate name (valid Rust identifier).
    let mut hasher = DefaultHasher::new();
    canonical_package.hash(&mut hasher);
    let crate_name = format!("rss_shim_{:016x}", hasher.finish());

    let shim = generate_shim_crate(&crate_name, native_deps, &abi_path, bindings);
    let cache_key = shim_cache_key(&shim.cargo_toml, &shim.lib_rs, native_deps, &abi_path)?;
    let cache_root = std::env::var_os("RSS_NATIVE_PLUGIN_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rss-native-plugins-v2"));
    create_private_dir(&cache_root)?;
    create_private_dir(&cache_root.join("locks"))?;
    create_private_dir(&cache_root.join("entries"))?;
    create_private_dir(&cache_root.join("staging"))?;

    let lock_path = cache_root.join("locks").join(format!("{cache_key}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("failed to open shim cache lock: {error}"))?;
    lock.lock_exclusive()
        .map_err(|error| format!("failed to lock shim cache entry: {error}"))?;

    let published = cache_root.join("entries").join(&cache_key);
    if let Some(library) = verified_cached_library(&published, &crate_name)? {
        return Ok(library);
    }
    if published.exists() {
        fs::remove_dir_all(&published)
            .map_err(|error| format!("failed to remove invalid shim cache entry: {error}"))?;
    }

    let staging = cache_root.join("staging").join(format!(
        "{cache_key}.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    create_private_dir(&staging.join("src"))?;
    fs::write(staging.join("Cargo.toml"), &shim.cargo_toml)
        .map_err(|error| format!("failed to write shim manifest: {error}"))?;
    fs::write(staging.join("src/lib.rs"), &shim.lib_rs)
        .map_err(|error| format!("failed to write shim source: {error}"))?;

    let manifest = staging.join("Cargo.toml");
    let lock_output = Command::new("cargo")
        .arg("generate-lockfile")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .map_err(|error| format!("failed to generate native shim lockfile: {error}"))?;
    if !lock_output.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to lock native shim dependencies:\n{}",
            String::from_utf8_lossy(&lock_output.stderr)
        ));
    }
    let output = Command::new("cargo")
        .arg("build")
        .arg("--locked")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .map_err(|error| format!("failed to run cargo to build native shim: {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to build native shim crate:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let library_path = staging
        .join("target/release")
        .join(library_file_name(&crate_name));
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
    Ok(published
        .join("target/release")
        .join(library_file_name(&crate_name)))
}

fn shim_cache_key(
    cargo_toml: &str,
    lib_rs: &str,
    native_deps: &[(String, String)],
    abi_path: &str,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"rss-native-shim-cache-v2\0");
    digest.update(rss_native_abi::ABI_VERSION.to_le_bytes());
    digest.update(cargo_toml.as_bytes());
    digest.update([0]);
    digest.update(lib_rs.as_bytes());
    let rustc = Command::new("rustc")
        .arg("-Vv")
        .output()
        .map_err(|error| format!("failed to inspect rustc for shim cache key: {error}"))?;
    if !rustc.status.success() {
        return Err("rustc -Vv failed while computing shim cache key".to_string());
    }
    digest.update(&rustc.stdout);
    digest.update(std::env::consts::ARCH.as_bytes());
    digest.update(std::env::consts::OS.as_bytes());
    hash_source_tree(Path::new(abi_path), &mut digest)?;
    for (_, path) in native_deps {
        hash_source_tree(Path::new(path), &mut digest)?;
    }
    Ok(hex::encode(digest.finalize()))
}

fn hash_source_tree(path: &Path, digest: &mut Sha256) -> Result<(), String> {
    let mut files = Vec::new();
    collect_cache_inputs(path, &mut files)?;
    files.sort();
    for file in files {
        digest.update(file.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(
            fs::read(&file)
                .map_err(|error| format!("failed to read {}: {error}", file.display()))?,
        );
    }
    Ok(())
}

fn collect_cache_inputs(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect native source {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "native shim cache input may not be a symlink: {}",
            path.display()
        ));
    }
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(path)
        .map_err(|error| format!("failed to read native source {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to read native source entry: {error}"))?;
        let child = entry.path();
        let name = entry.file_name();
        if child.is_dir() && matches!(name.to_str(), Some("target" | ".git")) {
            continue;
        }
        collect_cache_inputs(&child, files)?;
    }
    Ok(())
}

fn verified_cached_library(entry: &Path, crate_name: &str) -> Result<Option<PathBuf>, String> {
    let library = entry
        .join("target/release")
        .join(library_file_name(crate_name));
    let expected = match fs::read_to_string(entry.join("artifact.sha256")) {
        Ok(value) => value.trim().to_string(),
        Err(_) => return Ok(None),
    };
    if !library.is_file() || file_sha256(&library)? != expected {
        return Ok(None);
    }
    Ok(Some(library))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to secure {}: {error}", path.display()))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    //! The native-plugin loader's *signature → shim binding* mapping. The build +
    //! `dlopen` half is OS plumbing (libloading) exercised by the gated e2e test;
    //! the bug-prone logic is this type mapping, which was previously 0% covered.
    //! Inputs are parsed from tiny interface snippets so we don't hand-build spans.
    use super::*;
    use crate::syntax::ast::Item;
    use crate::syntax::parse_source;
    use rss_native_abi::NativeValue;

    fn sig(src: &str) -> (Vec<Param>, Option<TypeRef>) {
        let program = parse_source("t.rssi", src);
        for item in program.items {
            if let Item::Function(decl) = item {
                return (decl.params, decl.return_ty);
            }
        }
        panic!("interface snippet declared no function");
    }

    #[test]
    fn builds_plain_scalar_binding() {
        let (params, ret) =
            sig("native fn Adder.add(a: Int, b: Int) -> Int\n    effects(native)\n");
        let binding = try_build_binding("Adder.add", "adder::add", &params, ret.as_ref())
            .expect("scalar binding should build");
        assert_eq!(binding.symbol, "Adder.add");
        assert_eq!(binding.rust_path, "adder::add");
        assert_eq!(binding.params, vec![ShimType::Int, ShimType::Int]);
        assert_eq!(binding.ret, ShimReturn::Plain(ShimType::Int));
        assert!(binding.mut_indices.is_empty());
    }

    #[test]
    fn records_mut_param_positions() {
        let (params, ret) =
            sig("native fn Buf.fill(buffer: mut Bytes, value: Int) -> Unit\n    effects(native)\n");
        let binding = try_build_binding("Buf.fill", "buf::fill", &params, ret.as_ref())
            .expect("mut binding should build");
        assert_eq!(binding.mut_indices, vec![0]);
        assert_eq!(binding.params, vec![ShimType::Bytes, ShimType::Int]);
    }

    #[test]
    fn maps_result_and_nested_container_types() {
        let (params, ret) = sig(
            "native fn P.run(items: read List<String>) -> Result<Option<Int>, String>\n    effects(native)\n",
        );
        let binding = try_build_binding("P.run", "p::run", &params, ret.as_ref())
            .expect("container binding should build");
        assert_eq!(
            binding.params,
            vec![ShimType::List(Box::new(ShimType::String))]
        );
        assert_eq!(
            binding.ret,
            ShimReturn::Result(ShimType::Option(Box::new(ShimType::Int)))
        );
    }

    #[test]
    fn missing_return_type_is_plain_unit() {
        let (params, ret) = sig("native fn X.g(a: Int)\n    effects(native)\n");
        let binding = try_build_binding("X.g", "x::g", &params, ret.as_ref())
            .expect("unit-return binding should build");
        assert_eq!(binding.ret, ShimReturn::Plain(ShimType::Unit));
    }

    #[test]
    fn rejects_unsupported_param_type() {
        // A user type the value bridge can't represent → the binding is skipped
        // (the VM reports "no host binding" only if the program actually calls it).
        let (params, ret) = sig("native fn X.f(cfg: read Config) -> Unit\n    effects(native)\n");
        assert!(try_build_binding("X.f", "x::f", &params, ret.as_ref()).is_none());
    }

    #[test]
    fn rejects_unsupported_return_type() {
        let (params, ret) = sig("native fn X.h(a: Int) -> Config\n    effects(native)\n");
        assert!(try_build_binding("X.h", "x::h", &params, ret.as_ref()).is_none());
    }

    #[test]
    fn shim_build_is_atomic_and_uses_versioned_byte_abi() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-shim-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let native = root.join("native");
        fs::create_dir_all(native.join("src")).expect("native source directory should create");
        fs::write(
            native.join("Cargo.toml"),
            "[package]\nname = \"shim_test_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .expect("native manifest should write");
        fs::write(
            native.join("src/lib.rs"),
            "pub fn add_one(value: i64) -> i64 { value + 1 }\n",
        )
        .expect("native source should write");
        let deps = vec![(
            "shim_test_native".to_string(),
            native.to_string_lossy().into_owned(),
        )];
        let bindings = vec![ShimBinding {
            symbol: "Demo.add_one".to_string(),
            rust_path: "shim_test_native::add_one".to_string(),
            params: vec![ShimType::Int],
            ret: ShimReturn::Plain(ShimType::Int),
            mut_indices: vec![],
        }];

        let paths = std::thread::scope(|scope| {
            let first = scope.spawn(|| build_shim_library(&root, &deps, &bindings));
            let second = scope.spawn(|| build_shim_library(&root, &deps, &bindings));
            [
                first.join().expect("first build thread should not panic"),
                second.join().expect("second build thread should not panic"),
            ]
        });
        let first = paths[0].as_ref().expect("first shim build should pass");
        let second = paths[1].as_ref().expect("second shim build should pass");
        assert_eq!(first, second);

        let loaded = rss_native_abi::load_registry(first).expect("registry should load");
        let (_, function) = loaded
            .iter()
            .find(|(name, _)| name == "Demo.add_one")
            .expect("generated binding should exist");
        assert_eq!(
            function.call(vec![NativeValue::Int(4)]),
            Ok(NativeValue::Int(5))
        );
        drop(loaded);
        fs::remove_dir_all(&root).expect("test source should clean up");
    }
}
