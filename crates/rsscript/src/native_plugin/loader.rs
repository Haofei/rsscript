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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fs2::FileExt;
use rss_native_abi::NativeInterpreterFn;
use sha2::{Digest, Sha256};

use crate::package::{
    BoundedRegularFile, CARGO_BUILD_TIMEOUT, CARGO_OUTPUT_MAX_BYTES, TreeLimits, check_package_dir,
    collect_bounded_regular_files, configure_reduced_build_environment, package_lowering_input,
    package_native_plugin_build_dependencies, run_bounded_command,
};
use crate::syntax::ast::{DataEffect, Item, Param, TypeRef};
use crate::syntax::parse_source;

use super::shim_gen::{ShimBinding, ShimDependency, ShimReturn, ShimType, generate_shim_crate};

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
    authorize_native_package(package_dir)?;

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
    let native_build_dependencies = package_native_plugin_build_dependencies(package_dir)?;
    let mut native_deps = Vec::new();
    for dependency in &native_build_dependencies {
        native_deps.push(ShimDependency {
            crate_name: dependency.crate_name.clone(),
            path: dependency.path.clone(),
            cargo_features: dependency.cargo_features.clone(),
            default_features: dependency.default_features,
        });
        for (symbol, rust_path) in &dependency.bindings {
            let (params, return_ty) = signatures.get(symbol).ok_or_else(|| {
                format!("native binding `{symbol}` has no interface signature for the VM shim.")
            })?;
            bindings.push(build_binding(
                symbol,
                rust_path,
                params,
                return_ty.as_ref(),
            )?);
        }
    }
    native_deps.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then(left.path.cmp(&right.path))
            .then(left.cargo_features.cmp(&right.cargo_features))
            .then(left.default_features.cmp(&right.default_features))
    });
    native_deps.dedup();
    bindings.sort_by(|left, right| left.symbol.cmp(&right.symbol));

    let library_path = build_shim_library(package_dir, &native_deps, &bindings)?;
    rss_native_abi::load_registry(&library_path)
}

fn authorize_native_package(package_dir: &Path) -> Result<(), String> {
    let check = check_package_dir(package_dir)?;
    if check.ok {
        return Ok(());
    }
    let reasons = if check.reasons.is_empty() {
        "package check did not authorize native execution".to_string()
    } else {
        check.reasons.join("; ")
    };
    Err(format!(
        "native build/load denied because package review or policy did not authorize execution: {reasons}"
    ))
}

/// Build a shim binding spec. `mut` parameters are supported: their positions are
/// recorded so the shim passes them by `&mut` and returns the mutated values for
/// the host to write back.
fn build_binding(
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
fn build_shim_library(
    package_dir: &Path,
    native_deps: &[ShimDependency],
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
    let lock = open_private_lock(&lock_path)?;
    lock.lock_exclusive()
        .map_err(|error| format!("failed to lock shim cache entry: {error}"))?;

    let published = cache_root.join("entries").join(&cache_key);
    if let Some(library) = verified_cached_library(&published, &crate_name)? {
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
        .arg("--manifest-path")
        .arg(&manifest);
    configure_reduced_build_environment(&mut lock_command);
    let lock_output = run_bounded_command(
        &mut lock_command,
        "native shim cargo generate-lockfile",
        CARGO_BUILD_TIMEOUT,
        CARGO_OUTPUT_MAX_BYTES,
    )
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;
    if !lock_output.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to lock native shim dependencies:\n{}",
            String::from_utf8_lossy(&lock_output.stderr)
        ));
    }
    let mut build_command = Command::new("cargo");
    build_command
        .arg("build")
        .arg("--locked")
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
    native_deps: &[ShimDependency],
    abi_path: &str,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"rss-native-shim-cache-v2\0");
    digest.update(rss_native_abi::ABI_VERSION.to_le_bytes());
    digest.update(cargo_toml.as_bytes());
    digest.update([0]);
    digest.update(lib_rs.as_bytes());
    let mut rustc_command = Command::new("rustc");
    rustc_command.arg("-Vv");
    configure_reduced_build_environment(&mut rustc_command);
    let rustc = run_bounded_command(
        &mut rustc_command,
        "rustc version inspection for native shim cache key",
        Duration::from_secs(30),
        256 * 1024,
    )?;
    if !rustc.status.success() {
        return Err("rustc -Vv failed while computing shim cache key".to_string());
    }
    digest.update(&rustc.stdout);
    digest.update(std::env::consts::ARCH.as_bytes());
    digest.update(std::env::consts::OS.as_bytes());
    hash_source_tree(Path::new(abi_path), &mut digest)?;
    for dependency in native_deps {
        hash_source_tree(Path::new(&dependency.path), &mut digest)?;
    }
    Ok(hex::encode(digest.finalize()))
}

