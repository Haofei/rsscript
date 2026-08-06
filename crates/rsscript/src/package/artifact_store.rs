#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use fs2::FileExt;

#[cfg(not(unix))]
use super::open_regular_file_no_follow;
use super::{canonical_checked_root, is_package_link_like};

const MUTATION_LOCK: &str = ".rsscript-artifacts.lock";
const ARTIFACT_READ_MAX_BYTES: u64 = 16 * 1024 * 1024;

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
    pub fn open(package_root: &Path) -> Result<Self, String> {
        let root = canonical_checked_root(package_root, "package artifact store")?;
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
        if !metadata.is_dir() || is_package_link_like(&metadata) {
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
                    Ok(metadata) if metadata.is_dir() && !is_package_link_like(&metadata) => {}
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
            fs::rename(&temporary, &destination)
                .map_err(|error| format!("failed to atomically publish {label}: {error}"))?;
            sync_directory(parent)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
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
        && (is_package_link_like(&metadata) || !metadata.is_file())
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
    if !metadata.is_file() || is_package_link_like(&metadata) {
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
}
