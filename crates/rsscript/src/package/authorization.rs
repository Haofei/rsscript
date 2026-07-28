use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use super::{
    NativePluginBuildDependency, PackageLoweringInput, TreeLimits, check_package_dir,
    collect_bounded_regular_files, package_lowering_input,
    package_native_plugin_build_dependencies,
};

#[derive(Debug)]
struct PrivateContentSnapshot {
    root: PathBuf,
    native_abi_path: PathBuf,
}

impl PrivateContentSnapshot {
    fn root(&self) -> &Path {
        &self.root
    }
}

/// A package whose review, lock, dependency graph, and native policy checks
/// succeeded.
///
/// Values can only be created by [`prepare_authorized_package`]. Native build
/// and load code consumes this type instead of accepting an unchecked path.
#[derive(Debug)]
pub struct AuthorizedPackage {
    package_dir: PathBuf,
    lowering_input: PackageLoweringInput,
    native_build_dependencies: Vec<NativePluginBuildDependency>,
    content_snapshot: Option<PrivateContentSnapshot>,
}

impl AuthorizedPackage {
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    /// Return the checked lowering snapshot captured during authorization.
    ///
    /// AOT callers must use this snapshot rather than re-reading the package
    /// path after authorization.
    pub fn lowering_input(&self) -> &PackageLoweringInput {
        &self.lowering_input
    }

    pub(crate) fn native_build_dependencies(&self) -> &[NativePluginBuildDependency] {
        &self.native_build_dependencies
    }

    pub(crate) fn native_snapshot_root(&self) -> Option<&Path> {
        self.content_snapshot
            .as_ref()
            .map(PrivateContentSnapshot::root)
    }

    pub(crate) fn native_abi_path(&self) -> Option<&Path> {
        self.content_snapshot
            .as_ref()
            .map(|snapshot| snapshot.native_abi_path.as_path())
    }
}

/// Review and authorize a package before any native build or dynamic load.
///
/// The returned value also captures the lowering and native dependency inputs
/// used by the loader, so the loader cannot independently rediscover an
/// unchecked package graph.
pub fn prepare_authorized_package(package_dir: &Path) -> Result<AuthorizedPackage, String> {
    let check = check_package_dir(package_dir)?;
    if !check.ok {
        let reasons = if check.reasons.is_empty() {
            "package check did not authorize native execution".to_string()
        } else {
            check.reasons.join("; ")
        };
        return Err(format!(
            "native build/load denied because package review or policy did not authorize execution: {reasons}"
        ));
    }

    let package_dir = package_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize authorized package {}: {error}",
            package_dir.display()
        )
    })?;
    let mut lowering_input = package_lowering_input(&package_dir)?;
    let native_build_dependencies = package_native_plugin_build_dependencies(&package_dir)?;
    let (native_build_dependencies, content_snapshot) =
        snapshot_native_build_inputs(&native_build_dependencies)?;
    for dependency in &mut lowering_input.native_dependencies {
        let snapshotted = native_build_dependencies
            .iter()
            .find(|candidate| candidate.crate_name == dependency.crate_name)
            .ok_or_else(|| {
                format!(
                    "authorized lowering dependency `{}` has no native content snapshot",
                    dependency.crate_name
                )
            })?;
        dependency.path = snapshotted.path.clone();
    }

    Ok(AuthorizedPackage {
        package_dir,
        lowering_input,
        native_build_dependencies,
        content_snapshot,
    })
}

fn snapshot_native_build_inputs(
    dependencies: &[NativePluginBuildDependency],
) -> Result<
    (
        Vec<NativePluginBuildDependency>,
        Option<PrivateContentSnapshot>,
    ),
    String,
