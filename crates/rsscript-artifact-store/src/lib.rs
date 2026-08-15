use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use fs2::FileExt;
use rsscript_project::{ProjectTreeLimits, collect_project_regular_files};
use sha2::{Digest, Sha256};

const MUTATION_LOCK: &str = ".rsscript-artifacts.lock";
const ARTIFACT_READ_MAX_BYTES: u64 = 16 * 1024 * 1024;

fn canonical_checked_root(path: &Path, operation: &str) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if is_link_like(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{operation} requires a real directory and rejects symlinks or reparse points: {}",
            path.display()
        ));
    }
    fs::canonicalize(path)
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))
}

fn is_link_like(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

#[cfg(not(unix))]
fn open_regular_file_no_follow(
    path: &Path,
    operation: &str,
) -> Result<(File, fs::Metadata), String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        format!(
            "failed to open {} without following links: {error}",
            path.display()
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect opened {}: {error}", path.display()))?;
    if !metadata.is_file() || is_link_like(&metadata) {
        return Err(format!(
            "{operation} requires a regular file and rejects symlinks or reparse points: {}",
            path.display()
        ));
    }
    Ok((file, metadata))
}

/// A locked, confined writer for artifacts owned by one package.
///
/// The lock serializes cooperating processes for the lifetime of this value.
/// On Unix, file publication traverses directories by descriptor and opens
/// every component with `O_NOFOLLOW`. Other platforms retain the same path and
/// reparse-point checks, but the standard library cannot make traversal fully
/// race-free; callers needing a hostile multi-user boundary should additionally
/// isolate the package workspace at the OS level.
pub struct ArtifactStore {
    root: PathBuf,
    _lock: File,
    #[cfg(unix)]
    root_dir: File,
}

impl ArtifactStore {
    /// Create a package-owned artifact root when it does not already exist,
    /// then open the confined locked writer.
    pub fn create(package_root: &Path) -> Result<Self, String> {
        fs::create_dir_all(package_root).map_err(|error| {
            format!(
                "failed to create package artifact root {}: {error}",
                package_root.display()
            )
        })?;
        Self::open(package_root)
    }

    pub fn open(package_root: &Path) -> Result<Self, String> {
        let root = canonical_checked_root(package_root, "package artifact store")?;
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
        if !metadata.is_dir() || is_link_like(&metadata) {
            return Err(format!(
                "package artifact root must be a real directory: {}",
                root.display()
            ));
        }

        #[cfg(unix)]
        let root_dir = open_unix_directory(&root, "package artifact root")?;

        #[cfg(unix)]
        let lock = open_unix_child_file(
            &root_dir,
            Path::new(MUTATION_LOCK),
            true,
            "package mutation lock",
        )?;
        #[cfg(not(unix))]
        let lock = open_portable_lock(&root)?;

        lock.lock_exclusive()
            .map_err(|error| format!("failed to lock {}: {error}", root.display()))?;

        Ok(Self {
            root,
            _lock: lock,
            #[cfg(unix)]
            root_dir,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: impl AsRef<Path>) -> Result<PathBuf, String> {
        let relative = checked_relative(relative.as_ref(), "package artifact")?;
        Ok(self.root.join(relative))
    }

    pub fn read(&self, relative: impl AsRef<Path>, label: &str) -> Result<Vec<u8>, String> {
        self.read_bounded(relative, label, ARTIFACT_READ_MAX_BYTES)
    }

    pub fn read_bounded(
        &self,
        relative: impl AsRef<Path>,
        label: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, String> {
        let relative = checked_relative(relative.as_ref(), label)?;
        #[cfg(unix)]
        {
            use std::io::Read;

            let file = open_unix_child_file(&self.root_dir, relative, false, label)?;
            let metadata = file
                .metadata()
                .map_err(|error| format!("failed to inspect {label}: {error}"))?;
            if metadata.len() > max_bytes {
                return Err(format!("{label} exceeded byte limit of {max_bytes}"));
            }
            let mut contents = Vec::with_capacity(metadata.len() as usize);
            file.take(max_bytes + 1)
                .read_to_end(&mut contents)
                .map_err(|error| format!("failed to read {label}: {error}"))?;
            if contents.len() as u64 > max_bytes {
                return Err(format!("{label} exceeded byte limit of {max_bytes}"));
            }
            Ok(contents)
        }
        #[cfg(not(unix))]
        {
            use std::io::Read;

            let path = self.checked_portable_path(relative, label, true)?;
            let (file, metadata) = open_regular_file_no_follow(&path, label)?;
            if metadata.len() > max_bytes {
                return Err(format!("{label} exceeded byte limit of {max_bytes}"));
            }
            let mut contents = Vec::with_capacity(metadata.len() as usize);
            file.take(max_bytes + 1)
                .read_to_end(&mut contents)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            if contents.len() as u64 > max_bytes {
                return Err(format!("{label} exceeded byte limit of {max_bytes}"));
            }
            Ok(contents)
        }
    }

    pub fn create_directory_all(
        &self,
        relative: impl AsRef<Path>,
        label: &str,
    ) -> Result<(), String> {
        let relative = checked_relative(relative.as_ref(), label)?;
        #[cfg(unix)]
        {
            ensure_unix_directory(&self.root_dir, relative, label).map(|_| ())
        }
        #[cfg(not(unix))]
        {
            let mut current = self.root.clone();
            for component in relative.components() {
                let Component::Normal(component) = component else {
                    return Err(format!("{label} directory is not confined"));
                };
                current.push(component);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.is_dir() && !is_link_like(&metadata) => {}
                    Ok(_) => {
                        return Err(format!(
                            "{label} must be a real directory: {}",
                            current.display()
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        fs::create_dir(&current).map_err(|error| {
                            format!("failed to create {}: {error}", current.display())
                        })?;
                    }
                    Err(error) => {
                        return Err(format!("failed to inspect {}: {error}", current.display()));
                    }
                }
            }
            sync_directory(&current)
        }
    }

    pub fn write_atomic(
        &self,
        relative: impl AsRef<Path>,
        contents: &[u8],
        label: &str,
    ) -> Result<(), String> {
        let relative = checked_relative(relative.as_ref(), label)?;
        #[cfg(unix)]
        {
            self.write_atomic_unix(relative, contents, label)
        }
        #[cfg(not(unix))]
        {
            self.write_atomic_portable(relative, contents, label)
        }
    }

    /// Atomically publish a file only when its bytes differ from the existing
    /// regular file. The boolean reports whether a write occurred.
    pub fn write_atomic_if_changed(
        &self,
        relative: impl AsRef<Path>,
        contents: &[u8],
        label: &str,
    ) -> Result<bool, String> {
        let relative = checked_relative(relative.as_ref(), label)?;
        let destination = self.root.join(relative);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if is_link_like(&metadata) || !metadata.is_file() => {
                return Err(format!(
                    "{label} destination must be a regular file, not a symlink or reparse point: {}",
                    destination.display()
                ));
            }
            Ok(_) => {
                if self.read(relative, label)? == contents {
                    return Ok(false);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to inspect {label} destination {}: {error}",
                    destination.display()
                ));
            }
        }
        self.write_atomic(relative, contents, label)?;
        Ok(true)
    }

    /// Remove an optional regular artifact without following a symlink at the
    /// destination. Missing files are treated as already removed.
    pub fn remove_regular_file(
        &self,
        relative: impl AsRef<Path>,
        label: &str,
    ) -> Result<(), String> {
        let relative = checked_relative(relative.as_ref(), label)?;
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, FileType, unlinkat};

            let (parent, name) = open_unix_parent(&self.root_dir, relative, label)?;
            match rustix::fs::statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile => {
                    return Err(format!(
                        "{label} destination must be a regular file, not a symlink"
                    ));
                }
                Ok(_) => {
                    unlinkat(&parent, &name, AtFlags::empty())
                        .map_err(|error| format!("failed to remove {label}: {error}"))?;
                    rustix::fs::fsync(&parent)
                        .map_err(|error| format!("failed to sync {label} directory: {error}"))?;
                }
                Err(error) if error == rustix::io::Errno::NOENT => {}
                Err(error) => return Err(format!("failed to inspect {label}: {error}")),
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let destination = self.checked_portable_path(relative, label, true)?;
            match fs::symlink_metadata(&destination) {
                Ok(metadata) if is_link_like(&metadata) || !metadata.is_file() => {
                    return Err(format!(
                        "{label} destination must be a regular file, not a symlink or reparse point: {}",
                        destination.display()
                    ));
                }
                Ok(_) => fs::remove_file(&destination)
                    .map_err(|error| format!("failed to remove {label}: {error}"))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("failed to inspect {label}: {error}")),
            }
            Ok(())
        }
    }

    #[cfg(unix)]
    fn write_atomic_unix(
        &self,
        relative: &Path,
        contents: &[u8],
        label: &str,
    ) -> Result<(), String> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, renameat, statat};

        let (parent, name) = open_unix_parent(&self.root_dir, relative, label)?;
        match statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile => {
                return Err(format!(
                    "{label} destination must be a regular file, not a symlink"
                ));
            }
            Ok(_) => {}
            Err(error) if error == rustix::io::Errno::NOENT => {}
            Err(error) => return Err(format!("failed to inspect {label}: {error}")),
        }

        let temporary_name = format!(
            ".{}.{}.{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let temporary = rustix::fs::openat(
            &parent,
            temporary_name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| format!("failed to create temporary {label}: {error}"))?;
        let mut temporary_file = File::from(temporary);
        let result = (|| {
            temporary_file
                .write_all(contents)
                .map_err(|error| format!("failed to write {label}: {error}"))?;
            temporary_file
                .sync_all()
                .map_err(|error| format!("failed to sync {label}: {error}"))?;
            renameat(&parent, temporary_name.as_str(), &parent, &name)
                .map_err(|error| format!("failed to atomically publish {label}: {error}"))?;
            rustix::fs::fsync(&parent)
                .map_err(|error| format!("failed to sync {label} directory: {error}"))
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&parent, temporary_name.as_str(), AtFlags::empty());
        }
        result
    }

    #[cfg(not(unix))]
    fn checked_portable_path(
        &self,
        relative: &Path,
        label: &str,
        allow_missing_leaf: bool,
    ) -> Result<PathBuf, String> {
        let mut current = self.root.clone();
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(component) = component else {
                return Err(format!("{label} path is not confined"));
            };
            current.push(component);
            let leaf = components.peek().is_none();
            match fs::symlink_metadata(&current) {
                Ok(metadata) if is_link_like(&metadata) => {
                    return Err(format!(
                        "{label} rejects symlinks or reparse points: {}",
                        current.display()
                    ));
                }
                Ok(metadata) if !leaf && !metadata.is_dir() => {
                    return Err(format!(
                        "{label} parent must be a real directory: {}",
                        current.display()
                    ));
                }
                Ok(metadata) if leaf && !metadata.is_file() => {
                    return Err(format!(
                        "{label} destination must be a regular file: {}",
                        current.display()
                    ));
                }
                Ok(_) => {}
                Err(error)
                    if leaf
                        && allow_missing_leaf
                        && error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "failed to inspect confined {label} path {}: {error}",
                        current.display()
                    ));
                }
            }
        }
        Ok(current)
    }

    #[cfg(not(unix))]
    fn write_atomic_portable(
        &self,
        relative: &Path,
        contents: &[u8],
        label: &str,
    ) -> Result<(), String> {
        let destination = self.checked_portable_path(relative, label, true)?;
        let parent = destination
            .parent()
            .ok_or_else(|| format!("{label} has no parent"))?;
        let temporary = parent.join(format!(
            ".{}.{}.{}.tmp",
            destination
                .file_name()
                .expect("checked destination has a file name")
                .to_string_lossy(),
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| format!("failed to create temporary {label}: {error}"))?;
            file.write_all(contents)
                .map_err(|error| format!("failed to write {label}: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("failed to sync {label}: {error}"))?;
            replace_portable_file(&temporary, &destination, label)?;
            sync_directory(parent)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

/// The generated files that comprise one experimental Rust AOT package. The
/// compiler owns their contents; this adapter owns safe publication.
#[derive(Debug, Clone, Copy)]
pub struct GeneratedRustPackageFiles<'a> {
    pub cargo_toml: &'a str,
    pub lib_rs: &'a str,
    pub main_rs: Option<&'a str>,
    pub source_map_json: &'a str,
}

/// Persist generated Rust package files with confined, atomic writes. Reused
/// files preserve their modification time so downstream Cargo caches do not
/// rebuild solely because RSScript ran again.
pub fn write_generated_rust_package(
    out_dir: &Path,
    package: GeneratedRustPackageFiles<'_>,
) -> Result<(), String> {
    let store = ArtifactStore::create(out_dir)?;
    store.create_directory_all("src", "generated Rust source directory")?;
    store.write_atomic_if_changed(
        "Cargo.toml",
        package.cargo_toml.as_bytes(),
        "generated Cargo.toml",
    )?;
    store.write_atomic_if_changed(
        "src/lib.rs",
        package.lib_rs.as_bytes(),
        "generated Rust library",
    )?;
    if let Some(main_rs) = package.main_rs {
        store.write_atomic_if_changed("src/main.rs", main_rs.as_bytes(), "generated Rust main")?;
    } else {
        store.remove_regular_file("src/main.rs", "generated Rust main")?;
    }
    store.write_atomic_if_changed(
        "rsscript-source-map.json",
        package.source_map_json.as_bytes(),
        "generated Rust source map",
    )?;
    Ok(())
}

/// Atomically write a generated `Cargo.lock` beside a captured `Cargo.toml`.
/// Native snapshots are private staging trees, so this deliberately avoids the
/// package mutation lock file used by [`ArtifactStore`]; adding that lock file
/// would change the content-addressed native snapshot itself.
pub fn write_generated_cargo_lock(cargo_toml: &Path, contents: &str) -> Result<(), String> {
    let package_dir = cargo_toml.parent().ok_or_else(|| {
        format!(
            "generated Cargo.lock requires a Cargo.toml parent directory: {}",
            cargo_toml.display()
        )
    })?;
    let directory_metadata = fs::symlink_metadata(package_dir).map_err(|error| {
        format!(
            "failed to inspect generated Cargo.lock directory {}: {error}",
            package_dir.display()
        )
    })?;
    if is_link_like(&directory_metadata) || !directory_metadata.is_dir() {
        return Err(format!(
            "generated Cargo.lock directory must be a real directory: {}",
            package_dir.display()
        ));
    }
    let destination = package_dir.join("Cargo.lock");
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if is_link_like(&metadata) || !metadata.is_file() => {
            return Err(format!(
                "generated Cargo.lock destination must be a regular file, not a symlink or reparse point: {}",
                destination.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to inspect generated Cargo.lock {}: {error}",
                destination.display()
            ));
        }
    }

    let temporary = package_dir.join(format!(
        ".Cargo.lock.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                format!("failed to create generated Cargo.lock temporary file: {error}")
            })?;
        file.write_all(contents.as_bytes())
            .map_err(|error| format!("failed to write generated Cargo.lock: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync generated Cargo.lock: {error}"))?;
        #[cfg(unix)]
        fs::rename(&temporary, &destination)
            .map_err(|error| format!("failed to publish generated Cargo.lock: {error}"))?;
        #[cfg(not(unix))]
        replace_portable_file(&temporary, &destination, "generated Cargo.lock")?;
        #[cfg(unix)]
        File::open(package_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                format!(
                    "failed to sync generated Cargo.lock directory {}: {error}",
                    package_dir.display()
                )
            })?;
        #[cfg(not(unix))]
        sync_directory(package_dir)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Atomically replace a regular artifact below `package_root` without following
/// symlinks in its parent path or at the destination.
///
/// This convenience adapter preserves the historical package-tooling call
/// shape while keeping all persistence policy outside the compiler crate.
pub fn write_package_artifact_atomic(
    package_root: &Path,
    destination: &Path,
    contents: &[u8],
    label: &str,
) -> Result<(), String> {
    let store = ArtifactStore::open(package_root)?;
    let relative = destination.strip_prefix(package_root).map_err(|_| {
        format!(
            "{label} destination escapes package root: {}",
            destination.display()
        )
    })?;
    store.write_atomic(relative, contents, label)
}

/// Owns the private staging and content-addressed publication lifecycle for
/// reviewed native build snapshots. Callers decide which inputs are approved;
/// this adapter ensures that the resulting tree is staged, sealed, and reused
/// only after a digest check.
pub struct NativeSnapshotStore {
    staging_root: PathBuf,
    entries_root: PathBuf,
    locks_root: PathBuf,
}

/// A private, unpublished native snapshot tree.
pub struct NativeSnapshotStaging {
    directory: tempfile::TempDir,
}

/// A sealed, content-addressed native snapshot that may safely be reused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedNativeSnapshot {
    digest: String,
    path: PathBuf,
}

impl NativeSnapshotStore {
    /// Open the host-selected default native snapshot cache.
    pub fn open_default() -> Result<Self, String> {
        let root = std::env::var_os("RSS_NATIVE_SNAPSHOT_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("rss-native-snapshots-v2"));
        Self::open(&root)
    }

    /// Open or create a private content-addressed native snapshot cache.
    pub fn open(root: &Path) -> Result<Self, String> {
        ensure_private_directory(root, "native snapshot cache root")?;
        let staging_root = root.join("staging");
        let entries_root = root.join("entries");
        let locks_root = root.join("locks");
        for (path, label) in [
            (&staging_root, "native snapshot staging directory"),
            (&entries_root, "native snapshot entry directory"),
            (&locks_root, "native snapshot lock directory"),
        ] {
            ensure_private_directory(path, label)?;
        }
        Ok(Self {
            staging_root,
            entries_root,
            locks_root,
        })
    }

    /// Create a private staging tree which is deleted unless published.
    pub fn stage(&self) -> Result<NativeSnapshotStaging, String> {
        let directory = tempfile::Builder::new()
            .prefix("rsscript-authorized-native-")
            .tempdir_in(&self.staging_root)
            .map_err(|error| format!("failed to create private native snapshot: {error}"))?;
        set_private_directory_permissions(directory.path())?;
        Ok(NativeSnapshotStaging { directory })
    }

    /// Publish a staged tree under its digest, or reuse an existing entry only
    /// after revalidating it. The domain identifies the caller's snapshot
    /// protocol so digest values cannot cross protocol boundaries.
    pub fn publish(
        &self,
        staging: NativeSnapshotStaging,
        limits: ProjectTreeLimits,
        label: &str,
        domain: &[u8],
    ) -> Result<PublishedNativeSnapshot, String> {
        let digest = regular_tree_digest(staging.path(), limits, label, domain)?;
        let published = self.entries_root.join(&digest);
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.locks_root.join(format!("{digest}.lock")))
            .map_err(|error| format!("failed to open native snapshot cache lock: {error}"))?;
        lock.lock_exclusive()
            .map_err(|error| format!("failed to lock native snapshot cache entry: {error}"))?;

        if let Ok(metadata) = fs::symlink_metadata(&published)
            && (is_link_like(&metadata) || !metadata.is_dir())
        {
            return Err(format!(
                "native snapshot cache entry must be a real directory: {}",
                published.display()
            ));
        }
        if published.exists() {
            let published_digest = regular_tree_digest(&published, limits, label, domain)?;
            if published_digest != digest {
                return Err(format!(
                    "native snapshot cache entry failed integrity verification: {}",
                    published.display()
                ));
            }
            drop(staging);
        } else {
            let staged_path = staging.directory.keep();
            fs::rename(&staged_path, &published).map_err(|error| {
                format!(
                    "failed to publish native snapshot {}: {error}",
                    published.display()
                )
            })?;
            seal_regular_tree_read_only(&published, limits, label)?;
        }

        Ok(PublishedNativeSnapshot {
            digest,
            path: published,
        })
    }
}

impl NativeSnapshotStaging {
    /// Path available to the caller while the snapshot is still private.
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}

impl PublishedNativeSnapshot {
    /// Content-addressed identity of the published tree.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Sealed filesystem path of the published tree.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Copy a bounded, no-follow regular-file tree into a private Artifact staging
/// directory. The caller owns the semantic decision to snapshot; this adapter
/// owns filesystem traversal, byte accounting, and safe file copying.
pub fn snapshot_regular_tree(
    source: &Path,
    destination: &Path,
    limits: ProjectTreeLimits,
    label: &str,
    skip: impl Fn(&Path, &str) -> bool,
) -> Result<(), String> {
    let files = collect_project_regular_files(source, limits, label, skip)?;
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "failed to create snapshot directory {}: {error}",
            destination.display()
        )
    })?;
    for file in files {
        let relative = file.path.strip_prefix(source).map_err(|_| {
            format!(
                "{label} source escaped its root {}: {}",
                source.display(),
                file.path.display()
            )
        })?;
        let target = destination.join(relative);
        snapshot_regular_file_bounded(&file.path, &target, file.bytes, label)?;
    }
    Ok(())
}

