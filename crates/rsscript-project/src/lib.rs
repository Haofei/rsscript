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
    WorkspaceDependencySection, WorkspaceFileKind, WorkspaceLoadError, WorkspaceLoadErrorCode,
    WorkspaceLoader, WorkspaceManifestV1, WorkspaceSnapshot,
};
use sha2::{Digest, Sha256};

pub use rsscript_artifact::PackageIdentityV1 as PackageIdentity;
pub use rsscript_workspace_loader::WorkspaceSourceFile;

const PROJECT_CAPTURE_MAX_FILES: usize = 20_000;
const PROJECT_CAPTURE_MAX_ENTRIES: usize = 40_000;
const PROJECT_CAPTURE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PROJECT_CAPTURE_MAX_DEPTH: usize = 64;
const PROJECT_MANIFEST_GRAPH_MAX_PACKAGES: usize = 4_096;
const PROJECT_MANIFEST_GRAPH_MAX_BYTES: u64 = 32 * 1024 * 1024;

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

/// One project-owned local dependency declaration captured from a manifest.
///
/// This is intentionally a filesystem discovery fact, not a package-policy
/// or feature-resolution model. Compiler compatibility code may interpret the
/// raw manifest bytes, while project capture owns which local roots exist.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectPathDependency {
    name: String,
    section: WorkspaceDependencySection,
    declared_path: String,
}

impl ProjectPathDependency {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn section(&self) -> WorkspaceDependencySection {
        self.section
    }

    pub fn declared_path(&self) -> &str {
        &self.declared_path
    }
}

/// One immutable manifest node in a captured local dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectManifestGraphPackage {
    root: PathBuf,
    manifest_source: String,
    path_dependencies: Vec<ProjectPathDependency>,
}

impl ProjectManifestGraphPackage {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_source(&self) -> &str {
        &self.manifest_source
    }

    pub fn path_dependencies(&self) -> &[ProjectPathDependency] {
        &self.path_dependencies
    }
}

/// Bounded input limits for [`capture_project_manifest_graph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectManifestGraphLimits {
    pub max_packages: usize,
    pub max_manifest_bytes: u64,
    pub max_total_manifest_bytes: u64,
}

impl Default for ProjectManifestGraphLimits {
    fn default() -> Self {
        Self {
            max_packages: PROJECT_MANIFEST_GRAPH_MAX_PACKAGES,
            max_manifest_bytes: MANIFEST_CAPTURE_MAX_BYTES,
            max_total_manifest_bytes: PROJECT_MANIFEST_GRAPH_MAX_BYTES,
        }
    }
}

const MANIFEST_CAPTURE_MAX_BYTES: u64 = 1024 * 1024;

/// Immutable raw-manifest capture for every available local path dependency.
///
/// The graph is rooted at a canonical package directory, sorted by canonical
/// root, deduplicated across aliases, and bounded before compiler package
/// semantics consume it. Registry, git, and version-only declarations remain
/// absent by construction: they cannot make this loader read a host path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectManifestGraph {
    root: PathBuf,
    packages: Vec<ProjectManifestGraphPackage>,
}

impl ProjectManifestGraph {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn packages(&self) -> &[ProjectManifestGraphPackage] {
        &self.packages
    }

    pub fn package(&self, root: &Path) -> Option<&ProjectManifestGraphPackage> {
        let root = root.canonicalize().ok()?;
        self.packages.iter().find(|package| package.root == root)
    }

    /// Raw captured manifest bytes for one root in this graph.
    ///
    /// The lookup's canonicalization remains inside the project boundary, so
    /// compiler compatibility consumers never need to reopen or probe a
    /// manifest merely to find the bytes captured for a dependency.
    pub fn manifest_source(&self, root: &Path) -> Option<&str> {
        self.package(root)
            .map(ProjectManifestGraphPackage::manifest_source)
    }
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

/// Resolve one manifest-declared local package dependency at the project I/O
/// boundary.
///
/// A missing `rsspkg.toml` remains an unresolved dependency rather than an
/// I/O failure so package-semantic callers can retain their existing
/// diagnostics. When a manifest is present, its root is checked to be a
/// canonical non-link directory, while the returned path preserves the
/// manifest spelling needed by legacy lockfile identity. Compiler and review
/// code must not reproduce this path joining and filesystem probing.
pub fn resolve_project_path_dependency(
    package_dir: &Path,
    declared_path: &str,
) -> Result<Option<PathBuf>, String> {
    let package_root = canonical_capture_root(package_dir)?;
    let declared = Path::new(declared_path);
    let candidate = if declared.is_absolute() {
        declared.to_path_buf()
    } else {
        package_root.join(declared)
    };
    let manifest = candidate.join("rsspkg.toml");
    match fs::symlink_metadata(&manifest) {
        Ok(metadata) if is_link_like(&metadata) => Err(format!(
            "project dependency manifest rejects symlinks or reparse points: {}",
            manifest.display()
        )),
        Ok(_) => {
            canonical_capture_root(&candidate)?;
            Ok(Some(candidate))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to inspect project dependency manifest {}: {error}",
            manifest.display()
        )),
    }
}

