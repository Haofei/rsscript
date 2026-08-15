#![forbid(unsafe_code)]

//! Typed project capture for filesystem-oriented RSScript tools.
//!
//! This crate is the boundary between an OS-facing workspace loader and the
//! compiler's immutable, in-memory [`FrontendInputSnapshot`]. It deliberately
//! owns no compiler, Artifact Bundle construction, Provider, or VM API:
//! callers capture once here and pass the resulting snapshot to their chosen
//! frontend consumer.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use rsscript_operation::OperationContext;
use rsscript_semantics::FrontendInputSnapshot;
use rsscript_workspace_loader::{
    WorkspaceFileKind, WorkspaceLoadError, WorkspaceLoadErrorCode, WorkspaceLoader,
    WorkspaceSnapshot,
};
use sha2::{Digest, Sha256};

pub use rsscript_artifact::PackageIdentityV1 as PackageIdentity;
pub use rsscript_workspace_loader::WorkspaceSourceFile;

const PROJECT_CAPTURE_MAX_FILES: usize = 20_000;
const PROJECT_CAPTURE_MAX_ENTRIES: usize = 40_000;
const PROJECT_CAPTURE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PROJECT_CAPTURE_MAX_DEPTH: usize = 64;

/// Private, bounded filesystem capture of a package graph.
///
/// The temporary directory remains owned by this value, so a compiler or
/// review adapter can safely consume the captured paths without reopening the
/// original package graph. This is intentionally a project boundary rather
/// than a compiler facility: it performs OS I/O and rejects link-like entries.
#[derive(Debug)]
pub struct CapturedProjectGraph {
    _directory: tempfile::TempDir,
    root: PathBuf,
    paths: Vec<(PathBuf, PathBuf)>,
}

/// One selected package root inside an immutable captured project graph.
///
/// The graph retains the private temporary directory and every dependency
/// mapping; this projection adds the root consumed by one package-oriented
/// adapter without exposing mutable checkout paths.
#[derive(Debug)]
pub struct CapturedPackageGraph {
    captured: CapturedProjectGraph,
    root: PathBuf,
}

/// Bounded raw manifest input captured from a non-link project root.
///
/// Parsing package-specific fields intentionally belongs to a higher layer;
/// this type only establishes the filesystem boundary and preserves the exact
/// bytes that parser consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectManifestSnapshot {
    root: PathBuf,
    source: String,
}

impl ProjectManifestSnapshot {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Capture `rsspkg.toml` from one project root without following links.
///
/// The caller supplies the byte bound appropriate to its manifest schema.
/// This is deliberately raw input: package manifest decoding, feature
/// resolution, review, and native policy remain outside the project boundary.
pub fn capture_project_manifest(
    package_dir: &Path,
    max_bytes: u64,
) -> Result<ProjectManifestSnapshot, String> {
    let root = canonical_capture_root(package_dir)?;
    let manifest_path = root.join("rsspkg.toml");
    let source = read_regular_utf8_no_follow(&manifest_path, max_bytes, "project manifest")?;
    Ok(ProjectManifestSnapshot { root, source })
}

impl CapturedProjectGraph {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Captured path corresponding to an original package root.
    pub fn captured_path(&self, original: &Path) -> Option<&Path> {
        let original = original
            .canonicalize()
            .unwrap_or_else(|_| original.to_path_buf());
        self.paths
            .iter()
            .find(|(candidate, _)| {
                candidate
                    .canonicalize()
                    .unwrap_or_else(|_| candidate.clone())
                    == original
            })
            .map(|(_, captured)| captured.as_path())
    }

    /// Map a captured path back to its source graph path for diagnostics and
    /// human presentation. No mutable checkout path is exposed.
    pub fn original_path(&self, captured_path: &Path) -> Option<PathBuf> {
        let captured_path = captured_path
            .canonicalize()
            .unwrap_or_else(|_| captured_path.to_path_buf());
        self.paths.iter().find_map(|(original, captured)| {
            let captured = captured
                .canonicalize()
                .unwrap_or_else(|_| captured.to_path_buf());
            captured_path.strip_prefix(&captured).ok().map(|relative| {
                if relative.as_os_str().is_empty() {
                    original.clone()
                } else {
                    original.join(relative)
                }
            })
        })
    }

    pub fn path_mappings(&self) -> &[(PathBuf, PathBuf)] {
        &self.paths
    }