/// Copy one regular file into a private Artifact staging directory without
/// following source links.
pub fn snapshot_regular_file(source: &Path, destination: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("failed to inspect {}: {error}", source.display()))?;
    if is_link_like(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{label} must be a regular file, not a symlink or reparse point: {}",
            source.display()
        ));
    }
    snapshot_regular_file_bounded(source, destination, metadata.len(), label)
}

/// Return a deterministic digest for a bounded regular-file tree. `domain`
/// prevents reuse across distinct snapshot protocols.
pub fn regular_tree_digest(
    root: &Path,
    limits: ProjectTreeLimits,
    label: &str,
    domain: &[u8],
) -> Result<String, String> {
    let files = collect_project_regular_files(root, limits, label, |_, _| false)?;
    let mut digest = Sha256::new();
    digest.update(domain);
    for file in files {
        let relative = file.path.strip_prefix(root).map_err(|_| {
            format!(
                "{label} input escaped root {}: {}",
                root.display(),
                file.path.display()
            )
        })?;
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        let mut input = open_snapshot_input(&file.path, file.bytes, label)?;
        std::io::copy(&mut input, &mut DigestWriter(&mut digest))
            .map_err(|error| format!("failed to hash {}: {error}", file.path.display()))?;
    }
    Ok(hex::encode(digest.finalize()))
}