/// Capture typed local dependency discovery and the raw manifests it selected.
///
/// Package syntax, feature resolution, review/risk, and native metadata are
/// intentionally not interpreted here. This boundary supplies the immutable
/// manifest graph that those compatibility consumers will progressively adopt.
pub fn capture_project_manifest_graph(
    package_dir: &Path,
    limits: ProjectManifestGraphLimits,
) -> Result<ProjectManifestGraph, String> {
    if limits.max_packages == 0 {
        return Err("project manifest graph requires a non-zero package limit".to_string());
    }
    let root_snapshot = capture_project_manifest(package_dir, limits.max_manifest_bytes)?;
    let root = root_snapshot.root().to_path_buf();
    let mut pending = vec![root.clone()];
    let mut packages = BTreeMap::<PathBuf, ProjectManifestGraphPackage>::new();
    let mut total_bytes = 0_u64;

    while let Some(candidate) = pending.pop() {
        let snapshot = capture_project_manifest(&candidate, limits.max_manifest_bytes)?;
        let canonical_root = snapshot.root().to_path_buf();
        if packages.contains_key(&canonical_root) {
            continue;
        }
        if packages.len() >= limits.max_packages {
            return Err(format!(
                "project manifest graph exceeded package limit of {} at {}",
                limits.max_packages,
                canonical_root.display()
            ));
        }
        let source_bytes = u64::try_from(snapshot.source().len()).map_err(|_| {
            format!(
                "project manifest graph manifest length overflow at {}",
                canonical_root.display()
            )
        })?;
        total_bytes = total_bytes.checked_add(source_bytes).ok_or_else(|| {
            format!(
                "project manifest graph byte accounting overflow at {}",
                canonical_root.display()
            )
        })?;
        if total_bytes > limits.max_total_manifest_bytes {
            return Err(format!(
                "project manifest graph exceeded total manifest byte limit of {} at {}",
                limits.max_total_manifest_bytes,
                canonical_root.display()
            ));
        }
        let manifest = WorkspaceManifestV1::parse(snapshot.source()).map_err(|error| {
            format!(
                "failed to parse project manifest {}: {error}",
                canonical_root.join("rsspkg.toml").display()
            )
        })?;
        let path_dependencies = manifest
            .path_dependencies()
            .iter()
            .map(|dependency| ProjectPathDependency {
                name: dependency.name().to_string(),
                section: dependency.section(),
                declared_path: dependency.path().to_string(),
            })
            .collect::<Vec<_>>();
        for dependency in path_dependencies.iter().rev() {
            if let Some(root) =
                resolve_project_path_dependency(&canonical_root, dependency.declared_path())?
            {
                pending.push(root);
            }
        }
        packages.insert(
            canonical_root.clone(),
            ProjectManifestGraphPackage {
                root: canonical_root,
                manifest_source: snapshot.source().to_string(),
                path_dependencies,
            },
        );
    }

    Ok(ProjectManifestGraph {
        root,
        packages: packages.into_values().collect(),
    })
}

/// Capture one bounded, project-relative UTF-8 file without following links.
///
/// This is intentionally a narrow project-boundary primitive for immutable
/// snapshot metadata. Package-specific interpretation remains with the caller.
pub fn capture_project_utf8(
    package_dir: &Path,
    relative_path: &str,
    max_bytes: u64,
    label: &str,
) -> Result<String, String> {
    let root = canonical_capture_root(package_dir)?;
    let path = confined_project_path(&root, relative_path, label)?;
    read_regular_utf8_within_root(&root, &path, max_bytes, label).map(|(source, _)| source)
}