fn hash_source_tree(path: &Path, digest: &mut Sha256) -> Result<(), String> {
    let limits = TreeLimits::default();
    let files = collect_bounded_regular_files(
        path,
        limits.clone(),
        "native shim cache input scan",
        |_parent, entry| {
            matches!(
                entry.file_name().to_str(),
                Some("target" | ".git" | ".DS_Store")
            )
        },
    )?;
    let mut remaining = limits.max_bytes;
    for BoundedRegularFile {
        path: file,
        bytes: expected,
    } in files
    {
        if expected > remaining {
            return Err(format!(
                "native shim cache input hashing exceeded total byte limit of {} at {}",
                limits.max_bytes,
                file.display()
            ));
        }
        digest.update(file.to_string_lossy().as_bytes());
        digest.update([0]);
        let hashed = hash_file_streaming_bounded(&file, digest, expected)?;
        remaining -= hashed;
    }
    Ok(())
}

fn hash_file_streaming(path: &Path, digest: &mut Sha256) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "native cache hashing requires a regular file, not a symlink: {}",
            path.display()
        ));
    }
    hash_file_streaming_bounded(path, digest, metadata.len()).map(|_| ())
}

fn hash_file_streaming_bounded(
    path: &Path,
    digest: &mut Sha256,
    max_bytes: u64,
) -> Result<u64, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hashed = 0_u64;
    loop {
        let remaining = max_bytes.saturating_sub(hashed);
        let read_cap = usize::try_from(remaining.saturating_add(1))
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = file
            .read(&mut buffer[..read_cap])
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hashed = hashed.checked_add(read as u64).ok_or_else(|| {
            format!(
                "native cache hash byte count overflow at {}",
                path.display()
            )
        })?;
        if hashed > max_bytes {
            return Err(format!(
                "native cache input exceeded approved byte limit of {max_bytes} while hashing {}",
                path.display()
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok(hashed)
}

fn verified_cached_library(entry: &Path, crate_name: &str) -> Result<Option<PathBuf>, String> {
    match fs::symlink_metadata(entry) {
        Ok(_) => validate_private_dir(entry)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to inspect native shim cache entry {}: {error}",
                entry.display()
            ));
        }
    }
    validate_owned_dir(&entry.join("target"))?;
    validate_owned_dir(&entry.join("target/release"))?;
    let library = entry
        .join("target/release")
        .join(library_file_name(crate_name));
    let digest_path = entry.join("artifact.sha256");
    match fs::symlink_metadata(&digest_path) {
        Ok(_) => validate_private_file(&digest_path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to inspect native shim cache digest: {error}"
            ));
        }
    }
    let expected = fs::read_to_string(&digest_path)
        .map_err(|error| format!("failed to read native shim cache digest: {error}"))?
        .trim()
        .to_string();
    validate_private_file(&library)?;
    if file_sha256(&library)? != expected {
        return Ok(None);
    }
    Ok(Some(library))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    hash_file_streaming(path, &mut digest)?;
    Ok(hex::encode(digest.finalize()))
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!(
                "native shim cache path must be a real directory, not a symlink: {}",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
        }
        Err(error) => {
            return Err(format!(
                "failed to inspect cache directory {}: {error}",
                path.display()
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to secure {}: {error}", path.display()))?;
    }
    validate_private_dir(path)
}

fn validate_private_dir(path: &Path) -> Result<(), String> {
    let metadata = validate_owned_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "native shim cache directory is accessible by other users: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_owned_dir(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect cache directory {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "native shim cache path must be a real directory, not a symlink: {}",
            path.display()
        ));
    }
    validate_cache_owner(path, &metadata)?;
    Ok(metadata)
}

fn validate_private_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect cache file {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "native shim cache artifact must be a regular file, not a symlink: {}",
            path.display()
        ));
    }
    validate_cache_owner(path, &metadata)
}

