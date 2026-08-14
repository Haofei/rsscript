#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

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
        let root = if package_dir.is_absolute() {
            package_dir.to_path_buf()
        } else {
            base.join(package_dir)
        };
        self.snapshot_at(root)
    }

    /// Compatibility capture API using the process current directory for
    /// relative paths. New embedding code should use snapshot_from.
    pub fn snapshot(&self, package_dir: &Path) -> Result<WorkspaceSnapshot, WorkspaceLoadError> {
        let root = if package_dir.is_absolute() {
            package_dir.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| {
                    WorkspaceLoadError::global(
                        WorkspaceLoadErrorCode::ResolveRoot,
                        format!("cannot resolve current directory: {error}"),
                    )
                })?
                .join(package_dir)
        };
        self.snapshot_at(root)
    }

    /// Compatibility API returning the captured file list.
    pub fn load(&self, package_dir: &Path) -> Result<Vec<WorkspaceSourceFile>, WorkspaceLoadError> {
        self.snapshot(package_dir)
            .map(WorkspaceSnapshot::into_files)
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

    fn snapshot_at(&self, root: PathBuf) -> Result<WorkspaceSnapshot, WorkspaceLoadError> {
        if !root.is_dir() {
            return Err(WorkspaceLoadError::at(
                WorkspaceLoadErrorCode::RootNotDirectory,
                &root,
                "package root is not a directory",
            ));
        }
        let mut files = Vec::new();
        let mut total_bytes = 0u64;
        scan_source_tree(&root, &root, false, self, &mut total_bytes, &mut files)?;

        let mut visited = BTreeSet::new();
        let mut pending_dependencies = dependency_paths(&root)?;
        while let Some(dependency) = pending_dependencies.pop() {
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
                &mut total_bytes,
                &mut files,
            )?;
            pending_dependencies.extend(dependency_paths(&dependency)?);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(WorkspaceSnapshot { root, files })
    }
}

fn scan_source_tree(
    root: &Path,
    display_root: &Path,
    interfaces_only: bool,
    limits: &WorkspaceLoader,
    total_bytes: &mut u64,
    files: &mut Vec<WorkspaceSourceFile>,
) -> Result<(), WorkspaceLoadError> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
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

fn dependency_paths(package_dir: &Path) -> Result<Vec<PathBuf>, WorkspaceLoadError> {
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
        let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        for dependency in dependencies.values() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_has_a_stable_structured_error() {
        let error = WorkspaceLoader::default()
            .load(Path::new("definitely-missing-rsscript-workspace"))
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
    }
}
