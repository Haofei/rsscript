#![forbid(unsafe_code)]

//! Typed project capture for filesystem-oriented RSScript tools.
//!
//! This crate is the boundary between an OS-facing workspace loader and the
//! compiler's immutable, in-memory [`FrontendInputSnapshot`]. It deliberately
//! owns no compiler, Artifact, Provider, or VM API: callers capture once here
//! and pass the resulting snapshot to their chosen frontend consumer.

use std::error::Error;
use std::fmt;
use std::path::Path;

use rsscript_operation::OperationContext;
use rsscript_semantics::FrontendInputSnapshot;
use rsscript_workspace_loader::{
    WorkspaceFileKind, WorkspaceLoadError, WorkspaceLoadErrorCode, WorkspaceLoader,
    WorkspaceSnapshot,
};
use sha2::{Digest, Sha256};

pub use rsscript_workspace_loader::WorkspaceSourceFile;

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
        assert!(project
            .files()
            .iter()
            .any(|file| file.kind == WorkspaceFileKind::Test));
        assert_eq!(project.frontend().sources().files().len(), 1);
        assert_eq!(
            project.frontend().sources().files()[0].path(),
            "root/main.rss"
        );
    }
}
