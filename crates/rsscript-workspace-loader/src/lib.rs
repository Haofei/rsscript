#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::Read;
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};

use rsscript_operation::{OperationAbort, OperationContext};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_WORKSPACE_FILES: usize = 20_000;
const MAX_WORKSPACE_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFileKind {
    Interface,
    Source,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSourceFile {
    /// Physical path used only by the OS-facing loader and editor adapters.
    pub path: String,
    /// Package-relative display path retained for user diagnostics.
    pub relative_path: String,
    /// Stable snapshot identity. It never includes an absolute host path and
    /// distinguishes root files from dependency interface files with the same
    /// relative path.
    pub logical_path: String,
    pub contents: String,
    pub kind: WorkspaceFileKind,
}

/// The supported, typed project-manifest projection used during workspace
/// capture.
///
/// RSScript currently captures only local path dependencies. Registry, git,
/// and other dependency forms remain outside the project loader's input
/// contract: they may be interpreted by a future resolver, but cannot
/// silently cause the loader to read additional host paths today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceManifestV1 {
    path_dependencies: Vec<WorkspacePathDependencyV1>,
}

impl WorkspaceManifestV1 {
    /// Parse the loader-owned manifest projection from one `rsspkg.toml`
    /// document. The returned dependencies are sorted canonically so capture
    /// order is independent of TOML table ordering.
    pub fn parse(source: &str) -> Result<Self, toml::de::Error> {
        let manifest: RawWorkspaceManifest = toml::from_str(source)?;
        let mut path_dependencies = Vec::new();
        collect_path_dependencies(
            &mut path_dependencies,
            WorkspaceDependencySection::Dependencies,
            manifest.dependencies,
        );
        collect_path_dependencies(
            &mut path_dependencies,
            WorkspaceDependencySection::DevDependencies,
            manifest.dev_dependencies,
        );
        path_dependencies.sort_by(|left, right| {
            (left.section, left.name.as_str(), left.path.as_str()).cmp(&(
                right.section,
                right.name.as_str(),
                right.path.as_str(),
            ))
        });
        Ok(Self { path_dependencies })
    }

    /// Explicit local dependency paths that participate in one captured
    /// workspace. This intentionally excludes non-path dependency forms.
    pub fn path_dependencies(&self) -> &[WorkspacePathDependencyV1] {
        &self.path_dependencies
    }
}

/// One explicit local dependency declared by a project manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspacePathDependencyV1 {
    name: String,
    section: WorkspaceDependencySection,
    path: String,
}

impl WorkspacePathDependencyV1 {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn section(&self) -> WorkspaceDependencySection {
        self.section
    }

    /// The manifest-relative path exactly as declared. The OS-facing loader
    /// resolves it against the owning manifest directory.
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// The only manifest dependency sections considered during local workspace
/// capture. The distinction remains visible for tools without making either
/// section part of compiler semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkspaceDependencySection {
    Dependencies,
    DevDependencies,
}

#[derive(Debug, Deserialize, Default)]
struct RawWorkspaceManifest {
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependency>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, RawDependency>,
}

/// A manifest dependency is either a conventional version requirement or a
/// table. Table fields other than `path` are deliberately ignored by this
/// local capture projection; they cannot create additional filesystem input.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawDependency {
    Version(String),
    Table(RawDependencyTable),
}

#[derive(Debug, Deserialize, Default)]
struct RawDependencyTable {
    #[serde(default)]
    path: Option<String>,
}

fn collect_path_dependencies(
    output: &mut Vec<WorkspacePathDependencyV1>,
    section: WorkspaceDependencySection,
    dependencies: BTreeMap<String, RawDependency>,
) {
    for (name, dependency) in dependencies {
        let dependency = match dependency {
            RawDependency::Version(version) => {
                // Version-only declarations are intentionally outside this
                // local-path capture projection.
                let _ = version;
                continue;
            }
            RawDependency::Table(dependency) => dependency,
        };
        let Some(path) = dependency.path else {
            continue;
        };
        output.push(WorkspacePathDependencyV1 {
            name,
            section,
            path,
        });
    }
}

/// Immutable, filesystem-captured input for one workspace operation.
///
/// The loader owns OS access; compiler and language layers can consume the
/// resulting source bytes without consulting paths or the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    root: PathBuf,
    files: Vec<WorkspaceSourceFile>,
    content_digest: String,
}

impl WorkspaceSnapshot {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn files(&self) -> &[WorkspaceSourceFile] {
        &self.files
    }