/// Capture an optional bounded project-relative UTF-8 file without making a
/// compiler consumer probe the filesystem itself.
///
/// A missing file is represented as `Ok(None)`; every present path receives
/// the same confinement and no-follow checks as [`capture_project_utf8`].
pub fn capture_optional_project_utf8(
    package_dir: &Path,
    relative_path: &str,
    max_bytes: u64,
    label: &str,
) -> Result<Option<String>, String> {
    let root = canonical_capture_root(package_dir)?;
    let path = confined_project_path(&root, relative_path, label)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if is_link_like(&metadata) => Err(format!(
            "{label} rejects symlinks or reparse points: {}",
            path.display()
        )),
        Ok(_) => read_regular_utf8_within_root(&root, &path, max_bytes, label)
            .map(|(source, _)| Some(source)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to inspect {label} {}: {error}",
            path.display()
        )),
    }
}

/// Bounds for project-owned RSScript source capture. The compiler selects
/// semantic source roots, while this boundary owns traversal and file I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectSourceCaptureLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_depth: usize,
}

/// A source file captured from an explicitly selected project-relative root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSourceFile {
    relative_path: String,
    contents: String,
}

impl ProjectSourceFile {
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// Stateful project source capture shared by multiple source-root selections.
///
/// One package manifest can select base, feature, source, and test roots. The
/// aggregate budget is intentionally retained across calls so a caller cannot
/// reset limits simply by splitting its manifest into more sections.
#[derive(Debug)]
pub struct ProjectSourceCapture {
    root: PathBuf,
    limits: ProjectSourceCaptureLimits,
    files: usize,
    bytes: u64,
}

/// Resource limits for a generic project-owned regular-file tree scan.
///
/// This is intentionally independent of any package/review policy. Callers
/// supply their operation label and entry filter; the project boundary owns
/// confinement, link rejection, deterministic traversal, and accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectTreeLimits {
    pub max_files: usize,
    pub max_entries: usize,
    pub max_bytes: u64,
    pub max_depth: usize,
}

impl Default for ProjectTreeLimits {
    fn default() -> Self {
        Self {
            max_files: PROJECT_CAPTURE_MAX_FILES,
            max_entries: PROJECT_CAPTURE_MAX_ENTRIES,
            max_bytes: PROJECT_CAPTURE_MAX_BYTES,
            max_depth: PROJECT_CAPTURE_MAX_DEPTH,
        }
    }
}

/// One regular file admitted by [`collect_project_regular_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRegularFile {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Collect regular files under a non-link project root with deterministic,
/// bounded traversal.
///
/// `skip` receives the parent directory and child entry name before the child
/// is inspected. It is appropriate for caller-owned policy such as excluding
/// build output; it cannot bypass the boundary checks for visited entries.
pub fn collect_project_regular_files(
    path: &Path,
    limits: ProjectTreeLimits,
    operation: &str,
    skip: impl Fn(&Path, &str) -> bool,
) -> Result<Vec<ProjectRegularFile>, String> {
    fn visit(
        root: &Path,
        path: &Path,
        depth: usize,
        limits: ProjectTreeLimits,
        operation: &str,
        skip: &impl Fn(&Path, &str) -> bool,
        budget: &mut ProjectTreeBudget,
        files: &mut Vec<ProjectRegularFile>,
    ) -> Result<(), String> {
        budget.check_depth(limits, depth, operation, path)?;
        let metadata = project_path_metadata(path, operation)?;
        ensure_project_path_within_root(root, path, operation)?;
        if metadata.is_file() {
            let (_file, opened_metadata) =
                open_project_regular_file_within_root(root, path, operation)?;
            budget.add_file(limits, opened_metadata.len(), operation, path)?;
            files.push(ProjectRegularFile {
                path: path.to_path_buf(),
                bytes: opened_metadata.len(),
            });
            return Ok(());
        }
        if !metadata.is_dir() {
            return Err(format!(
                "{operation} only accepts regular files or directories: {}",
                path.display()
            ));
        }
        let mut entries = fs::read_dir(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_path = entry.path();
            budget.add_entry(limits, operation, &entry_path)?;
            let name = entry.file_name();
            if skip(path, &name.to_string_lossy()) {
                continue;
            }
            visit(
                root,
                &entry_path,
                depth + 1,
                limits,
                operation,
                skip,
                budget,
                files,
            )?;
        }
        Ok(())
    }

    let root = canonical_project_tree_root(path, operation)?;
    let mut files = Vec::new();
    visit(
        &root,
        path,
        0,
        limits,
        operation,
        &skip,
        &mut ProjectTreeBudget::default(),
        &mut files,
    )?;
    Ok(files)
}