> {
    if dependencies.is_empty() {
        return Ok((Vec::new(), None));
    }

    let cache_root = std::env::var_os("RSS_NATIVE_SNAPSHOT_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rss-native-snapshots-v1"));
    let staging_root = cache_root.join("staging");
    let entries_root = cache_root.join("entries");
    let locks_root = cache_root.join("locks");
    for path in [&cache_root, &staging_root, &entries_root, &locks_root] {
        fs::create_dir_all(path)
            .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
        set_private_directory_permissions(path)?;
    }
    let directory = tempfile::Builder::new()
        .prefix("rsscript-authorized-native-")
        .tempdir_in(&staging_root)
        .map_err(|error| format!("failed to create private native snapshot: {error}"))?;
    set_private_directory_permissions(directory.path())?;

    let mut snapshotted = Vec::with_capacity(dependencies.len());
    for (index, dependency) in dependencies.iter().enumerate() {
        let source = Path::new(&dependency.path);
        let reviewed_lock = validate_reviewed_cargo_inputs(source, &dependency.crate_name)?;
        let destination = directory.path().join("native").join(index.to_string());
        snapshot_tree(source, &destination)?;
        if !destination.join("Cargo.lock").is_file() {
            if let Some(reviewed_lock) = reviewed_lock {
                snapshot_file(&reviewed_lock, &destination.join("Cargo.lock"))?;
            } else {
                super::native::prepare_native_cargo_lock(&destination.join("Cargo.toml"))?;
            }
        }
        let mut dependency = dependency.clone();
        dependency.path = destination.display().to_string();
        snapshotted.push(dependency);
    }

    let native_abi_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../native-abi");
    let native_abi_path = directory.path().join("native-abi");
    snapshot_tree(&native_abi_source, &native_abi_path)?;
    let digest = snapshot_tree_digest(directory.path())?;
    let published = entries_root.join(digest);
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(locks_root.join(format!(
            "{}.lock",
            published
                .file_name()
                .and_then(|name| name.to_str())
                .expect("snapshot digest is UTF-8")
        )))
        .map_err(|error| format!("failed to open native snapshot cache lock: {error}"))?;
    lock.lock_exclusive()
        .map_err(|error| format!("failed to lock native snapshot cache entry: {error}"))?;
    if let Ok(metadata) = fs::symlink_metadata(&published)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(format!(
            "authorized native snapshot cache entry must be a real directory: {}",
            published.display()
        ));
    }
    if published.exists() {
        drop(directory);
    } else {
        let staging = directory.keep();
        fs::rename(&staging, &published).map_err(|error| {
            format!(
                "failed to publish authorized native snapshot {}: {error}",
                published.display()
            )
        })?;
        make_tree_read_only(&published)?;
    }

    Ok((
        snapshotted
            .into_iter()
            .enumerate()
            .map(|(index, mut dependency)| {
                dependency.path = published
                    .join("native")
                    .join(index.to_string())
                    .display()
                    .to_string();
                dependency
            })
            .collect(),
        Some(PrivateContentSnapshot {
            root: published.clone(),
            native_abi_path: published.join("native-abi"),
        }),
    ))
}

fn snapshot_tree_digest(root: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"rsscript-authorized-native-snapshot-v1\0");
    let files = collect_bounded_regular_files(
        root,
        TreeLimits::default(),
        "authorized native snapshot digest",
        |_parent, _entry| false,
    )?;
    for file in files {
        let relative = file.path.strip_prefix(root).map_err(|_| {
            format!(
                "native snapshot digest input escaped root: {}",
                file.path.display()
            )
        })?;
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        let mut input = File::open(&file.path)
            .map_err(|error| format!("failed to hash {}: {error}", file.path.display()))?;
        std::io::copy(&mut input, &mut DigestWriter(&mut digest))
            .map_err(|error| format!("failed to hash {}: {error}", file.path.display()))?;
    }
    Ok(hex::encode(digest.finalize()))
}

struct DigestWriter<'a>(&'a mut Sha256);

impl Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_reviewed_cargo_inputs(
    native_root: &Path,
    crate_name: &str,
) -> Result<Option<PathBuf>, String> {
    let lock_path = super::native::reviewed_native_cargo_lock(native_root, crate_name)?;
    let Some(lock_path) = lock_path else {
        return Ok(None);
    };
    let lock = fs::read_to_string(&lock_path).map_err(|error| {
        format!(
            "native build denied: failed to read reviewed Cargo.lock {}: {error}",
            lock_path.display()
        )
    })?;
    let parsed: toml::Value = toml::from_str(&lock).map_err(|error| {
        format!(
            "native build denied: invalid reviewed Cargo.lock {}: {error}",
            lock_path.display()
        )
    })?;
    let uses_registry = parsed
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| package.get("source").and_then(toml::Value::as_str))
        .any(|source| source.starts_with("registry+"));
    if !uses_registry {
        return Ok(Some(lock_path));
    }

    let vendor = native_root.join("vendor");
    let cargo_config = native_root.join(".cargo/config.toml");
    if vendor.exists() != cargo_config.exists() {
        return Err(format!(
            "native build denied: reviewed Cargo vendor directory and `.cargo/config.toml` must be supplied together under {}",
            native_root.display()
        ));
    }
    Ok(Some(lock_path))
}