    /// Select one captured package root after graph assembly has completed.
    pub fn select_package_root(self, original_root: &Path) -> Result<CapturedPackageGraph, String> {
        let root = self.captured_path(original_root).ok_or_else(|| {
            format!(
                "captured graph does not contain original package root {}",
                original_root.display()
            )
        })?;
        Ok(CapturedPackageGraph {
            root: root.to_path_buf(),
            captured: self,
        })
    }

    /// Read a bounded UTF-8 file from the private captured graph.
    ///
    /// `original_root` identifies a root supplied to
    /// [`capture_project_graph`]; `relative_path` is restricted to normal path
    /// components. Compatibility adapters can therefore inspect the immutable
    /// private copy without reopening a mutable author checkout.
    pub fn read_captured_utf8(
        &self,
        original_root: &Path,
        relative_path: &Path,
        max_bytes: u64,
    ) -> Result<String, String> {
        let path = self.captured_regular_file_path(original_root, relative_path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "failed to inspect captured file {}: {error}",
                path.display()
            )
        })?;
        if metadata.len() > max_bytes {
            return Err(format!(
                "captured file exceeds {max_bytes}-byte limit: {}",
                path.display()
            ));
        }
        let mut file = File::open(&path)
            .map_err(|error| format!("failed to open captured file {}: {error}", path.display()))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).map_err(|error| {
            format!(
                "failed to read UTF-8 captured file {}: {error}",
                path.display()
            )
        })?;
        if contents.len() as u64 > max_bytes {
            return Err(format!(
                "captured file changed beyond {max_bytes}-byte limit while reading: {}",
                path.display()
            ));
        }
        Ok(contents)
    }

    /// Whether a regular, non-link file exists beneath one captured root.
    pub fn has_captured_regular_file(
        &self,
        original_root: &Path,
        relative_path: &Path,
    ) -> Result<bool, String> {
        let path = self.captured_file_path(original_root, relative_path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "failed to inspect captured file {}: {error}",
                    path.display()
                ));
            }
        };
        if is_link_like(&metadata) || !metadata.is_file() {
            return Err(format!(
                "captured graph entry must be a regular non-link file: {}",
                path.display()
            ));
        }
        Ok(true)
    }

    /// Replace a captured UTF-8 file only if it still matches the source seen
    /// during dependency resolution. This updates a private mirror, never an
    /// author's original checkout.
    pub fn replace_captured_utf8(
        &self,
        original_root: &Path,
        relative_path: &Path,
        expected_contents: &str,
        replacement: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        if replacement.len() as u64 > max_bytes {
            return Err(format!(
                "replacement exceeds {max_bytes}-byte limit for captured file {}",
                relative_path.display()
            ));
        }
        let path = self.captured_regular_file_path(original_root, relative_path)?;
        let actual = self.read_captured_utf8(original_root, relative_path, max_bytes)?;
        if actual != expected_contents {
            return Err(format!(
                "captured file changed after graph capture: {}",
                path.display()
            ));
        }
        let mut output = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "failed to rewrite captured file {}: {error}",
                    path.display()
                )
            })?;
        output.write_all(replacement.as_bytes()).map_err(|error| {
            format!(
                "failed to rewrite captured file {}: {error}",
                path.display()
            )
        })?;
        output
            .flush()
            .map_err(|error| format!("failed to flush captured file {}: {error}", path.display()))
    }

    /// Create a new bounded UTF-8 metadata file inside a captured package
    /// root. Existing files are never overwritten; graph assemblers use this
    /// only for capture-owned metadata excluded from the copied tree.
    pub fn create_captured_utf8(
        &self,
        original_root: &Path,
        relative_path: &Path,
        contents: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        if contents.len() as u64 > max_bytes {
            return Err(format!(
                "new captured file exceeds {max_bytes}-byte limit: {}",
                relative_path.display()
            ));
        }
        let path = self.captured_file_path(original_root, relative_path)?;
        let parent = path
            .parent()
            .expect("captured file path always has a captured-root parent");
        let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
            format!(
                "failed to inspect captured file parent {}: {error}",
                parent.display()
            )
        })?;
        if is_link_like(&parent_metadata) || !parent_metadata.is_dir() {
            return Err(format!(
                "captured file parent must be a real directory: {}",
                parent.display()
            ));
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!("failed to create captured file {}: {error}", path.display())
            })?;
        output.write_all(contents.as_bytes()).map_err(|error| {
            format!("failed to write captured file {}: {error}", path.display())
        })?;
        output
            .flush()
            .map_err(|error| format!("failed to flush captured file {}: {error}", path.display()))
    }

    fn captured_regular_file_path(
        &self,
        original_root: &Path,
        relative_path: &Path,
    ) -> Result<PathBuf, String> {
        let path = self.captured_file_path(original_root, relative_path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "failed to inspect captured file {}: {error}",
                path.display()
            )
        })?;
        if is_link_like(&metadata) || !metadata.is_file() {
            return Err(format!(
                "captured graph entry must be a regular non-link file: {}",
                path.display()
            ));
        }
        Ok(path)
    }

    fn captured_file_path(
        &self,
        original_root: &Path,
        relative_path: &Path,
    ) -> Result<PathBuf, String> {
        if relative_path.as_os_str().is_empty()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "captured graph relative path must contain only normal components: {}",
                relative_path.display()
            ));
        }
        let root = self.captured_path(original_root).ok_or_else(|| {
            format!(
                "captured graph does not contain original package root {}",
                original_root.display()
            )
        })?;
        Ok(root.join(relative_path))
    }
}