#[derive(Debug, Default)]
struct ProjectTreeBudget {
    files: usize,
    entries: usize,
    bytes: u64,
}

impl ProjectTreeBudget {
    fn check_depth(
        &self,
        limits: ProjectTreeLimits,
        depth: usize,
        operation: &str,
        path: &Path,
    ) -> Result<(), String> {
        if depth > limits.max_depth {
            return Err(format!(
                "{operation} exceeded directory depth limit of {} at {}",
                limits.max_depth,
                path.display()
            ));
        }
        Ok(())
    }

    fn add_file(
        &mut self,
        limits: ProjectTreeLimits,
        bytes: u64,
        operation: &str,
        path: &Path,
    ) -> Result<(), String> {
        self.files = self.files.checked_add(1).ok_or_else(|| {
            format!(
                "{operation} file count overflow while visiting {}",
                path.display()
            )
        })?;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(|| {
            format!(
                "{operation} byte count overflow while visiting {}",
                path.display()
            )
        })?;
        if self.files > limits.max_files {
            return Err(format!(
                "{operation} exceeded file count limit of {} at {}",
                limits.max_files,
                path.display()
            ));
        }
        if self.bytes > limits.max_bytes {
            return Err(format!(
                "{operation} exceeded total byte limit of {} at {}",
                limits.max_bytes,
                path.display()
            ));
        }
        Ok(())
    }

    fn add_entry(
        &mut self,
        limits: ProjectTreeLimits,
        operation: &str,
        path: &Path,
    ) -> Result<(), String> {
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            format!(
                "{operation} directory entry count overflow while visiting {}",
                path.display()
            )
        })?;
        if self.entries > limits.max_entries {
            return Err(format!(
                "{operation} exceeded directory entry limit of {} at {}",
                limits.max_entries,
                path.display()
            ));
        }
        Ok(())
    }
}

impl ProjectSourceCapture {
    pub fn new(package_dir: &Path, limits: ProjectSourceCaptureLimits) -> Result<Self, String> {
        Ok(Self {
            root: canonical_capture_root(package_dir)?,
            limits,
            files: 0,
            bytes: 0,
        })
    }

    /// Capture `.rss` and `.rssi` files beneath the selected roots, excluding
    /// any exact project-relative roots from traversal. All supplied roots are
    /// manifest data and must remain confined beneath the package root.
    pub fn capture(
        &mut self,
        roots: &[String],
        excluded_roots: &[String],
    ) -> Result<Vec<ProjectSourceFile>, String> {
        let excluded = excluded_roots
            .iter()
            .map(|value| confined_project_path(&self.root, value, "source root"))
            .collect::<Result<Vec<_>, _>>()?;
        let mut output = Vec::new();
        for value in roots {
            let root = confined_project_path(&self.root, value, "source root")?;
            if !root.exists() {
                continue;
            }
            let mut paths = Vec::new();
            self.collect_paths(&root, &excluded, 0, &mut paths)?;
            paths.sort();
            for path in paths {
                let (contents, bytes) = read_regular_utf8_within_root(
                    &self.root,
                    &path,
                    self.limits.max_file_bytes,
                    "project source",
                )?;
                self.add_file(bytes, &path)?;
                let relative = path.strip_prefix(&self.root).map_err(|_| {
                    format!(
                        "captured project source escaped package root {}: {}",
                        self.root.display(),
                        path.display()
                    )
                })?;
                output.push(ProjectSourceFile {
                    relative_path: relative.display().to_string().replace('\\', "/"),
                    contents,
                });
            }
        }
        Ok(output)
    }