/// Seal a private snapshot tree read-only after it has been atomically
/// published. This is an adapter responsibility, not compiler policy.
pub fn seal_regular_tree_read_only(
    root: &Path,
    limits: ProjectTreeLimits,
    label: &str,
) -> Result<(), String> {
    let files = collect_project_regular_files(root, limits, label, |_, _| false)?;
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

fn snapshot_regular_file_bounded(
    source: &Path,
    destination: &Path,
    expected_bytes: u64,
    label: &str,
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let mut input = open_snapshot_input(source, expected_bytes, label)?;
    let mut output = File::create(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let copied = std::io::copy(
        &mut Read::by_ref(&mut input).take(expected_bytes.saturating_add(1)),
        &mut output,
    )
    .map_err(|error| format!("failed to snapshot {}: {error}", source.display()))?;
    if copied != expected_bytes {
        return Err(format!(
            "{label} changed while content snapshot was captured: {}",
            source.display()
        ));
    }
    output
        .flush()
        .map_err(|error| format!("failed to flush {}: {error}", destination.display()))
}

fn open_snapshot_input(source: &Path, expected_bytes: u64, label: &str) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let input = options
        .open(source)
        .map_err(|error| format!("failed to snapshot {}: {error}", source.display()))?;
    let opened = input
        .metadata()
        .map_err(|error| format!("failed to inspect opened {}: {error}", source.display()))?;
    if !opened.is_file() || opened.len() != expected_bytes || is_link_like(&opened) {
        return Err(format!(
            "{label} changed while content snapshot was captured: {}",
            source.display()
        ));
    }
    Ok(input)
}

fn ensure_private_directory(path: &Path, label: &str) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("failed to create {label} {}: {error}", path.display()))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label} {}: {error}", path.display()))?;
    if is_link_like(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{label} must be a real directory, not a symlink or reparse point: {}",
            path.display()
        ));
    }
    set_private_directory_permissions(path)
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
        "native snapshot publication requires verifiable private directory ownership and ACLs; this platform backend is unavailable for {}",
        path.display()
    ))
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