impl CapturedPackageGraph {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn original_path(&self, snapshot_path: &Path) -> Option<PathBuf> {
        self.captured.original_path(snapshot_path)
    }

    /// Rewrite a graph-private path label for host-facing diagnostics without
    /// leaking the private temporary capture location.
    pub fn remap_path_label(&self, value: &str) -> String {
        if let Some(path) = value.strip_prefix("path+") {
            return self
                .original_path(Path::new(path))
                .map(|path| format!("path+{}", path.display()))
                .unwrap_or_else(|| value.to_string());
        }
        self.original_path(Path::new(value))
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| value.to_string())
    }

    /// Replace every private capture prefix in an error string with its
    /// original project path, preferring the deepest matching path first.
    pub fn remap_error(&self, error: String) -> String {
        let mut paths = self.captured.path_mappings().iter().collect::<Vec<_>>();
        paths.sort_by_key(|(_, captured)| std::cmp::Reverse(captured.as_os_str().len()));
        paths
            .into_iter()
            .fold(error, |error, (original, captured)| {
                error.replace(
                    &captured.display().to_string(),
                    &original.display().to_string(),
                )
            })
    }
}

/// Capture the given package roots into one private temporary graph.
///
/// Roots are canonicalized, deduplicated, copied in deterministic order, and
/// mirrored below a private `packages/` directory. The caller supplies only
/// names to exclude from every copied directory; no caller-controlled copy
/// callback can weaken the confinement or resource bounds.
pub fn capture_project_graph(
    roots: impl IntoIterator<Item = PathBuf>,
    excluded_entry_names: impl IntoIterator<Item = impl AsRef<str>>,
    operation: Option<&OperationContext>,
) -> Result<CapturedProjectGraph, String> {
    check_capture_operation(operation)?;
    let excluded = excluded_entry_names
        .into_iter()
        .map(|name| name.as_ref().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let mut roots = roots
        .into_iter()
        .map(|original| canonical_capture_root(&original).map(|canonical| (original, canonical)))
        .collect::<Result<Vec<_>, _>>()?;
    roots.sort_by(|(_, left), (_, right)| left.cmp(right));
    roots.dedup_by(|(_, left), (_, right)| left == right);
    if roots.is_empty() {
        return Err("project graph capture requires at least one package root".to_string());
    }

    let directory = tempfile::Builder::new()
        .prefix("rsscript-project-graph-")
        .tempdir()
        .map_err(|error| format!("failed to create private project graph snapshot: {error}"))?;
    set_private_directory_permissions(directory.path())?;
    let packages_root = directory.path().join("packages");
    fs::create_dir_all(&packages_root).map_err(|error| {
        format!(
            "failed to create project graph snapshot root {}: {error}",
            packages_root.display()
        )
    })?;
    set_private_directory_permissions(&packages_root)?;

    let mut paths = Vec::with_capacity(roots.len());
    for (original, root) in roots {
        check_capture_operation(operation)?;
        let destination = mirrored_capture_path(&packages_root, &root)?;
        copy_project_directory(&root, &destination, &excluded, operation)?;
        paths.push((original, destination));
    }
    Ok(CapturedProjectGraph {
        _directory: directory,
        root: packages_root,
        paths,
    })
}

#[derive(Debug)]
struct CaptureBudget {
    files: usize,
    entries: usize,
    bytes: u64,
}

impl CaptureBudget {
    fn check_operation(&self, operation: Option<&OperationContext>) -> Result<(), String> {
        check_capture_operation(operation)
    }

    fn add_entry(
        &mut self,
        operation: Option<&OperationContext>,
        path: &Path,
    ) -> Result<(), String> {
        self.check_operation(operation)?;
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            format!(
                "project graph capture directory entry count overflow while visiting {}",
                path.display()
            )
        })?;
        if self.entries > PROJECT_CAPTURE_MAX_ENTRIES {
            return Err(format!(
                "project graph capture exceeded directory entry limit of {PROJECT_CAPTURE_MAX_ENTRIES} at {}",
                path.display()
            ));
        }
        Ok(())
    }

    fn add_file(
        &mut self,
        operation: Option<&OperationContext>,
        bytes: u64,
        path: &Path,
    ) -> Result<(), String> {
        self.check_operation(operation)?;
        self.files = self.files.checked_add(1).ok_or_else(|| {
            format!(
                "project graph capture file count overflow while visiting {}",
                path.display()
            )
        })?;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(|| {
            format!(
                "project graph capture byte count overflow while visiting {}",
                path.display()
            )
        })?;
        if self.files > PROJECT_CAPTURE_MAX_FILES {
            return Err(format!(
                "project graph capture exceeded file count limit of {PROJECT_CAPTURE_MAX_FILES} at {}",
                path.display()
            ));
        }
        if self.bytes > PROJECT_CAPTURE_MAX_BYTES {
            return Err(format!(
                "project graph capture exceeded total byte limit of {PROJECT_CAPTURE_MAX_BYTES} at {}",
                path.display()
            ));
        }
        Ok(())
    }

    fn check_depth(
        &self,
        operation: Option<&OperationContext>,
        depth: usize,
        path: &Path,
    ) -> Result<(), String> {
        self.check_operation(operation)?;
        if depth > PROJECT_CAPTURE_MAX_DEPTH {
            return Err(format!(
                "project graph capture exceeded directory depth limit of {PROJECT_CAPTURE_MAX_DEPTH} at {}",
                path.display()
            ));
        }
        Ok(())
    }
}