    fn collect_paths(
        &self,
        path: &Path,
        excluded: &[PathBuf],
        depth: usize,
        output: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        if depth > self.limits.max_depth {
            return Err(format!(
                "project source tree exceeded depth limit of {} at {}",
                self.limits.max_depth,
                path.display()
            ));
        }
        if excluded.iter().any(|excluded| path == excluded) {
            return Ok(());
        }
        let metadata = project_path_metadata(path, "project source tree")?;
        if metadata.is_file() {
            if is_rsscript_source_path(path) {
                output.push(path.to_path_buf());
            }
            return Ok(());
        }
        if !metadata.is_dir() {
            return Err(format!(
                "project source tree only accepts regular files or directories: {}",
                path.display()
            ));
        }
        let mut entries = fs::read_dir(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_path = entry.path();
            let metadata = project_path_metadata(&entry_path, "project source tree")?;
            if metadata.is_dir() {
                self.collect_paths(&entry_path, excluded, depth + 1, output)?;
            } else if metadata.is_file() && is_rsscript_source_path(&entry_path) {
                output.push(entry_path);
            }
        }
        Ok(())
    }

    fn add_file(&mut self, bytes: u64, path: &Path) -> Result<(), String> {
        if bytes > self.limits.max_file_bytes {
            return Err(format!(
                "project source {} exceeded per-file byte limit of {}",
                path.display(),
                self.limits.max_file_bytes
            ));
        }
        self.files = self.files.saturating_add(1);
        if self.files > self.limits.max_files {
            return Err(format!(
                "project source tree exceeded file limit of {}",
                self.limits.max_files
            ));
        }
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| "project source byte accounting overflowed".to_string())?;
        if self.bytes > self.limits.max_total_bytes {
            return Err(format!(
                "project source tree exceeded total byte limit of {}",
                self.limits.max_total_bytes
            ));
        }
        Ok(())
    }
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
    // Package graphs commonly contain path dependencies beneath the root
    // package (for example `root/deps/helper`). Copying both roots to their
    // absolute mirrored destinations would try to create the helper's files a
    // second time inside the already-copied root. Reuse the parent capture for
    // nested roots instead: the graph still has an exact mapping for each
    // original package root, but every source byte is captured once.
    let mut captured_roots = Vec::<(PathBuf, PathBuf)>::with_capacity(roots.len());
    for (original, root) in roots {
        check_capture_operation(operation)?;
        let destination = captured_roots
            .iter()
            .filter_map(|(ancestor, captured)| {
                root.strip_prefix(ancestor)
                    .ok()
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .map(|relative| (ancestor, captured, relative))
            })
            .max_by_key(|(ancestor, _, _)| ancestor.as_os_str().len())
            .map(|(_, captured, relative)| captured.join(relative));
        let destination = match destination {
            Some(destination) => {
                if !destination.is_dir() {
                    return Err(format!(
                        "nested project graph root was excluded or missing from its parent capture: {}",
                        root.display()
                    ));
                }
                destination
            }
            None => {
                let destination = mirrored_capture_path(&packages_root, &root)?;
                copy_project_directory(&root, &destination, &excluded, operation)?;
                destination
            }
        };
        captured_roots.push((root.clone(), destination.clone()));
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

fn canonical_project_tree_root(path: &Path, operation: &str) -> Result<PathBuf, String> {
    let metadata = project_path_metadata(path, operation)?;
    if !(metadata.is_dir() || metadata.is_file()) {
        return Err(format!(
            "{operation} only accepts regular files or directories: {}",
            path.display()
        ));
    }
    fs::canonicalize(path)
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))
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

fn confined_project_path(root: &Path, value: &str, label: &str) -> Result<PathBuf, String> {
    let relative = Path::new(value);
    if relative.is_absolute() {
        return Err(format!(
            "{label} `{value}` must be relative to the package root."
        ));
    }
    if relative
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "{label} `{value}` must not escape the package root with `..`."
        ));
    }

    let path = root.join(relative);
    if !path.exists() {
        return Ok(path);
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize {label} `{value}`: {error}"))?;
    canonical.strip_prefix(root).map_err(|_| {
        format!(
            "{label} `{value}` resolves outside package root {}.",
            root.display()
        )
    })?;

    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        current.push(component);
        project_path_metadata(&current, label)?;
    }
    Ok(path)
}