#[cfg(unix)]
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

/// Publish a staged regular file on platforms without descriptor-relative
/// rename support in the standard library.
///
/// Windows `std::fs::rename` cannot replace an existing file. The fallback
/// therefore revalidates and removes an existing regular, non-reparse-point
/// destination immediately before rename. The store lock serializes RSScript
/// writers, but—as documented on `ArtifactStore`—a hostile external process can
/// still race this portable path and requires OS-level workspace isolation.
#[cfg(not(unix))]
fn replace_portable_file(source: &Path, destination: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if !metadata.is_file() || is_link_like(&metadata) => {
            return Err(format!(
                "{label} destination must be a regular file, not a symlink or reparse point"
            ));
        }
        Ok(_) => {
            #[cfg(windows)]
            fs::remove_file(destination).map_err(|error| {
                format!("failed to replace existing {label} destination: {error}")
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("failed to inspect {label} destination: {error}")),
    }
    fs::rename(source, destination)
        .map_err(|error| format!("failed to publish staged {label}: {error}"))
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<(), String> {
    // `std::fs::File::open` cannot open a directory for `sync_all` on Windows.
    // The staged file itself is synced before publication; directory durability
    // across power loss requires a platform adapter outside this safe Core.
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed to sync directory {}: {error}", path.display()))
}

fn checked_relative<'a>(relative: &'a Path, label: &str) -> Result<&'a Path, String> {
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "{label} path must be a confined non-empty relative path: {}",
            relative.display()
        ));
    }
    Ok(relative)
}