    pub fn into_files(self) -> Vec<WorkspaceSourceFile> {
        self.files
    }

    /// Stable identity for the captured source/interface content. Absolute
    /// filesystem paths are deliberately excluded, so equivalent captures on
    /// different hosts have the same input identity.
    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLoadErrorCode {
    ResolveRoot,
    RootNotDirectory,
    ResolveDependency,
    ReadDirectory,
    InspectEntry,
    FileLimitExceeded,
    SourceBytesOverflow,
    SourceBytesLimitExceeded,
    ReadSource,
    ReadManifest,
    ParseManifest,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLoadError {
    pub code: WorkspaceLoadErrorCode,
    pub path: Option<PathBuf>,
    pub message: String,
}

impl WorkspaceLoadError {
    fn at(
        code: WorkspaceLoadErrorCode,
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: Some(path.into()),
            message: message.into(),
        }
    }

    fn global(code: WorkspaceLoadErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            path: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for WorkspaceLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(
                formatter,
                "{:?} at {}: {}",
                self.code,
                path.display(),
                self.message
            )
        } else {
            write!(formatter, "{:?}: {}", self.code, self.message)
        }
    }
}

impl Error for WorkspaceLoadError {}

#[derive(Debug, Clone, Copy)]
pub struct WorkspaceLoader {
    pub max_files: usize,
    pub max_source_bytes: u64,
}

struct ScanState<'a> {
    limits: &'a WorkspaceLoader,
    operation: Option<&'a OperationContext>,
    file_count: usize,
    total_bytes: u64,
}

impl Default for WorkspaceLoader {
    fn default() -> Self {
        Self {
            max_files: MAX_WORKSPACE_FILES,
            max_source_bytes: MAX_WORKSPACE_SOURCE_BYTES,
        }
    }
}

impl WorkspaceLoader {
    /// Capture a workspace relative to an explicit caller-provided base path.
    ///
    /// This is the preferred embedding API because it does not read the
    /// process current directory.
    pub fn snapshot_from(
        &self,
        base: &Path,
        package_dir: &Path,
    ) -> Result<WorkspaceSnapshot, WorkspaceLoadError> {
        self.snapshot_from_inner(base, package_dir, None)
    }

    /// Capture a workspace relative to an explicit base while observing the
    /// caller's cancellation and deadline boundary during filesystem traversal.
    pub fn snapshot_from_with_operation(
        &self,
        base: &Path,
        package_dir: &Path,
        operation: &OperationContext,
    ) -> Result<WorkspaceSnapshot, WorkspaceLoadError> {
        self.snapshot_from_inner(base, package_dir, Some(operation))
    }

    fn snapshot_from_inner(
        &self,
        base: &Path,
        package_dir: &Path,
        operation: Option<&OperationContext>,
    ) -> Result<WorkspaceSnapshot, WorkspaceLoadError> {
        check_operation(operation)?;
        let root = if package_dir.is_absolute() {
            package_dir.to_path_buf()
        } else {
            check_operation(operation)?;
            base.join(package_dir)
        };
        self.snapshot_at(root, operation)
    }

    /// Capture files relative to an explicit base and return the compatibility
    /// file-list view.
    pub fn load_from(
        &self,
        base: &Path,
        package_dir: &Path,
    ) -> Result<Vec<WorkspaceSourceFile>, WorkspaceLoadError> {
        self.snapshot_from(base, package_dir)
            .map(WorkspaceSnapshot::into_files)
    }