fn check_capture_operation(operation: Option<&OperationContext>) -> Result<(), String> {
    operation.map_or(Ok(()), |operation| {
        operation
            .check()
            .map_err(|abort| format!("project graph capture stopped: {abort:?}"))
    })
}

fn canonical_capture_root(path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect project capture root {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_dir() || is_link_like(&metadata) {
        return Err(format!(
            "project graph capture requires a non-link directory root: {}",
            path.display()
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to canonicalize project capture root {}: {error}",
            path.display()
        )
    })
}

fn read_regular_utf8_no_follow(path: &Path, max_bytes: u64, label: &str) -> Result<String, String> {
    #[cfg(unix)]
    let mut file = {
        use rustix::fs::{Mode, OFlags};

        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            format!(
                "failed to open {label} {} without following links: {error}",
                path.display()
            )
        })?;
        File::from(descriptor)
    };
    #[cfg(not(unix))]
    let mut file = {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(path).map_err(|error| {
            format!(
                "failed to open {label} {} without following links: {error}",
                path.display()
            )
        })?
    };
    let metadata = file.metadata().map_err(|error| {
        format!(
            "failed to inspect opened {label} {}: {error}",
            path.display()
        )
    })?;
    if is_link_like(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{label} requires a regular non-link file: {}",
            path.display()
        ));
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "{label} {} exceeded byte limit of {max_bytes}",
            path.display()
        ));
    }
    let capacity = usize::try_from(metadata.len().min(max_bytes))
        .map_err(|_| format!("{label} is too large for this platform: {}", path.display()))?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{label} {} exceeded byte limit of {max_bytes} while reading",
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        format!(
            "failed to read {label} {} as UTF-8: {error}",
            path.display()
        )
    })
}

fn mirrored_capture_path(root: &Path, source: &Path) -> Result<PathBuf, String> {
    let mut destination = root.to_path_buf();
    for component in source.components() {
        match component {
            Component::Prefix(prefix) => destination.push(capture_path_component(
                &prefix.as_os_str().to_string_lossy(),
            )),
            Component::RootDir => destination.push("root"),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "project graph capture root must be canonical: {}",
                    source.display()
                ));
            }
            Component::Normal(component) => {
                destination.push(capture_path_component(&component.to_string_lossy()))
            }
        }
    }
    Ok(destination)
}