#[cfg(unix)]
fn open_unix_directory(path: &Path, label: &str) -> Result<File, String> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| format!("failed to open {label} {}: {error}", path.display()))?;
    Ok(File::from(fd))
}

#[cfg(unix)]
fn open_unix_parent(root: &File, relative: &Path, label: &str) -> Result<(File, PathBuf), String> {
    use rustix::fs::{Mode, OFlags};

    let name = relative
        .file_name()
        .ok_or_else(|| format!("{label} has no file name"))?
        .into();
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = File::from(
        rustix::io::dup(root)
            .map_err(|error| format!("failed to duplicate package root handle: {error}"))?,
    );
    for component in parent.components() {
        let Component::Normal(component) = component else {
            return Err(format!("{label} parent is not confined"));
        };
        let fd = rustix::fs::openat(
            &current,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            format!(
                "failed to open confined {label} directory {}: {error}",
                component.to_string_lossy()
            )
        })?;
        current = File::from(fd);
    }
    Ok((current, name))
}

#[cfg(unix)]
fn ensure_unix_directory(root: &File, relative: &Path, label: &str) -> Result<File, String> {
    use rustix::fs::{Mode, OFlags};

    let mut current = File::from(
        rustix::io::dup(root)
            .map_err(|error| format!("failed to duplicate package root handle: {error}"))?,
    );
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!("{label} directory is not confined"));
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = match rustix::fs::openat(&current, component, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(error) if error == rustix::io::Errno::NOENT => {
                match rustix::fs::mkdirat(&current, component, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                {
                    Ok(()) => {}
                    Err(error) if error == rustix::io::Errno::EXIST => {}
                    Err(error) => {
                        return Err(format!(
                            "failed to create confined {label} directory {}: {error}",
                            component.to_string_lossy()
                        ));
                    }
                }
                rustix::fs::openat(&current, component, flags, Mode::empty()).map_err(|error| {
                    format!(
                        "failed to open confined {label} directory {}: {error}",
                        component.to_string_lossy()
                    )
                })?
            }
            Err(error) => {
                return Err(format!(
                    "failed to open confined {label} directory {}: {error}",
                    component.to_string_lossy()
                ));
            }
        };
        rustix::fs::fsync(&current)
            .map_err(|error| format!("failed to sync parent {label} directory: {error}"))?;
        current = File::from(fd);
    }
    Ok(current)
}