#[cfg(unix)]
fn validate_cache_owner(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let effective_uid = current_process_uid()?;
    if metadata.uid() != effective_uid {
        return Err(format!(
            "native shim cache path is not owned by the current user: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn current_process_uid() -> Result<u32, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::sync::OnceLock;

    static UID: OnceLock<Result<u32, String>> = OnceLock::new();
    UID.get_or_init(|| {
        let probe = std::env::temp_dir().join(format!(
            ".rss-native-owner-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(&probe)
            .map_err(|error| format!("failed to create cache owner probe: {error}"))?;
        let uid = file
            .metadata()
            .map_err(|error| format!("failed to inspect cache owner probe: {error}"))?
            .uid();
        drop(file);
        fs::remove_file(&probe)
            .map_err(|error| format!("failed to remove cache owner probe: {error}"))?;
        Ok(uid)
    })
    .clone()
}

#[cfg(not(unix))]
fn validate_cache_owner(_path: &Path, _metadata: &fs::Metadata) -> Result<(), String> {
    Ok(())
}

fn open_private_lock(path: &Path) -> Result<fs::File, String> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(format!(
            "native shim cache lock must be a regular file, not a symlink: {}",
            path.display()
        ));
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("failed to open shim cache lock {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect shim cache lock: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "native shim cache lock is not a regular file: {}",
            path.display()
        ));
    }
    validate_cache_owner(path, &metadata)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "native shim cache lock is accessible by other users: {}",
                path.display()
            ));
        }
    }
    Ok(file)
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
    fn streaming_hash_rejects_bytes_beyond_scanned_size() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-hash-growth-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("growing.rs");
        fs::write(&path, b"12345").expect("fixture file");
        let mut digest = Sha256::new();

        let error = hash_file_streaming_bounded(&path, &mut digest, 4)
            .expect_err("hashing beyond scanned size must fail");
        let _ = fs::remove_dir_all(&root);
        assert!(
            error.contains("approved byte limit of 4"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn native_authorization_rejects_package_without_successful_check() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-authorization-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory");
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"authorization-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("fixture manifest");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("fixture source");

        let error = authorize_native_package(&root)
            .expect_err("a package without an approved lock/check must not authorize native load");
        let _ = fs::remove_dir_all(&root);

        assert!(error.contains("native build/load denied"), "{error}");
        assert!(error.contains("rsspkg.lock missing"), "{error}");
    }

    #[test]
    fn builds_plain_scalar_binding() {
        let (params, ret) =
            sig("native fn Adder.add(a: Int, b: Int) -> Int\n    effects(native)\n");
        let binding = build_binding("Adder.add", "adder::add", &params, ret.as_ref())
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
        let binding = build_binding("Buf.fill", "buf::fill", &params, ret.as_ref())
            .expect("mut binding should build");
        assert_eq!(binding.mut_indices, vec![0]);
        assert_eq!(binding.params, vec![ShimType::Bytes, ShimType::Int]);
    }

    #[test]
    fn maps_result_and_nested_container_types() {
        let (params, ret) = sig(
            "native fn P.run(items: read List<String>) -> Result<Option<Int>, String>\n    effects(native)\n",
        );
        let binding = build_binding("P.run", "p::run", &params, ret.as_ref())
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
        let binding = build_binding("X.g", "x::g", &params, ret.as_ref())
            .expect("unit-return binding should build");
        assert_eq!(binding.ret, ShimReturn::Plain(ShimType::Unit));
    }

    #[test]
    fn rejects_unsupported_param_type() {
        let (params, ret) = sig("native fn X.f(cfg: read Config) -> Unit\n    effects(native)\n");
        let error = build_binding("X.f", "x::f", &params, ret.as_ref())
            .expect_err("unsupported parameter must fail shim construction");
        assert!(error.contains("parameter `cfg` has unsupported type `Config`"));
    }

    #[test]
    fn rejects_unsupported_return_type() {
        let (params, ret) = sig("native fn X.h(a: Int) -> Config\n    effects(native)\n");
        let error = build_binding("X.h", "x::h", &params, ret.as_ref())
            .expect_err("unsupported return must fail shim construction");
        assert!(error.contains("unsupported return type `Config`"));
    }

    #[test]
    fn rejects_result_with_non_string_error_type() {
        let (params, ret) =
            sig("native fn X.h(a: Int) -> Result<Int, Config>\n    effects(native)\n");
        let error = build_binding("X.h", "x::h", &params, ret.as_ref())
            .expect_err("unsupported Result error must fail shim construction");
        assert!(error.contains("Result<T, String>"));
    }

    #[cfg(unix)]
    #[test]
    fn private_cache_directory_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rss-native-cache-symlink-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let real = root.join("real");
        fs::create_dir_all(&real).expect("real directory should create");
        let linked = root.join("linked");
        symlink(&real, &linked).expect("cache symlink should create");

        let error = create_private_dir(&linked).expect_err("symlinked cache must be rejected");
        let _ = fs::remove_dir_all(&root);

        assert!(error.contains("not a symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn private_cache_lock_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rss-native-cache-lock-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        create_private_dir(&root).expect("cache directory should create");
        let target = root.join("target");
        fs::write(&target, "do not lock").expect("target should write");
        let linked = root.join("entry.lock");
        symlink(&target, &linked).expect("lock symlink should create");

        let error = open_private_lock(&linked).expect_err("symlinked lock must be rejected");
        let _ = fs::remove_dir_all(&root);

        assert!(error.contains("regular file, not a symlink"));
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
        let deps = vec![ShimDependency {
            crate_name: "shim_test_native".to_string(),
            path: native.to_string_lossy().into_owned(),
            cargo_features: vec![],
            default_features: true,
        }];
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