fn capture_path_component(component: &str) -> String {
    let mut encoded = String::with_capacity(component.len());
    for byte in component.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "_{byte:02x}");
        }
    }
    if encoded.is_empty() {
        "_".to_string()
    } else {
        encoded
    }
}

fn copy_project_directory(
    source: &Path,
    destination: &Path,
    excluded: &std::collections::BTreeSet<String>,
    operation: Option<&OperationContext>,
) -> Result<(), String> {
    let root = canonical_capture_root(source)?;
    let mut budget = CaptureBudget {
        files: 0,
        entries: 0,
        bytes: 0,
    };
    copy_project_directory_inner(
        &root,
        &root,
        destination,
        excluded,
        operation,
        0,
        &mut budget,
    )
}

fn copy_project_directory_inner(
    root: &Path,
    source: &Path,
    destination: &Path,
    excluded: &std::collections::BTreeSet<String>,
    operation: Option<&OperationContext>,
    depth: usize,
    budget: &mut CaptureBudget,
) -> Result<(), String> {
    budget.check_depth(operation, depth, source)?;
    let metadata = capture_path_metadata(root, source)?;
    if metadata.is_file() {
        let (input, metadata) = open_capture_file_within_root(root, source)?;
        budget.add_file(operation, metadata.len(), source)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            set_private_directory_permissions(parent)?;
        }
        copy_capture_file(input, source, destination, metadata.len())?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "project graph capture only accepts regular files and directories: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    set_private_directory_permissions(destination)?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?
        .map(|entry| {
            entry.map_err(|error| format!("failed to read entry in {}: {error}", source.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for entry in &entries {
        budget.add_entry(operation, &entry.path())?;
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if excluded.contains(&name.to_string_lossy().to_string()) {
            continue;
        }
        let path = entry.path();
        let target = destination.join(&name);
        copy_project_directory_inner(root, &path, &target, excluded, operation, depth + 1, budget)?;
    }
    Ok(())
}

fn capture_path_metadata(root: &Path, path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "failed to inspect project capture path {}: {error}",
            path.display()
        )
    })?;
    if is_link_like(&metadata) {
        return Err(format!(
            "project graph capture rejects symlinks or reparse points: {}",
            path.display()
        ));
    }
    ensure_capture_path_within_root(root, path)?;
    Ok(metadata)
}

fn ensure_capture_path_within_root(root: &Path, path: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to canonicalize project capture path {}: {error}",
            path.display()
        )
    })?;
    if canonical.strip_prefix(root).is_ok() {
        Ok(())
    } else {
        Err(format!(
            "project graph capture path escapes its root: {}",
            path.display()
        ))
    }
}

fn open_capture_file_within_root(root: &Path, path: &Path) -> Result<(File, fs::Metadata), String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "project graph capture path escapes its root: {}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};

        let root_fd = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            format!(
                "failed to open project capture root {}: {error}",
                root.display()
            )
        })?;
        let mut current = File::from(root_fd);
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(component) = component else {
                return Err(format!(
                    "project graph capture path is not confined: {}",
                    path.display()
                ));
            };
            let flags = if components.peek().is_some() {
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            } else {
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            };
            let fd =
                rustix::fs::openat(&current, component, flags, Mode::empty()).map_err(|error| {
                    format!(
                        "failed to open confined project capture path {}: {error}",
                        path.display()
                    )
                })?;
            current = File::from(fd);
        }
        let metadata = current
            .metadata()
            .map_err(|error| format!("failed to inspect opened {}: {error}", path.display()))?;
        if !metadata.is_file() || is_link_like(&metadata) {
            return Err(format!(
                "project graph capture requires a regular file: {}",
                path.display()
            ));
        }
        Ok((current, metadata))
    }
    #[cfg(not(unix))]
    {
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
                "failed to open project capture path {}: {error}",
                path.display()
            )
        })?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("failed to inspect opened {}: {error}", path.display()))?;
        if !metadata.is_file() || is_link_like(&metadata) {
            return Err(format!(
                "project graph capture requires a regular file: {}",
                path.display()
            ));
        }
        Ok((file, metadata))
    }
}