#[cfg(unix)]
fn open_unix_child_file(
    root: &File,
    relative: &Path,
    create: bool,
    label: &str,
) -> Result<File, String> {
    use rustix::fs::{Mode, OFlags};

    let (parent, name) = open_unix_parent(root, relative, label)?;
    let read_flags = if create {
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC
    } else {
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    };
    let fd = if create {
        loop {
            match rustix::fs::openat(&parent, &name, read_flags, Mode::empty()) {
                Ok(fd) => break fd,
                Err(error) if error == rustix::io::Errno::NOENT => {
                    let create_flags =
                        read_flags | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW;
                    match rustix::fs::openat(&parent, &name, create_flags, Mode::RUSR | Mode::WUSR)
                    {
                        Ok(fd) => break fd,
                        Err(error) if error == rustix::io::Errno::EXIST => {
                            std::thread::yield_now();
                        }
                        Err(error) => return Err(format!("failed to create {label}: {error}")),
                    }
                }
                Err(error) => return Err(format!("failed to open {label}: {error}")),
            }
        }
    } else {
        rustix::fs::openat(&parent, &name, read_flags, Mode::empty())
            .map_err(|error| format!("failed to open {label}: {error}"))?
    };
    let file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{label} must be a regular file"));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_portable_lock(root: &Path) -> Result<File, String> {
    let path = root.join(MUTATION_LOCK);
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (is_link_like(&metadata) || !metadata.is_file())
    {
        return Err(format!(
            "package mutation lock must be a regular file: {}",
            path.display()
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(&path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || is_link_like(&metadata) {
        return Err(format!(
            "package mutation lock must be a regular file: {}",
            path.display()
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsscript-artifact-store-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn rejects_parent_directory_escape() {
        let root = test_dir("escape");
        fs::create_dir_all(&root).expect("fixture root");
        let store = ArtifactStore::open(&root).expect("artifact store");

        let error = store
            .write_atomic("../outside", b"bad", "escaped artifact")
            .expect_err("parent traversal must fail");

        assert!(error.contains("confined"), "{error}");
        drop(store);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn bounded_read_rejects_oversized_artifact() {
        let root = test_dir("bounded-read");
        fs::create_dir_all(&root).expect("fixture root");
        fs::write(root.join("artifact"), b"12345").expect("fixture artifact");
        let store = ArtifactStore::open(&root).expect("artifact store");

        let error = store
            .read_bounded("artifact", "test artifact", 4)
            .expect_err("oversized artifact must fail");

        assert!(error.contains("byte limit of 4"), "{error}");
        drop(store);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_parent_without_touching_outside() {
        use std::os::unix::fs::symlink;

        let root = test_dir("parent-link");
        let outside = test_dir("parent-link-outside");
        fs::create_dir_all(&root).expect("fixture root");
        fs::create_dir_all(&outside).expect("outside root");
        symlink(&outside, root.join("linked")).expect("parent symlink");
        let store = ArtifactStore::open(&root).expect("artifact store");

        let error = store
            .write_atomic("linked/file", b"bad", "linked artifact")
            .expect_err("symlink parent must fail");

        assert!(error.contains("confined"), "{error}");
        assert!(!outside.join("file").exists());
        drop(store);
        fs::remove_dir_all(root).expect("fixture cleanup");
        fs::remove_dir_all(outside).expect("outside cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_while_creating_artifact_directories() {
        use std::os::unix::fs::symlink;

        let root = test_dir("directory-link");
        let outside = test_dir("directory-link-outside");
        fs::create_dir_all(&root).expect("fixture root");
        fs::create_dir_all(&outside).expect("outside root");
        symlink(&outside, root.join("review")).expect("review symlink");
        let store = ArtifactStore::open(&root).expect("artifact store");

        let error = store
            .create_directory_all("review/evidence", "review artifact directory")
            .expect_err("symlink directory must fail");

        assert!(error.contains("confined"), "{error}");
        assert!(!outside.join("evidence").exists());
        drop(store);
        fs::remove_dir_all(root).expect("fixture cleanup");
        fs::remove_dir_all(outside).expect("outside cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_destination_without_touching_outside() {
        use std::os::unix::fs::symlink;

        let root = test_dir("destination-link");
        let outside = test_dir("destination-link-outside");
        fs::create_dir_all(&root).expect("fixture root");
        fs::write(&outside, b"outside").expect("outside file");
        symlink(&outside, root.join("rsspkg.lock")).expect("destination symlink");
        let store = ArtifactStore::open(&root).expect("artifact store");

        let error = store
            .write_atomic("rsspkg.lock", b"bad", "package lock")
            .expect_err("symlink destination must fail");

        assert!(error.contains("regular file"), "{error}");
        assert_eq!(fs::read(&outside).expect("outside read"), b"outside");
        drop(store);
        fs::remove_dir_all(root).expect("fixture cleanup");
        fs::remove_file(outside).expect("outside cleanup");
    }

    #[test]
    fn mutation_lock_serializes_store_instances() {
        let root = test_dir("locking");
        fs::create_dir_all(&root).expect("fixture root");
        let first = ArtifactStore::open(&root).expect("first artifact store");
        let (started_tx, started_rx) = mpsc::channel();
        let (opened_tx, opened_rx) = mpsc::channel();
        let thread_root = root.clone();
        let handle = thread::spawn(move || {
            started_tx.send(()).expect("started notification");
            let second = ArtifactStore::open(&thread_root).expect("second artifact store");
            opened_tx.send(()).expect("opened notification");
            drop(second);
        });

        started_rx.recv().expect("thread should start");
        assert!(
            opened_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "second store must wait for the first store lock"
        );
        drop(first);
        opened_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second store should acquire released lock");
        handle.join().expect("writer thread");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn concurrent_writers_publish_only_complete_values() {
        let root = test_dir("concurrent");
        fs::create_dir_all(&root).expect("fixture root");
        let root = Arc::new(root);
        let barrier = Arc::new(Barrier::new(3));
        let values = [vec![b'a'; 128 * 1024], vec![b'b'; 128 * 1024]];
        let handles = values
            .iter()
            .cloned()
            .map(|value| {
                let root = Arc::clone(&root);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    ArtifactStore::open(&root)
                        .expect("artifact store")
                        .write_atomic("rsspkg.lock", &value, "package lock")
                        .expect("atomic write");
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for handle in handles {
            handle.join().expect("writer thread");
        }

        let actual = fs::read(root.join("rsspkg.lock")).expect("published artifact");
        assert!(actual == values[0] || actual == values[1]);
        assert!(
            fs::read_dir(root.as_ref())
                .expect("root entries")
                .all(|entry| !entry
                    .expect("root entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
        fs::remove_dir_all(root.as_ref()).expect("fixture cleanup");
    }

    #[test]
    fn regular_tree_snapshot_and_digest_are_deterministic() {
        let root = test_dir("snapshot");
        let source = root.join("source");
        let destination = root.join("snapshot");
        fs::create_dir_all(source.join("nested")).expect("fixture source tree");
        fs::write(source.join("manifest.toml"), b"name = 'fixture'\n").expect("fixture manifest");
        fs::write(source.join("nested/input.rss"), b"fn main() {}\n").expect("fixture source");

        snapshot_regular_tree(
            &source,
            &destination,
            ProjectTreeLimits::default(),
            "test snapshot",
            |_, _| false,
        )
        .expect("snapshot should succeed");
        assert_eq!(
            fs::read(destination.join("nested/input.rss")).expect("copied source"),
            b"fn main() {}\n"
        );

        let first = regular_tree_digest(
            &destination,
            ProjectTreeLimits::default(),
            "test snapshot digest",
            b"rsscript-artifact-store-test-v1\\0",
        )
        .expect("first digest");
        let second = regular_tree_digest(
            &destination,
            ProjectTreeLimits::default(),
            "test snapshot digest",
            b"rsscript-artifact-store-test-v1\\0",
        )
        .expect("second digest");
        assert_eq!(first, second);

        fs::write(destination.join("nested/input.rss"), b"fn main() { 1 }\n")
            .expect("mutated copy");
        let changed = regular_tree_digest(
            &destination,
            ProjectTreeLimits::default(),
            "test snapshot digest",
            b"rsscript-artifact-store-test-v1\\0",
        )
        .expect("changed digest");
        assert_ne!(first, changed);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn native_snapshot_store_reuses_only_a_revalidated_entry() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_dir("native-snapshot-store");
        let store = NativeSnapshotStore::open(&root).expect("native snapshot store");
        let first_staging = store.stage().expect("first staging tree");
        fs::create_dir_all(first_staging.path().join("native")).expect("staging source tree");
        fs::write(
            first_staging.path().join("native/lib.rs"),
            b"pub fn first() {}\n",
        )
        .expect("staged source");
        let first = store
            .publish(
                first_staging,
                ProjectTreeLimits::default(),
                "test native snapshot",
                b"rsscript-native-snapshot-test-v1\0",
            )
            .expect("first publication");

        let second_staging = store.stage().expect("second staging tree");
        fs::create_dir_all(second_staging.path().join("native")).expect("staging source tree");
        fs::write(
            second_staging.path().join("native/lib.rs"),
            b"pub fn first() {}\n",
        )
        .expect("staged source");
        let second = store
            .publish(
                second_staging,
                ProjectTreeLimits::default(),
                "test native snapshot",
                b"rsscript-native-snapshot-test-v1\0",
            )
            .expect("existing entry should be revalidated and reused");
        assert_eq!(first.digest(), second.digest());
        assert_eq!(first.path(), second.path());
        assert_eq!(
            fs::read(first.path().join("native/lib.rs")).expect("published source"),
            b"pub fn first() {}\n"
        );

        fn make_writable(path: &Path) {
            let metadata = fs::metadata(path).expect("inspect cleanup path");
            let mut permissions = metadata.permissions();
            permissions.set_mode(if metadata.is_dir() { 0o700 } else { 0o600 });
            fs::set_permissions(path, permissions).expect("unseal cleanup path");
            if metadata.is_dir() {
                for entry in fs::read_dir(path).expect("read cleanup directory") {
                    make_writable(&entry.expect("cleanup entry").path());
                }
            }
        }
        make_writable(&root);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn generated_rust_package_writer_is_atomic_and_removes_stale_main() {
        let root = test_dir("generated-rust");
        write_generated_rust_package(
            &root,
            GeneratedRustPackageFiles {
                cargo_toml: "[package]\nname = 'fixture'\n",
                lib_rs: "pub fn lib() {}\n",
                main_rs: Some("fn main() {}\n"),
                source_map_json: "[]\n",
            },
        )
        .expect("generated package should write");
        assert_eq!(
            fs::read_to_string(root.join("src/main.rs")).expect("generated main"),
            "fn main() {}\n"
        );

        write_generated_rust_package(
            &root,
            GeneratedRustPackageFiles {
                cargo_toml: "[package]\nname = 'fixture'\n",
                lib_rs: "pub fn lib() {}\n",
                main_rs: None,
                source_map_json: "[]\n",
            },
        )
        .expect("generated package should update");
        assert!(!root.join("src/main.rs").exists());
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn generated_cargo_lock_writer_replaces_existing_regular_content() {
        let root = test_dir("generated-cargo-lock");
        fs::create_dir_all(&root).expect("fixture root");
        let cargo_toml = root.join("Cargo.toml");
        fs::write(&cargo_toml, "[package]\nname = 'fixture'\n").expect("fixture manifest");
        fs::write(root.join("Cargo.lock"), "old\n").expect("fixture lock");

        write_generated_cargo_lock(&cargo_toml, "version = 4\n")
            .expect("generated Cargo.lock should publish");
        assert_eq!(
            fs::read_to_string(root.join("Cargo.lock")).expect("published lock"),
            "version = 4\n"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn generated_cargo_lock_writer_rejects_a_symlink_destination() {
        use std::os::unix::fs::symlink;

        let root = test_dir("generated-cargo-lock-link");
        let outside = test_dir("generated-cargo-lock-link-outside");
        fs::create_dir_all(&root).expect("fixture root");
        fs::write(root.join("Cargo.toml"), "[package]\nname = 'fixture'\n")
            .expect("fixture manifest");
        fs::write(&outside, "outside\n").expect("outside content");
        symlink(&outside, root.join("Cargo.lock")).expect("fixture lock symlink");

        let error = write_generated_cargo_lock(&root.join("Cargo.toml"), "version = 4\n")
            .expect_err("generated lock must reject symlink destination");
        assert!(error.contains("regular file"), "{error}");
        assert_eq!(
            fs::read_to_string(&outside).expect("outside read"),
            "outside\n"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
        fs::remove_file(outside).expect("outside cleanup");
    }
}