    fn snapshot_at(
        &self,
        root: PathBuf,
        operation: Option<&OperationContext>,
    ) -> Result<WorkspaceSnapshot, WorkspaceLoadError> {
        check_operation(operation)?;
        if !root.is_dir() {
            return Err(WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::RootNotDirectory,
                &root,
                "package root is not a directory",
            ));
        }
        let traversal_root = root.canonicalize().map_err(|error| {
            WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::ResolveRoot,
                &root,
                format!("cannot resolve package root: {error}"),
            )
        })?;
        let mut files = Vec::new();
        let mut scan = ScanState {
            limits: self,
            operation,
            file_count: 0,
            total_bytes: 0,
        };
        let mut root_files = Vec::new();
        scan_source_tree(
            &traversal_root,
            &traversal_root,
            false,
            &mut scan,
            &mut root_files,
        )?;
        for file in &mut root_files {
            file.path = root
                .join(&file.relative_path)
                .to_string_lossy()
                .into_owned();
        }
        assign_logical_paths("root", &mut root_files);
        files.extend(root_files);

        let mut visited = BTreeSet::new();
        let mut pending_dependencies = dependency_paths(&traversal_root, operation)?;
        while let Some(dependency) = pending_dependencies.pop() {
            check_operation(operation)?;
            let dependency = dependency.canonicalize().map_err(|error| {
                WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::ResolveDependency,
                    &dependency,
                    format!("cannot resolve dependency package: {error}"),
                )
            })?;
            if !visited.insert(dependency.clone()) {
                continue;
            }
            let mut dependency_files = Vec::new();
            scan_source_tree(
                &dependency,
                &dependency,
                true,
                &mut scan,
                &mut dependency_files,
            )?;
            let identity = dependency_identity(&dependency_files);
            assign_logical_paths(&format!("dependency/{identity}"), &mut dependency_files);
            files.extend(dependency_files);
            pending_dependencies.extend(dependency_paths(&dependency, operation)?);
        }
        check_operation(operation)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let content_digest = snapshot_content_digest(&files);
        Ok(WorkspaceSnapshot {
            root,
            files,
            content_digest,
        })
    }
}

fn snapshot_content_digest(files: &[WorkspaceSourceFile]) -> String {
    let mut canonical_files = files.iter().collect::<Vec<_>>();
    canonical_files.sort_by(|left, right| {
        (
            file_kind_tag(left.kind),
            left.logical_path.as_str(),
            left.contents.as_str(),
        )
            .cmp(&(
                file_kind_tag(right.kind),
                right.logical_path.as_str(),
                right.contents.as_str(),
            ))
    });
    let mut hasher = Sha256::new();
    hasher.update(b"rsscript.workspace_snapshot.v1\0");
    hasher.update((canonical_files.len() as u64).to_be_bytes());
    for file in canonical_files {
        let kind = [file_kind_tag(file.kind)];
        hasher.update(kind);
        hash_snapshot_field(&mut hasher, file.logical_path.as_bytes());
        hash_snapshot_field(&mut hasher, file.contents.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn file_kind_tag(kind: WorkspaceFileKind) -> u8 {
    match kind {
        WorkspaceFileKind::Interface => 1,
        WorkspaceFileKind::Source => 2,
        WorkspaceFileKind::Test => 3,
    }
}

fn hash_snapshot_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn assign_logical_paths(prefix: &str, files: &mut [WorkspaceSourceFile]) {
    for file in files {
        file.logical_path = format!("{prefix}/{}", file.relative_path);
    }
}

fn dependency_identity(files: &[WorkspaceSourceFile]) -> String {
    let mut entries = files.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        (
            file_kind_tag(left.kind),
            left.relative_path.as_str(),
            left.contents.as_str(),
        )
            .cmp(&(
                file_kind_tag(right.kind),
                right.relative_path.as_str(),
                right.contents.as_str(),
            ))
    });
    let mut hasher = Sha256::new();
    hasher.update(b"rsscript.workspace_dependency.v1\0");
    for file in entries {
        hasher.update([file_kind_tag(file.kind)]);
        hash_snapshot_field(&mut hasher, file.relative_path.as_bytes());
        hash_snapshot_field(&mut hasher, file.contents.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn scan_source_tree(
    root: &Path,
    display_root: &Path,
    interfaces_only: bool,
    scan: &mut ScanState<'_>,
    files: &mut Vec<WorkspaceSourceFile>,
) -> Result<(), WorkspaceLoadError> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        check_operation(scan.operation)?;
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| {
                WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::ReadDirectory,
                    &directory,
                    format!("cannot read directory: {error}"),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::ReadDirectory,
                    &directory,
                    format!("cannot read directory entry: {error}"),
                )
            })?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            check_operation(scan.operation)?;
            let file_type = entry.file_type().map_err(|error| {
                WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::InspectEntry,
                    entry.path(),
                    format!("cannot inspect entry: {error}"),
                )
            })?;
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                let name = entry.file_name();
                if !matches!(
                    name.to_str(),
                    Some(".git" | ".claude" | "target" | "native")
                ) {
                    pending.push(path);
                }
                continue;
            }
            let kind = match path.extension().and_then(|extension| extension.to_str()) {
                Some("rssi") => WorkspaceFileKind::Interface,
                Some("rss") if interfaces_only => continue,
                Some("rss") if path.components().any(|part| part.as_os_str() == "tests") => {
                    WorkspaceFileKind::Test
                }
                Some("rss") => WorkspaceFileKind::Source,
                _ => continue,
            };
            if scan.file_count >= scan.limits.max_files {
                return Err(WorkspaceLoadError::global(
                    WorkspaceLoadErrorCode::FileLimitExceeded,
                    "workspace source file count exceeds loader limit",
                ));
            }
            let remaining = scan
                .limits
                .max_source_bytes
                .checked_sub(scan.total_bytes)
                .ok_or_else(|| {
                    WorkspaceLoadError::global(
                        WorkspaceLoadErrorCode::SourceBytesLimitExceeded,
                        "workspace source bytes exceed loader limit",
                    )
                })?;
            let contents = read_source_utf8_bounded(display_root, &path, remaining)?;
            scan.total_bytes = scan
                .total_bytes
                .checked_add(contents.len() as u64)
                .ok_or_else(|| {
                    WorkspaceLoadError::global(
                        WorkspaceLoadErrorCode::SourceBytesOverflow,
                        "workspace source byte count overflow",
                    )
                })?;
            let relative_path = path
                .strip_prefix(display_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push(WorkspaceSourceFile {
                path: path.to_string_lossy().into_owned(),
                relative_path,
                logical_path: String::new(),
                contents,
                kind,
            });
            scan.file_count = scan.file_count.saturating_add(1);
        }
    }
    Ok(())
}