pub fn project_path_metadata(path: &Path, label: &str) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label} {}: {error}", path.display()))?;
    if is_link_like(&metadata) {
        return Err(format!(
            "{label} rejects symlinks or reparse points because the path resolves outside the package root: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

/// Open one regular project file while rejecting symlinks and reparse points.
pub fn open_project_regular_file_no_follow(
    path: &Path,
    operation: &str,
) -> Result<(File, fs::Metadata), String> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};

        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            format!(
                "failed to open {} without following links: {error}",
                path.display()
            )
        })?;
        File::from(descriptor)
    };
    #[cfg(not(unix))]
    let file = {
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
                "failed to open {} without following links: {error}",
                path.display()
            )
        })?
    };
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

/// Open a regular file through a checked project-root path walk.
pub fn open_project_regular_file_within_root(
    root: &Path,
    path: &Path,
    operation: &str,
) -> Result<(File, fs::Metadata), String> {
    let relative = confined_project_relative_path(root, path, operation)?;
    if relative.as_os_str().is_empty() {
        return open_project_regular_file_no_follow(path, operation);
    }
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};

        let root_descriptor = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| format!("failed to open project root {}: {error}", root.display()))?;
        let mut current = File::from(root_descriptor);
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(component) = component else {
                return Err(format!(
                    "{operation} path is not confined: {}",
                    path.display()
                ));
            };
            let flags = if components.peek().is_some() {
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            } else {
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            };
            let descriptor = rustix::fs::openat(&current, component, flags, Mode::empty())
                .map_err(|error| {
                    format!(
                        "failed to open confined {operation} path {}: {error}",
                        path.display()
                    )
                })?;
            current = File::from(descriptor);
        }
        let metadata = current
            .metadata()
            .map_err(|error| format!("failed to inspect opened {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "{operation} requires a regular file: {}",
                path.display()
            ));
        }
        Ok((current, metadata))
    }
    #[cfg(not(unix))]
    {
        ensure_project_path_components(root, path, operation)?;
        let opened = open_project_regular_file_no_follow(path, operation)?;
        ensure_project_path_components(root, path, operation)?;
        Ok(opened)
    }
}

/// Read a bounded UTF-8 regular file without following links.
pub fn read_project_utf8_file_bounded(
    path: &Path,
    max_bytes: u64,
    operation: &str,
) -> Result<String, String> {
    let (file, metadata) = open_project_regular_file_no_follow(path, operation)?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "{operation} exceeded byte limit of {max_bytes} at {}",
            path.display()
        ));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        format!(
            "{operation} file is too large for this platform: {}",
            path.display()
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::take(file, max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{operation} exceeded byte limit of {max_bytes} while reading {}",
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        format!(
            "{operation} is not valid UTF-8 at {}: {error}",
            path.display()
        )
    })
}

pub fn ensure_project_path_within_root(
    root: &Path,
    path: &Path,
    operation: &str,
) -> Result<(), String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
    if canonical.strip_prefix(root).is_ok() {
        Ok(())
    } else {
        Err(format!(
            "{operation} path escapes package root: {}",
            path.display()
        ))
    }
}

fn confined_project_relative_path(
    root: &Path,
    path: &Path,
    operation: &str,
) -> Result<PathBuf, String> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Ok(relative.to_path_buf());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{operation} path has no parent: {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{operation} path has no file name: {}", path.display()))?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("failed to canonicalize {}: {error}", parent.display()))?;
    canonical_parent
        .join(name)
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| format!("{operation} path escapes package root: {}", path.display()))
}

#[cfg(not(unix))]
fn ensure_project_path_components(root: &Path, path: &Path, operation: &str) -> Result<(), String> {
    let relative = confined_project_relative_path(root, path, operation)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!(
                "{operation} path is not confined: {}",
                path.display()
            ));
        };
        current.push(component);
        project_path_metadata(&current, operation)?;
    }
    ensure_project_path_within_root(root, path, operation)
}

fn is_rsscript_source_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rss") | Some("rssi")
    )
}