fn copy_capture_file(
    mut input: File,
    source: &Path,
    destination: &Path,
    expected: u64,
) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut output = options
        .open(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let metadata = output
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", destination.display()))?;
    if !metadata.is_file() || is_link_like(&metadata) {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "project graph capture destination is not a regular file: {}",
            destination.display()
        ));
    }
    let copied = std::io::copy(
        &mut Read::by_ref(&mut input).take(expected.saturating_add(1)),
        &mut output,
    )
    .map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    if copied != expected {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "project graph capture input changed while copying: {}",
            source.display()
        ));
    }
    output.flush().map_err(|error| {
        format!(
            "failed to flush copied file {}: {error}",
            destination.display()
        )
    })
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

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("failed to protect {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Native dependency metadata captured as part of an immutable project graph.
///
/// Experimental Rust/AOT lowering may consume this model, but it does not own
/// package identity, paths, or dependency policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRustDependency {
    pub crate_name: String,
    pub path: String,
    pub cargo_features: Vec<String>,
    pub default_features: bool,
    pub bindings: BTreeMap<String, String>,
}

/// Captured package input that can be projected to the compiler's pure,
/// in-memory frontend boundary.
///
/// This model retains package/AOT compatibility metadata for callers that
/// need it, while [`Self::frontend_input`] selects only source and interface
/// bytes for semantic compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLoweringInput {
    pub package: PackageIdentity,
    pub package_dir: String,
    pub source_path: String,
    pub source_relative_path: String,
    pub source: String,
    pub sources: Vec<(String, String)>,
    pub interfaces: Vec<(String, String)>,
    pub native_dependencies: Vec<NativeRustDependency>,
}

impl PackageLoweringInput {
    /// Project/package projection into the compiler's pure input boundary. No
    /// filesystem state is retained by the resulting value.
    pub fn frontend_input(&self) -> FrontendInputSnapshot {
        FrontendInputSnapshot::from_sources(
            self.sources
                .iter()
                .map(|(path, contents)| (path.as_str(), contents.as_str())),
            self.interfaces
                .iter()
                .map(|(path, contents)| (path.as_str(), contents.as_str())),
        )
    }
}

/// Immutable project input captured from one filesystem workspace.
///
/// The loader-owned workspace retains test files for tools, while `frontend`
/// contains exactly the source and interface bytes eligible for compilation.
/// Neither value rereads the filesystem after capture.
#[derive(Debug, Clone)]
pub struct ProjectSnapshot {
    workspace: WorkspaceSnapshot,
    frontend: FrontendInputSnapshot,
    frontend_digest: String,
}

impl ProjectSnapshot {
    pub fn frontend(&self) -> &FrontendInputSnapshot {
        &self.frontend
    }

    /// Stable identity of every file captured by the OS-facing loader.
    /// Absolute host paths are excluded.
    pub fn content_digest(&self) -> &str {
        self.workspace.content_digest()
    }

    /// Stable identity of exactly the source and interface input presented to
    /// a compiler. Test-only files intentionally do not affect this digest.
    pub fn frontend_digest(&self) -> &str {
        &self.frontend_digest
    }

    pub fn files(&self) -> &[WorkspaceSourceFile] {
        self.workspace.files()
    }
}

/// Project-capture failure classification suitable for composition by SDKs,
/// CLIs, and editor adapters without exposing loader implementation details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectLoadErrorCode {
    Capture,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLoadError {
    code: ProjectLoadErrorCode,
    message: String,
}

impl ProjectLoadError {
    pub fn code(&self) -> ProjectLoadErrorCode {
        self.code
    }
}

impl fmt::Display for ProjectLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProjectLoadError {}

/// The project/input capture boundary. An explicit base path is always
/// required, so this API never consults the process current directory.
#[derive(Debug, Clone, Default)]
pub struct ProjectLoader {
    workspace_loader: WorkspaceLoader,
}

