#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use rsscript_operation::{OperationAbort, OperationContext};
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
    pub path: String,
    pub relative_path: String,
    pub contents: String,
    pub kind: WorkspaceFileKind,
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
        let mut files = Vec::new();
        let mut total_bytes = 0u64;
        scan_source_tree(
            &root,
            &root,
            false,
            self,
            operation,
            &mut total_bytes,
            &mut files,
        )?;

        let mut visited = BTreeSet::new();
        let mut pending_dependencies = dependency_paths(&root, operation)?;
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
            scan_source_tree(
                &dependency,
                &dependency,
                true,
                self,
                operation,
                &mut total_bytes,
                &mut files,
            )?;
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
    hasher.update(b"rsscript.workspace_snapshot.v1\0");
    hasher.update((canonical_files.len() as u64).to_be_bytes());
    for file in canonical_files {
        let kind = [file_kind_tag(file.kind)];
        hasher.update(kind);
        hash_snapshot_field(&mut hasher, file.relative_path.as_bytes());
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

fn scan_source_tree(
    root: &Path,
    display_root: &Path,
    interfaces_only: bool,
    limits: &WorkspaceLoader,
    operation: Option<&OperationContext>,
    total_bytes: &mut u64,
    files: &mut Vec<WorkspaceSourceFile>,
) -> Result<(), WorkspaceLoadError> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        check_operation(operation)?;
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
            check_operation(operation)?;
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
            if files.len() >= limits.max_files {
                return Err(WorkspaceLoadError::global(
                    WorkspaceLoadErrorCode::FileLimitExceeded,
                    "workspace source file count exceeds loader limit",
                ));
            }
            let metadata = entry.metadata().map_err(|error| {
                WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::InspectEntry,
                    &path,
                    format!("cannot inspect source: {error}"),
                )
            })?;
            *total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
                WorkspaceLoadError::global(
                    WorkspaceLoadErrorCode::SourceBytesOverflow,
                    "workspace source byte count overflow",
                )
            })?;
            if *total_bytes > limits.max_source_bytes {
                return Err(WorkspaceLoadError::global(
                    WorkspaceLoadErrorCode::SourceBytesLimitExceeded,
                    "workspace source bytes exceed loader limit",
                ));
            }
            let contents = fs::read_to_string(&path).map_err(|error| {
                WorkspaceLoadError::at(
                    WorkspaceLoadErrorCode::ReadSource,
                    &path,
                    format!("cannot read source: {error}"),
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
                contents,
                kind,
            });
        }
    }
    Ok(())
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
    let manifest: toml::Value = toml::from_str(&source).map_err(|error| {
        WorkspaceLoadError::at(
            WorkspaceLoadErrorCode::ParseManifest,
            &manifest_path,
            format!("cannot parse manifest: {error}"),
        )
    })?;
    let mut paths = Vec::new();
    for section in ["dependencies", "dev-dependencies"] {
        check_operation(operation)?;
        let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for dependency in dependencies.values() {
            check_operation(operation)?;
            if let Some(path) = dependency
                .as_table()
                .and_then(|entry| entry.get("path"))
                .and_then(toml::Value::as_str)
            {
                paths.push(package_dir.join(path));
            }
        }
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
    use std::time::{Duration, Instant};

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
                contents: "fn main() -> Unit {}".to_string(),
                kind: WorkspaceFileKind::Source,
            },
            WorkspaceSourceFile {
                path: "/one/interfaces/host.rssi".to_string(),
                relative_path: "interfaces/host.rssi".to_string(),
                contents: "module host".to_string(),
                kind: WorkspaceFileKind::Interface,
            },
        ];
        let second = vec![
            WorkspaceSourceFile {
                path: "/two/interfaces/host.rssi".to_string(),
                relative_path: "interfaces/host.rssi".to_string(),
                contents: "module host".to_string(),
                kind: WorkspaceFileKind::Interface,
            },
            WorkspaceSourceFile {
                path: "/two/src/main.rss".to_string(),
                relative_path: "src/main.rss".to_string(),
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
}