fn read_regular_utf8_within_root(
    root: &Path,
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<(String, u64), String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "{label} path escapes package root {}: {}",
            root.display(),
            path.display()
        )
    })?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "{label} path must contain only normal components: {}",
            path.display()
        ));
    }

    #[cfg(unix)]
    let mut file = {
        use rustix::fs::{Mode, OFlags};

        let root_descriptor = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            format!(
                "failed to open package root {} without following links: {error}",
                root.display()
            )
        })?;
        let mut parent = File::from(root_descriptor);
        let components = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(component) => Some(component),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            if index + 1 < components.len() {
                flags |= OFlags::DIRECTORY;
            }
            let descriptor = rustix::fs::openat(&parent, *component, flags, Mode::empty())
                .map_err(|error| {
                    format!(
                        "failed to open {label} {} without following links: {error}",
                        path.display()
                    )
                })?;
            parent = File::from(descriptor);
        }
        parent
    };
    #[cfg(not(unix))]
    let mut file = {
        project_path_metadata(path, label)?;
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|error| format!("failed to open {label} {}: {error}", path.display()))?
    };

    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {label} {}: {error}", path.display()))?;
    if is_link_like(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{label} must be a regular non-link file: {}",
            path.display()
        ));
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "{label} {} exceeded per-file byte limit of {max_bytes}",
            path.display()
        ));
    }
    let capacity = usize::try_from(metadata.len().min(max_bytes)).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(capacity);
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{label} {} exceeded per-file byte limit of {max_bytes}",
            path.display()
        ));
    }
    let actual = bytes.len() as u64;
    String::from_utf8(bytes)
        .map(|contents| (contents, actual))
        .map_err(|error| {
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
    fn project_boundary_resolves_only_present_local_dependency_roots() {
        let directory = tempfile::tempdir().expect("workspace");
        let package = directory.path().join("package");
        let dependency = directory.path().join("dependency");
        std::fs::create_dir_all(&package).expect("package root");
        std::fs::create_dir_all(&dependency).expect("dependency root");
        std::fs::write(package.join("rsspkg.toml"), "[package]\nname = \"root\"\n")
            .expect("package manifest");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"dependency\"\n",
        )
        .expect("dependency manifest");

        assert_eq!(
            resolve_project_path_dependency(&package, "../dependency")
                .expect("resolve present dependency"),
            Some(
                package
                    .canonicalize()
                    .expect("canonical package")
                    .join("../dependency")
            )
        );
        assert_eq!(
            resolve_project_path_dependency(&package, "../missing")
                .expect("missing dependency is an unresolved package"),
            None
        );
    }

    #[test]
    fn manifest_graph_captures_local_dependencies_once_and_ignores_remote_forms() {
        let directory = tempfile::tempdir().expect("workspace");
        let root = directory.path().join("root");
        let dependency = directory.path().join("dependency");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&dependency).expect("dependency");
        std::fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"root\"\n\n[dependencies]\nlocal = { path = \"../dependency\" }\nregistry = \"1.0\"\nremote = { git = \"https://example.invalid/repo\" }\n",
        )
        .expect("root manifest");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"dependency\"\n\n[dependencies]\nroot = { path = \"../root\" }\n",
        )
        .expect("dependency manifest");

        let graph = capture_project_manifest_graph(
            &root,
            ProjectManifestGraphLimits {
                max_packages: 2,
                max_manifest_bytes: 1024,
                max_total_manifest_bytes: 2048,
            },
        )
        .expect("bounded manifest graph");
        assert_eq!(graph.root(), root.canonicalize().as_deref().expect("root"));
        assert_eq!(graph.packages().len(), 2);
        let root_package = graph.package(&root).expect("root package");
        assert_eq!(root_package.path_dependencies().len(), 1);
        assert_eq!(root_package.path_dependencies()[0].name(), "local");
        assert_eq!(
            root_package.path_dependencies()[0].section(),
            WorkspaceDependencySection::Dependencies
        );
        assert!(graph.package(&dependency).is_some());
    }

    #[test]
    fn manifest_graph_enforces_total_manifest_budget() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"root\"\n",
        )
        .expect("manifest");
        let error = capture_project_manifest_graph(
            directory.path(),
            ProjectManifestGraphLimits {
                max_packages: 1,
                max_manifest_bytes: 1024,
                max_total_manifest_bytes: 1,
            },
        )
        .expect_err("aggregate manifest budget must be enforced");
        assert!(error.contains("total manifest byte limit"), "{error}");
    }

    #[test]
    fn optional_project_utf8_returns_none_for_missing_and_reads_confined_file() {
        let directory = tempfile::tempdir().expect("workspace");
        assert_eq!(
            capture_optional_project_utf8(directory.path(), "identity", 1024, "identity")
                .expect("missing optional file"),
            None
        );
        std::fs::write(directory.path().join("identity"), "captured").expect("identity file");
        assert_eq!(
            capture_optional_project_utf8(directory.path(), "identity", 1024, "identity")
                .expect("captured optional file"),
            Some("captured".to_string())
        );
    }

    #[test]
    fn project_source_capture_is_confined_bounded_and_excludes_manifest_roots() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(directory.path().join("src/ignored")).expect("source tree");
        std::fs::write(
            directory.path().join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("source");
        std::fs::write(directory.path().join("src/api.rssi"), "module api\n").expect("interface");
        std::fs::write(directory.path().join("src/ignored/hidden.rss"), "hidden")
            .expect("excluded source");
        std::fs::write(directory.path().join("src/readme.txt"), "ignored").expect("non-source");

        let mut capture = ProjectSourceCapture::new(
            directory.path(),
            ProjectSourceCaptureLimits {
                max_files: 2,
                max_total_bytes: 1024,
                max_file_bytes: 1024,
                max_depth: 8,
            },
        )
        .expect("capture boundary");
        let files = capture
            .capture(&["src".to_string()], &["src/ignored".to_string()])
            .expect("bounded capture");
        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_path())
                .collect::<Vec<_>>(),
            vec!["src/api.rssi", "src/main.rss"]
        );
        assert!(capture.capture(&["../outside".to_string()], &[]).is_err());
    }

    #[test]
    fn project_tree_scan_is_bounded_sorted_and_policy_filtered() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(directory.path().join("nested")).expect("tree");
        std::fs::write(directory.path().join("z.txt"), "z").expect("source");
        std::fs::write(directory.path().join("a.txt"), "a").expect("source");
        std::fs::write(directory.path().join("nested/keep.txt"), "keep").expect("source");
        std::fs::write(directory.path().join("nested/skip.txt"), "skip").expect("source");

        let files = collect_project_regular_files(
            directory.path(),
            ProjectTreeLimits {
                max_files: 3,
                max_entries: 8,
                max_bytes: 32,
                max_depth: 4,
            },
            "project scan test",
            |_, name| name == "skip.txt",
        )
        .expect("bounded tree scan");
        assert_eq!(
            files
                .iter()
                .map(|file| {
                    file.path
                        .strip_prefix(directory.path())
                        .expect("project-relative output")
                        .display()
                        .to_string()
                })
                .collect::<Vec<_>>(),
            vec!["a.txt", "nested/keep.txt", "z.txt"]
        );
        let error = collect_project_regular_files(
            directory.path(),
            ProjectTreeLimits {
                max_files: 2,
                max_entries: 8,
                max_bytes: 32,
                max_depth: 4,
            },
            "project scan test",
            |_, name| name == "skip.txt",
        )
        .expect_err("file limit must apply before a fourth accepted file");
        assert!(error.contains("file count limit"), "{error}");
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

    #[test]
    fn project_graph_capture_reuses_parent_snapshot_for_nested_roots() {
        let directory = tempfile::tempdir().expect("workspace");
        let root = directory.path().join("root");
        let dependency = root.join("deps/helper");
        std::fs::create_dir_all(dependency.join("interface")).expect("dependency directories");
        std::fs::write(root.join("rsspkg.toml"), "[package]\nname = 'root'\n")
            .expect("root manifest");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = 'helper'\n",
        )
        .expect("dependency manifest");
        std::fs::write(
            dependency.join("interface/lib.rssi"),
            "pub fn Helper.value() -> Int\n",
        )
        .expect("dependency interface");

        let graph = capture_project_graph(
            [dependency.clone(), root.clone()],
            std::iter::empty::<&str>(),
            None,
        )
        .expect("nested package roots should share one private capture");
        let captured_root = graph.captured_path(&root).expect("root mapping");
        let captured_dependency = graph
            .captured_path(&dependency)
            .expect("dependency mapping");
        assert_eq!(captured_dependency, captured_root.join("deps/helper"));
        assert_eq!(
            graph
                .read_captured_utf8(&dependency, Path::new("interface/lib.rssi"), 1024)
                .expect("nested dependency contents"),
            "pub fn Helper.value() -> Int\n"
        );
        assert_eq!(
            graph.original_path(&captured_dependency.join("interface/lib.rssi")),
            Some(dependency.join("interface/lib.rssi"))
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