impl ProjectLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a project loader with caller-selected bounded filesystem
    /// capture limits.
    pub fn with_workspace_loader(workspace_loader: WorkspaceLoader) -> Self {
        Self { workspace_loader }
    }

    pub fn capture_from(
        &self,
        base: &Path,
        package_dir: &Path,
    ) -> Result<ProjectSnapshot, ProjectLoadError> {
        self.capture_inner(base, package_dir, None)
    }

    pub fn capture_from_with_operation(
        &self,
        base: &Path,
        package_dir: &Path,
        operation: &OperationContext,
    ) -> Result<ProjectSnapshot, ProjectLoadError> {
        self.capture_inner(base, package_dir, Some(operation))
    }

    fn capture_inner(
        &self,
        base: &Path,
        package_dir: &Path,
        operation: Option<&OperationContext>,
    ) -> Result<ProjectSnapshot, ProjectLoadError> {
        let workspace = match operation {
            Some(operation) => {
                self.workspace_loader
                    .snapshot_from_with_operation(base, package_dir, operation)
            }
            None => self.workspace_loader.snapshot_from(base, package_dir),
        }
        .map_err(map_workspace_load_error)?;
        let sources = workspace
            .files()
            .iter()
            .filter(|file| file.kind == WorkspaceFileKind::Source)
            .map(|file| (file.logical_path.as_str(), file.contents.as_str()))
            .collect::<Vec<_>>();
        let interfaces = workspace
            .files()
            .iter()
            .filter(|file| file.kind == WorkspaceFileKind::Interface)
            .map(|file| (file.logical_path.as_str(), file.contents.as_str()))
            .collect::<Vec<_>>();
        let frontend = FrontendInputSnapshot::from_sources(sources, interfaces);
        let frontend_digest = frontend_snapshot_digest(&frontend);
        Ok(ProjectSnapshot {
            workspace,
            frontend,
            frontend_digest,
        })
    }
}

fn map_workspace_load_error(error: WorkspaceLoadError) -> ProjectLoadError {
    let code = match error.code {
        WorkspaceLoadErrorCode::Cancelled => ProjectLoadErrorCode::Cancelled,
        WorkspaceLoadErrorCode::DeadlineExceeded => ProjectLoadErrorCode::DeadlineExceeded,
        _ => ProjectLoadErrorCode::Capture,
    };
    ProjectLoadError {
        code,
        message: error.to_string(),
    }
}