fn snapshot_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let files = collect_bounded_regular_files(
        source,
        TreeLimits::default(),
        "authorized native snapshot",
        |_parent, entry| {
            matches!(
                entry.file_name().to_str(),
                Some("target" | ".git" | ".DS_Store")
            )
        },
    )?;
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "failed to create native snapshot directory {}: {error}",
            destination.display()
        )
    })?;
    let mut directories = BTreeSet::new();
    for file in files {
        let relative = file.path.strip_prefix(source).map_err(|_| {
            format!(
                "native snapshot source escaped reviewed root: {}",
                file.path.display()
            )
        })?;
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            directories.insert(parent.to_path_buf());
        }
        snapshot_file_bounded(&file.path, &target, file.bytes)?;
    }
    for directory in directories {
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "failed to create native snapshot directory {}: {error}",
                directory.display()
            )
        })?;
    }
    Ok(())
}

fn snapshot_file(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("failed to inspect {}: {error}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "snapshot input must be a regular file, not a symlink: {}",
            source.display()
        ));
    }
    snapshot_file_bounded(source, destination, metadata.len())
}

fn snapshot_file_bounded(
    source: &Path,
    destination: &Path,
    expected_bytes: u64,
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut input = options
        .open(source)
        .map_err(|error| format!("failed to snapshot {}: {error}", source.display()))?;
    let opened = input
        .metadata()
        .map_err(|error| format!("failed to inspect opened {}: {error}", source.display()))?;
    if !opened.is_file() || opened.len() != expected_bytes {
        return Err(format!(
            "native input changed while authorization snapshot was captured: {}",
            source.display()
        ));
    }
    let mut output = File::create(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let copied = std::io::copy(
        &mut Read::by_ref(&mut input).take(expected_bytes.saturating_add(1)),
        &mut output,
    )
    .map_err(|error| format!("failed to snapshot {}: {error}", source.display()))?;
    if copied != expected_bytes {
        return Err(format!(
            "native input changed while authorization snapshot was captured: {}",
            source.display()
        ));
    }
    output
        .flush()
        .map_err(|error| format!("failed to flush {}: {error}", destination.display()))
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("failed to protect {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    Err(format!(
        "native authorization snapshots require verifiable private directory ownership and ACLs; this platform backend is unavailable for {}",
        path.display()
    ))
}