/// Open a source exactly once, reject link-like leaf entries, inspect the
/// opened descriptor, and read no more than the workspace's remaining byte
/// budget. This closes the metadata/open and size-growth races that would
/// otherwise let a concurrently replaced file escape capture policy.
fn read_source_utf8_bounded(
    root: &Path,
    path: &Path,
    max_bytes: u64,
) -> Result<String, WorkspaceLoadError> {
    #[cfg(not(unix))]
    let _ = root;
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};

        let relative = path.strip_prefix(root).map_err(|_| {
            WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::ReadSource,
                path,
                "workspace source is outside its capture root",
            )
        })?;
        let root_descriptor = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::ReadSource,
                root,
                format!("cannot open workspace root without following links: {error}"),
            )
        })?;
        let mut current = File::from(root_descriptor);
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(component) = component else {
                return Err(WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::ReadSource,
                    path,
                    "workspace source path is not confined",
                ));
            };
            let flags = if components.peek().is_some() {
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            } else {
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            };
            let descriptor = rustix::fs::openat(&current, component, flags, Mode::empty())
                .map_err(|error| {
                    WorkspaceLoadError::at(
                        WorkspaceLoadErrorCode::ReadSource,
                        path,
                        format!("cannot open confined source without following links: {error}"),
                    )
                })?;
            current = File::from(descriptor);
        }
        current
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::InspectEntry,
                path,
                format!("cannot inspect source: {error}"),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::InspectEntry,
                path,
                "workspace source must not be a symlink or reparse point",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(path).map_err(|error| {
            WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::ReadSource,
                path,
                format!("cannot open source without following links: {error}"),
            )
        })?
    };
    let metadata = file.metadata().map_err(|error| {
        WorkspaceLoadError::at(
            WorkspaceLoadErrorCode::InspectEntry,
            path,
            format!("cannot inspect opened source: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(WorkspaceLoadError::at(
            WorkspaceLoadErrorCode::InspectEntry,
            path,
            "workspace source must be a regular file",
        ));
    }
    if metadata.len() > max_bytes {
        return Err(WorkspaceLoadError::global(
            WorkspaceLoadErrorCode::SourceBytesLimitExceeded,
            "workspace source bytes exceed loader limit",
        ));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        WorkspaceLoadError::global(
            WorkspaceLoadErrorCode::SourceBytesOverflow,
            "workspace source file is too large for this platform",
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::take(file, max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::ReadSource,
                path,
                format!("cannot read source: {error}"),
            )
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(WorkspaceLoadError::global(
            WorkspaceLoadErrorCode::SourceBytesLimitExceeded,
            "workspace source bytes exceed loader limit",
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        WorkspaceLoadError::at(
            WorkspaceLoadErrorCode::ReadSource,
            path,
            format!("source is not valid UTF-8: {error}"),
        )
    })
}

fn dependency_paths(
    package_dir: &Path,
    operation: Option<&OperationContext>,
) -> Result<Vec<PathBuf>, WorkspaceLoadError> {
    check_operation(operation)?;
    let manifest_path = package_dir.join("rsspkg.toml");
    let source = match fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::ReadManifest,
                &manifest_path,
                format!("cannot read manifest: {error}"),
            ));
        }
    };
    let manifest = WorkspaceManifestV1::parse(&source).map_err(|error| {
        WorkspaceLoadError::at(
            WorkspaceLoadErrorCode::ParseManifest,
            &manifest_path,
            format!("cannot parse manifest: {error}"),
        )
    })?;
    let mut paths = Vec::with_capacity(manifest.path_dependencies().len());
    for dependency in manifest.path_dependencies() {
        check_operation(operation)?;
        paths.push(package_dir.join(dependency.path()));
    }
    paths.sort();
    Ok(paths)
}