fn frontend_snapshot_digest(snapshot: &FrontendInputSnapshot) -> String {
    let mut entries = snapshot
        .sources()
        .files()
        .iter()
        .map(|file| ("source", file.path(), file.text()))
        .chain(
            snapshot
                .interfaces()
                .files()
                .iter()
                .map(|file| ("interface", file.path(), file.text())),
        )
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(b"rsscript.in_memory_snapshot.v1\0");
    for (role, path, text) in entries {
        for value in [role.as_bytes(), path.as_bytes(), text.as_bytes()] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value);
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_manifest_capture_is_bounded_and_preserves_the_parser_input() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("manifest");
        let snapshot = capture_project_manifest(directory.path(), 1024).expect("capture manifest");
        assert_eq!(
            snapshot.root(),
            directory
                .path()
                .canonicalize()
                .expect("canonical workspace root")
        );
        assert!(snapshot.source().contains("name = \"fixture\""));
        assert!(capture_project_manifest(directory.path(), 4).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_manifest_capture_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("workspace");
        let outside = directory.path().join("outside.toml");
        std::fs::write(&outside, "[package]\nname = \"outside\"\n").expect("outside manifest");
        symlink(&outside, directory.path().join("rsspkg.toml")).expect("manifest link");
        assert!(capture_project_manifest(directory.path(), 1024).is_err());
    }

    #[test]
    fn project_graph_capture_is_private_bounded_and_maps_paths_back() {
        let directory = tempfile::tempdir().expect("workspace");
        let package = directory.path().join("package");
        std::fs::create_dir_all(package.join("nested")).expect("package directories");
        std::fs::write(
            package.join("main.rss"),
            "fn main() -> Unit { return Unit }",
        )
        .expect("source");
        std::fs::write(package.join("nested/input.txt"), "captured").expect("input");
        std::fs::write(package.join("target"), "excluded").expect("excluded input");

        let graph = capture_project_graph([package.clone()], ["target"], None)
            .expect("bounded graph capture");
        let captured = graph
            .captured_path(&package)
            .expect("original package has a captured path");
        assert_eq!(
            std::fs::read_to_string(captured.join("nested/input.txt")).expect("captured contents"),
            "captured"
        );
        assert!(!captured.join("target").exists());
        assert_eq!(
            graph.original_path(&captured.join("nested/input.txt")),
            Some(package.join("nested/input.txt"))
        );
        assert_eq!(
            graph
                .read_captured_utf8(&package, Path::new("nested/input.txt"), 1024)
                .expect("read captured text"),
            "captured"
        );
        graph
            .replace_captured_utf8(
                &package,
                Path::new("nested/input.txt"),
                "captured",
                "rewritten",
                1024,
            )
            .expect("rewrite private capture");
        assert_eq!(
            graph
                .read_captured_utf8(&package, Path::new("nested/input.txt"), 1024)
                .expect("read rewritten text"),
            "rewritten"
        );
        assert!(
            graph
                .replace_captured_utf8(
                    &package,
                    Path::new("nested/input.txt"),
                    "captured",
                    "ignored",
                    1024,
                )
                .is_err()
        );
        graph
            .create_captured_utf8(
                &package,
                Path::new("capture-metadata.toml"),
                "identity = 'captured'\n",
                1024,
            )
            .expect("create capture-owned metadata");
        assert_eq!(
            graph
                .read_captured_utf8(&package, Path::new("capture-metadata.toml"), 1024)
                .expect("read capture metadata"),
            "identity = 'captured'\n"
        );
        assert!(
            graph
                .create_captured_utf8(
                    &package,
                    Path::new("capture-metadata.toml"),
                    "replacement",
                    1024,
                )
                .is_err()
        );
        assert!(
            graph
                .read_captured_utf8(&package, Path::new("../outside"), 1024)
                .is_err()
        );
        let captured_input = captured.join("nested/input.txt");
        let selected = graph
            .select_package_root(&package)
            .expect("select captured package root");
        assert!(selected.root().is_dir());
        assert_eq!(
            selected.original_path(&captured_input),
            Some(package.join("nested/input.txt"))
        );
        assert_eq!(
            selected.remap_path_label(&captured_input.display().to_string()),
            package.join("nested/input.txt").display().to_string()
        );
        assert!(
            selected
                .remap_error(format!("failed under {}", selected.root().display()))
                .contains(&package.display().to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn project_graph_capture_rejects_links_without_reading_their_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("workspace");
        let package = directory.path().join("package");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&package).expect("package directory");
        std::fs::write(&outside, "outside").expect("outside input");
        symlink(&outside, package.join("link")).expect("fixture link");

        let error = capture_project_graph([package], std::iter::empty::<&str>(), None)
            .expect_err("links are not a valid package capture input");
        assert!(error.contains("rejects symlinks"), "{error}");
        assert_eq!(
            std::fs::read_to_string(outside).expect("outside contents"),
            "outside"
        );
    }

    #[test]
    fn frontend_digest_excludes_test_files_but_retains_logical_source_identity() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(directory.path().join("tests")).expect("tests directory");
        std::fs::write(
            directory.path().join("main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("source");
        std::fs::write(
            directory.path().join("tests/check.rss"),
            "fn check() -> Unit { return Unit }\n",
        )
        .expect("test");

        let project = ProjectLoader::new()
            .capture_from(directory.path(), Path::new("."))
            .expect("capture");
        assert!(project.content_digest().starts_with("sha256:"));
        assert!(project.frontend_digest().starts_with("sha256:"));
        assert!(
            project
                .files()
                .iter()
                .any(|file| file.kind == WorkspaceFileKind::Test)
        );
        assert_eq!(project.frontend().sources().files().len(), 1);
        assert_eq!(
            project.frontend().sources().files()[0].path(),
            "root/main.rss"
        );
    }

    #[test]
    fn captured_package_input_projects_only_compiler_frontend_bytes() {
        let input = PackageLoweringInput {
            package: PackageIdentity {
                name: "fixture".into(),
                version: "0.1.0".into(),
                edition: "2024".into(),
            },
            package_dir: "/host-specific/fixture".into(),
            source_path: "/host-specific/fixture/src/main.rss".into(),
            source_relative_path: "src/main.rss".into(),
            source: "fn main() -> Unit { return Unit }".into(),
            sources: vec![(
                "root/src/main.rss".into(),
                "fn main() -> Unit { return Unit }".into(),
            )],
            interfaces: vec![(
                "dep/api.rssi".into(),
                "module api\npub fn log(message: read String) -> Unit".into(),
            )],
            native_dependencies: vec![NativeRustDependency {
                crate_name: "fixture-native".into(),
                path: "/host-specific/native".into(),
                cargo_features: vec!["fast".into()],
                default_features: false,
                bindings: BTreeMap::new(),
            }],
        };
        let frontend = input.frontend_input();
        assert_eq!(frontend.sources().files().len(), 1);
        assert_eq!(frontend.interfaces().files().len(), 1);
        assert_eq!(frontend.sources().files()[0].path(), "root/src/main.rss");
        assert_eq!(frontend.interfaces().files()[0].path(), "dep/api.rssi");
    }
}