fn make_tree_read_only(root: &Path) -> Result<(), String> {
    let files = collect_bounded_regular_files(
        root,
        TreeLimits::default(),
        "authorized snapshot sealing",
        |_parent, _entry| false,
    )?;
    for file in files {
        let mut permissions = fs::metadata(&file.path)
            .map_err(|error| format!("failed to inspect {}: {error}", file.path.display()))?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file.path, permissions)
            .map_err(|error| format!("failed to seal {}: {error}", file.path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut directories = Vec::new();
        collect_directories(root, &mut directories)?;
        for directory in directories.into_iter().rev() {
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
                .map_err(|error| format!("failed to seal {}: {error}", directory.display()))?;
        }
    }
    Ok(())
}

fn collect_directories(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    output.push(path.to_path_buf());
    for entry in
        fs::read_dir(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?
    {
        let entry = entry.map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?
            .is_dir()
        {
            collect_directories(&entry.path(), output)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::native_plugin::load_authorized_package_native_bindings;
    use crate::package::{format_package_lock_toml, lock_package_dir};

    fn pure_package_fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rss-authorized-package-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory");
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("fixture manifest");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("fixture source");
        root
    }

    fn add_native_dependency(root: &Path) {
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[native.rust]\nenabled = true\npath = \"native/rust\"\ncrate = \"authorized_test_native\"\nbuild_scripts = \"forbid\"\nproc_macros = \"forbid\"\nunsafe = \"forbid\"\n",
        )
        .expect("fixture native manifest declaration");
        fs::create_dir_all(root.join("native/rust/src")).expect("fixture native source directory");
        fs::write(
            root.join("native/rust/Cargo.toml"),
            "[package]\nname = \"authorized_test_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture native Cargo manifest");
        fs::write(root.join("native/rust/src/lib.rs"), "pub fn unused() {}\n")
            .expect("fixture native source");
        fs::write(
            root.join("native/rust/Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"authorized_test_native\"\nversion = \"0.1.0\"\n",
        )
        .expect("fixture reviewed Cargo lock");
    }

    #[test]
    fn successful_check_is_the_only_authorized_package_constructor() {
        let root = pure_package_fixture();
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let package = prepare_authorized_package(&root).expect("checked fixture should authorize");
        assert_eq!(
            package.package_dir(),
            root.canonicalize().expect("canonical fixture")
        );
        assert_eq!(package.lowering_input().package.name, "authorized-test");
        assert!(package.native_build_dependencies().is_empty());
        assert!(
            load_authorized_package_native_bindings(&package)
                .expect("pure authorized package should load without native work")
                .is_empty()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_check_cannot_produce_an_authorized_package() {
        let root = pure_package_fixture();

        let error =
            prepare_authorized_package(&root).expect_err("missing lock must prevent authorization");
        assert!(error.contains("native build/load denied"), "{error}");
        assert!(error.contains("rsspkg.lock missing"), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn successful_native_authorization_captures_checked_build_inputs() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let package =
            prepare_authorized_package(&root).expect("checked native fixture should authorize");
        assert_eq!(package.lowering_input().native_dependencies.len(), 1);
        assert_eq!(package.native_build_dependencies().len(), 1);
        assert_eq!(
            package.native_build_dependencies()[0].crate_name,
            "authorized_test_native"
        );
        assert_ne!(
            Path::new(&package.native_build_dependencies()[0].path),
            root.join("native/rust")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn authorized_native_snapshot_is_stable_after_source_mutation() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let package =
            prepare_authorized_package(&root).expect("checked native fixture should authorize");
        fs::write(
            root.join("native/rust/src/lib.rs"),
            "compile_error!(\"mutated after authorization\");\n",
        )
        .expect("original source mutation");

        let snapshotted_source =
            Path::new(&package.native_build_dependencies()[0].path).join("src/lib.rs");
        assert_eq!(
            fs::read_to_string(snapshotted_source).expect("private snapshot source"),
            "pub fn unused() {}\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn cloned_aot_lowering_input_keeps_stable_snapshotted_native_paths() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let package =
            prepare_authorized_package(&root).expect("checked native fixture should authorize");
        let aot_input = package.lowering_input().clone();
        let loader_path = package.native_build_dependencies()[0].path.clone();
        assert_eq!(aot_input.native_dependencies[0].path, loader_path);
        drop(package);

        fs::write(
            root.join("native/rust/src/lib.rs"),
            "compile_error!(\"AOT must not read this mutation\");\n",
        )
        .expect("original source mutation");
        let aot_source = Path::new(&aot_input.native_dependencies[0].path).join("src/lib.rs");
        assert_eq!(
            fs::read_to_string(aot_source).expect("stable AOT snapshot source"),
            "pub fn unused() {}\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn identical_authorizations_reuse_content_addressed_snapshot_paths() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let first = prepare_authorized_package(&root).expect("first authorization");
        let second = prepare_authorized_package(&root).expect("second authorization");
        assert_eq!(
            first.native_build_dependencies()[0].path,
            second.native_build_dependencies()[0].path
        );
        assert_eq!(
            first.lowering_input().native_dependencies[0].path,
            second.lowering_input().native_dependencies[0].path
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(unix))]
    #[test]
    fn native_authorization_fails_closed_without_private_acl_enforcement() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let error = prepare_authorized_package(&root)
            .expect_err("native authorization must require private cache enforcement");
        assert!(
            error.contains("private owner and ACL enforcement")
                || error.contains("platform backend is unavailable"),
            "{error}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_authorization_requires_reviewed_cargo_lock() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        fs::remove_file(root.join("native/rust/Cargo.lock")).expect("remove fixture Cargo lock");
        fs::write(
            root.join("native/rust/Cargo.toml"),
            "[package]\nname = \"authorized_test_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .expect("fixture unlocked registry dependency");
        let lock = lock_package_dir(&root).expect("fixture RSS lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let error = prepare_authorized_package(&root)
            .expect_err("native package without Cargo.lock must fail closed");
        assert!(
            error.contains("cargo metadata failed") || error.contains("Cargo.lock"),
            "{error}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