fn check_operation(operation: Option<&OperationContext>) -> Result<(), WorkspaceLoadError> {
    operation.map_or(Ok(()), |operation| {
        operation.check().map_err(|abort| {
            let code = match abort {
                OperationAbort::Cancelled => WorkspaceLoadErrorCode::Cancelled,
                OperationAbort::DeadlineExceeded => WorkspaceLoadErrorCode::DeadlineExceeded,
            };
            WorkspaceLoadError::global(code, format!("workspace capture stopped: {abort:?}"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_operation::{CancellationToken, MonotonicDeadline};
    use std::io::Write;
    use std::time::{Duration, Instant};

    #[test]
    fn typed_manifest_exposes_only_explicit_sorted_path_dependencies() {
        let manifest = WorkspaceManifestV1::parse(
            r#"
                [dependencies]
                registry = "1.2.3"
                zeta = { path = "../zeta", version = "0.1" }
                alpha = { path = "../alpha" }
                remote = { git = "https://example.invalid/remote" }

                [dev-dependencies]
                test = { path = "../test" }
            "#,
        )
        .expect("typed manifest projection must parse supported dependency forms");

        let dependencies = manifest.path_dependencies();
        assert_eq!(dependencies.len(), 3);
        assert_eq!(dependencies[0].name(), "alpha");
        assert_eq!(
            dependencies[0].section(),
            WorkspaceDependencySection::Dependencies
        );
        assert_eq!(dependencies[0].path(), "../alpha");
        assert_eq!(dependencies[1].name(), "zeta");
        assert_eq!(
            dependencies[1].section(),
            WorkspaceDependencySection::Dependencies
        );
        assert_eq!(dependencies[2].name(), "test");
        assert_eq!(
            dependencies[2].section(),
            WorkspaceDependencySection::DevDependencies
        );
    }

    #[test]
    fn typed_manifest_rejects_dependency_values_outside_supported_toml_shapes() {
        let error = WorkspaceManifestV1::parse(
            r#"
                [dependencies]
                invalid = 42
            "#,
        )
        .expect_err("invalid dependency shapes must not be silently interpreted");
        assert!(error.to_string().contains("untagged enum"));
    }

    #[test]
    fn missing_root_has_a_stable_structured_error() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let error = WorkspaceLoader::default()
            .load_from(base, Path::new("definitely-missing-rsscript-workspace"))
            .unwrap_err();
        assert_eq!(error.code, WorkspaceLoadErrorCode::RootNotDirectory);
        assert!(error.path.is_some());
    }

    #[test]
    fn bounded_source_reader_uses_the_open_descriptor_and_actual_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.rss");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"12345").unwrap();
        file.flush().unwrap();

        assert_eq!(
            read_source_utf8_bounded(directory.path(), &path, 5).unwrap(),
            "12345"
        );
        let error = read_source_utf8_bounded(directory.path(), &path, 4).unwrap_err();
        assert_eq!(error.code, WorkspaceLoadErrorCode::SourceBytesLimitExceeded);
    }

    #[cfg(unix)]
    #[test]
    fn bounded_source_reader_never_follows_a_symlink_leaf() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("outside.rss");
        fs::write(&target, "secret").unwrap();
        let link = directory.path().join("main.rss");
        symlink(&target, &link).unwrap();

        let error = read_source_utf8_bounded(directory.path(), &link, 1024).unwrap_err();
        assert_eq!(error.code, WorkspaceLoadErrorCode::ReadSource);
    }

    #[cfg(unix)]
    #[test]
    fn capture_resolves_an_explicit_symlink_root_once() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("main.rss"),
            "fn main() -> Unit { return Unit }",
        )
        .unwrap();
        let root = directory.path().join("root");
        symlink(&package, &root).unwrap();

        let snapshot = WorkspaceLoader::default()
            .snapshot_from(directory.path(), Path::new("root"))
            .unwrap();
        assert_eq!(snapshot.root(), root);
        assert_eq!(snapshot.files().len(), 1);
        assert_eq!(
            snapshot.files()[0].path,
            root.join("main.rss").display().to_string()
        );
        assert_eq!(snapshot.files()[0].relative_path, "main.rss");
    }

    #[test]
    fn explicit_base_capture_does_not_require_process_current_directory() {
        let loader = WorkspaceLoader::default();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let snapshot = loader.snapshot_from(root, Path::new(".")).unwrap();
        assert_eq!(snapshot.root(), root);
        assert_eq!(
            loader.load_from(root, Path::new(".")).unwrap(),
            snapshot.files()
        );
        assert!(snapshot.content_digest().starts_with("sha256:"));
    }

    #[test]
    fn operation_aware_capture_rejects_cancelled_and_expired_requests_before_io() {
        let loader = WorkspaceLoader::default();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let cancelled_error = loader
            .snapshot_from_with_operation(
                root,
                Path::new("."),
                &OperationContext {
                    cancellation: Some(cancelled),
                    ..OperationContext::default()
                },
            )
            .expect_err("cancelled request must not capture a workspace");
        assert_eq!(cancelled_error.code, WorkspaceLoadErrorCode::Cancelled);

        let expired_error = loader
            .snapshot_from_with_operation(
                root,
                Path::new("."),
                &OperationContext {
                    deadline: Some(MonotonicDeadline::at(
                        Instant::now() - Duration::from_millis(1),
                    )),
                    ..OperationContext::default()
                },
            )
            .expect_err("expired request must not capture a workspace");
        assert_eq!(expired_error.code, WorkspaceLoadErrorCode::DeadlineExceeded);
    }

    #[test]
    fn snapshot_digest_is_independent_of_absolute_paths_and_file_enumeration_order() {
        let first = vec![
            WorkspaceSourceFile {
                path: "/one/src/main.rss".to_string(),
                relative_path: "src/main.rss".to_string(),
                logical_path: "root/src/main.rss".to_string(),
                contents: "fn main() -> Unit {}".to_string(),
                kind: WorkspaceFileKind::Source,
            },
            WorkspaceSourceFile {
                path: "/one/interfaces/host.rssi".to_string(),
                relative_path: "interfaces/host.rssi".to_string(),
                logical_path: "root/interfaces/host.rssi".to_string(),
                contents: "module host".to_string(),
                kind: WorkspaceFileKind::Interface,
            },
        ];
        let second = vec![
            WorkspaceSourceFile {
                path: "/two/interfaces/host.rssi".to_string(),
                relative_path: "interfaces/host.rssi".to_string(),
                logical_path: "root/interfaces/host.rssi".to_string(),
                contents: "module host".to_string(),
                kind: WorkspaceFileKind::Interface,
            },
            WorkspaceSourceFile {
                path: "/two/src/main.rss".to_string(),
                relative_path: "src/main.rss".to_string(),
                logical_path: "root/src/main.rss".to_string(),
                contents: "fn main() -> Unit {}".to_string(),
                kind: WorkspaceFileKind::Source,
            },
        ];
        assert_eq!(
            snapshot_content_digest(&first),
            snapshot_content_digest(&second)
        );

        let mut changed = second;
        changed[1].contents.push_str("\n// changed");
        assert_ne!(
            snapshot_content_digest(&first),
            snapshot_content_digest(&changed)
        );
    }

    #[test]
    fn logical_paths_distinguish_root_and_dependency_files_with_the_same_relative_path() {
        let mut root = WorkspaceSourceFile {
            path: "/host-a/src/main.rss".to_string(),
            relative_path: "src/main.rss".to_string(),
            logical_path: String::new(),
            contents: "fn main() -> Unit {}".to_string(),
            kind: WorkspaceFileKind::Source,
        };
        let mut dependency = WorkspaceSourceFile {
            path: "/host-b/src/main.rss".to_string(),
            relative_path: "src/main.rss".to_string(),
            logical_path: String::new(),
            contents: "module dependency".to_string(),
            kind: WorkspaceFileKind::Interface,
        };
        assign_logical_paths("root", std::slice::from_mut(&mut root));
        assign_logical_paths(
            "dependency/sha256:fixture",
            std::slice::from_mut(&mut dependency),
        );

        assert_ne!(root.logical_path, dependency.logical_path);
        assert_ne!(
            snapshot_content_digest(&[root]),
            snapshot_content_digest(&[dependency])
        );
    }
}
