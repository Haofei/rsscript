use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::package::{
    BoundedRegularFile, TreeLimits, collect_bounded_regular_files,
    configure_reduced_build_environment, run_bounded_command,
};

use super::super::shim_gen::{ShimBinding, ShimDependency};

pub(super) fn shim_crate_name(
    abi_path: &Path,
    native_deps: &[ShimDependency],
    bindings: &[ShimBinding],
) -> Result<String, String> {
    let mut identity = Sha256::new();
    identity.update(b"rss-native-shim-identity-v3\0");
    identity.update(rss_native_abi::ABI_VERSION.to_le_bytes());
    hash_source_tree(abi_path, &mut identity)?;
    for dependency in native_deps {
        identity.update(dependency.crate_name.as_bytes());
        identity.update([0]);
        identity.update([u8::from(dependency.default_features)]);
        for feature in &dependency.cargo_features {
            identity.update(feature.as_bytes());
            identity.update([0]);
        }
        hash_source_tree(Path::new(&dependency.path), &mut identity)?;
    }
    for binding in bindings {
        identity.update(format!("{binding:?}").as_bytes());
        identity.update([0]);
    }
    Ok(format!(
        "rss_shim_{}",
        &hex::encode(identity.finalize())[..16]
    ))
}

pub(super) fn shim_cache_key(
    lib_rs: &str,
    native_deps: &[ShimDependency],
    abi_path: &str,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"rss-native-shim-cache-v3\0");
    digest.update(rss_native_abi::ABI_VERSION.to_le_bytes());
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
        let relative = file.strip_prefix(path).map_err(|_| {
            format!(
                "native cache input escaped approved root: {}",
                file.display()
            )
        })?;
        digest.update(relative.to_string_lossy().as_bytes());
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

pub(super) fn hash_file_streaming_bounded(
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

pub(super) fn verified_cached_library(
    entry: &Path,
    library_file_name: &str,
) -> Result<Option<PathBuf>, String> {
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
    let library = entry.join("target/release").join(library_file_name);
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

pub(super) fn file_sha256(path: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    hash_file_streaming(path, &mut digest)?;
    Ok(hex::encode(digest.finalize()))
}

pub(super) fn create_private_dir(path: &Path) -> Result<(), String> {
    ensure_private_cache_security_supported(path)?;
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

#[cfg(unix)]
fn ensure_private_cache_security_supported(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_cache_security_supported(path: &Path) -> Result<(), String> {
    Err(format!(
        "native shim cache requires verifiable private owner and ACL enforcement; this platform backend is unavailable for {}",
        path.display()
    ))
}

fn validate_private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let metadata = validate_owned_dir(path)?;
    #[cfg(not(unix))]
    validate_owned_dir(path)?;
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
fn validate_cache_owner(path: &Path, _metadata: &fs::Metadata) -> Result<(), String> {
    Err(format!(
        "native shim cache owner verification is unavailable on this platform: {}",
        path.display()
    ))
}

pub(super) fn open_private_lock(path: &Path) -> Result<fs::File, String> {
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
