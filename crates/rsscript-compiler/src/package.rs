#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::derivable_impls,
    clippy::doc_lazy_continuation,
    clippy::if_same_then_else,
    clippy::items_after_test_module,
    clippy::let_and_return,
    clippy::manual_contains,
    clippy::manual_slice_fill,
    clippy::mutable_key_type,
    clippy::needless_borrow,
    clippy::needless_lifetimes,
    clippy::needless_range_loop,
    clippy::nonminimal_bool,
    clippy::op_ref,
    clippy::ptr_arg,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_lazy_evaluations,
    clippy::useless_conversion
)]
// Compatibility package/review tooling keeps its lint debt local to this module.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::diagnostic::Diagnostic;

mod analysis;
#[path = "package/review/review_await.rs"]
mod analysis_await;
mod analysis_execution;
mod artifact_store;
mod authorization;
mod check;
mod contract;
mod dependency;
mod diff;
mod format;
mod graph;
mod lock;
mod metadata;
mod native;
mod policy;
mod review;
mod source_set;
mod types;

pub const PACKAGE_REVIEW_METADATA_SCHEMA: &str = "rsscript.package_review.v1";

const PACKAGE_TREE_MAX_FILES: usize = 20_000;
const PACKAGE_TREE_MAX_ENTRIES: usize = 40_000;
const PACKAGE_TREE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PACKAGE_TREE_MAX_DEPTH: usize = 64;
const PACKAGE_MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

pub use analysis::analyze_package_dir;
pub use artifact_store::ArtifactStore;
pub use authorization::{
    ExecutablePackageSnapshot, PreparedPackage, WorkspaceSnapshot, load_workspace_snapshot,
    load_workspace_snapshot_with_operation, prepare_executable_package,
    prepare_package_for_execution,
};
pub use check::check_package_dir;
use dependency::{
    PackageDependencySpec, collect_dependency_interface_sources,
    collect_dependency_interface_sources_for_tests, collect_dependency_lowering_sources,
    package_dependency_spec, package_feature_resolution_diagnostics,
};
pub use diff::diff_package_dirs;
pub use format::*;
pub use graph::package_tree;
pub use lock::{diff_package_locks, lock_package_dir};
pub use metadata::{package_lowering_input, package_metadata, package_metadata_verify};
pub(crate) use native::package_native_plugin_build_dependencies;
use native::{manifest_native_enabled, manifest_native_unsafe_boundary};
pub use review::review_package_dir;
use source_set::{LoadedPackage, Manifest, ManifestNativeRust, PackageSource};
pub use types::*;

pub fn package_sources(package_dir: &Path) -> Result<Vec<PackageSourceFile>, String> {
    let package = source_set::load_package(package_dir)?;
    Ok(package_source_files(package.sources))
}

pub fn package_sources_with_dependency_interfaces(
    package_dir: &Path,
) -> Result<Vec<PackageSourceFile>, String> {
    let package = source_set::load_package(package_dir)?;
    let mut sources = package.sources;
    sources.extend(collect_dependency_interface_sources(
        package_dir,
        &package.manifest,
    )?);
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(package_source_files(sources))
}

fn package_source_files(sources: Vec<PackageSource>) -> Vec<PackageSourceFile> {
    sources
        .into_iter()
        .map(|source| PackageSourceFile {
            path: source.path,
            relative_path: source.relative_path,
            contents: source.contents,
            kind: source.kind,
        })
        .collect()
}

fn relative_path(base: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(base)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            let canonical_base = base.canonicalize().ok()?;
            let canonical_path = path.canonicalize().ok()?;
            canonical_path
                .strip_prefix(canonical_base)
                .ok()
                .map(Path::to_path_buf)
        });
    relative
        .as_deref()
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

fn collect_regular_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let root = canonical_checked_root(path, "package file scan")?;
    let mut budget = TreeBudget::default();
    collect_regular_files_inner(&root, path, files, 0, &mut budget)
}

fn collect_regular_files_inner(
    root: &Path,
    path: &Path,
    files: &mut Vec<PathBuf>,
    depth: usize,
    budget: &mut TreeBudget,
) -> Result<(), String> {
    budget.check_depth(depth, "package file scan", path)?;
    let metadata = package_path_metadata(path, "package file scan")?;
    ensure_package_path_within_root(root, path, "package file scan")?;
    if metadata.is_file() {
        let (_file, opened_metadata) =
            open_regular_file_within_root(root, path, "package file scan")?;
        budget.add_file(opened_metadata.len(), "package file scan", path)?;
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let entries = read_bounded_sorted_entries(path, "package file scan", budget)?;
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if matches!(
            name.to_string_lossy().as_ref(),
            "target" | ".git" | ".DS_Store" | "Cargo.lock"
        ) {
            continue;
        }
        collect_regular_files_inner(root, &path, files, depth + 1, budget)?;
    }
    Ok(())
}

fn copy_package_directory(source: &Path, destination: &Path) -> Result<(), String> {
    copy_package_directory_with_limits(source, destination, TreeLimits::default())
}

fn copy_package_directory_with_operation(
    source: &Path,
    destination: &Path,
    operation: &rsscript_operation::OperationContext,
) -> Result<(), String> {
    let root = canonical_checked_root(source, "package copy")?;
    let mut budget = TreeBudget::with_operation(TreeLimits::default(), operation);
    copy_package_directory_inner(&root, source, destination, 0, &mut budget)
}

fn copy_package_directory_with_limits(
    source: &Path,
    destination: &Path,
    limits: TreeLimits,
) -> Result<(), String> {
    let root = canonical_checked_root(source, "package copy")?;
    let mut budget = TreeBudget::with_limits(limits);
    copy_package_directory_inner(&root, source, destination, 0, &mut budget)
}

fn copy_package_directory_inner(
    root: &Path,
    source: &Path,
    destination: &Path,
    depth: usize,
    budget: &mut TreeBudget,
) -> Result<(), String> {
    budget.check_depth(depth, "package copy", source)?;
    let metadata = package_path_metadata(source, "package copy")?;
    ensure_package_path_within_root(root, source, "package copy")?;
    if metadata.is_file() {
        let (input, opened_metadata) = open_regular_file_within_root(root, source, "package copy")?;
        budget.add_file(opened_metadata.len(), "package copy", source)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        copy_regular_file_bounded(input, source, destination, opened_metadata.len())?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let entries = read_bounded_sorted_entries(source, "package copy", budget)?;
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if should_skip_vendor_copy_entry(&name.to_string_lossy()) {
            continue;
        }
        let target = destination.join(name);
        copy_package_directory_inner(root, &path, &target, depth + 1, budget)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct TreeLimits {
    pub max_files: usize,
    pub max_entries: usize,
    pub max_bytes: u64,
    pub max_depth: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_files: PACKAGE_TREE_MAX_FILES,
            max_entries: PACKAGE_TREE_MAX_ENTRIES,
            max_bytes: PACKAGE_TREE_MAX_BYTES,
            max_depth: PACKAGE_TREE_MAX_DEPTH,
        }
    }
}

#[derive(Debug, Default)]
struct TreeBudget {
    limits: TreeLimits,
    files: usize,
    entries: usize,
    bytes: u64,
    operation: Option<rsscript_operation::OperationContext>,
}

impl TreeBudget {
    fn with_limits(limits: TreeLimits) -> Self {
        Self {
            limits,
            files: 0,
            entries: 0,
            bytes: 0,
            operation: None,
        }
    }

    fn with_operation(
        limits: TreeLimits,
        operation: &rsscript_operation::OperationContext,
    ) -> Self {
        Self {
            limits,
            files: 0,
            entries: 0,
            bytes: 0,
            operation: Some(operation.clone()),
        }
    }

    fn check_operation(&self) -> Result<(), String> {
        self.operation.as_ref().map_or(Ok(()), |operation| {
            operation
                .check()
                .map_err(|abort| format!("package operation stopped: {abort:?}"))
        })
    }

    fn check_depth(&self, depth: usize, operation: &str, path: &Path) -> Result<(), String> {
        self.check_operation()?;
        if depth > self.limits.max_depth {
            return Err(format!(
                "{operation} exceeded directory depth limit of {} at {}",
                self.limits.max_depth,
                path.display()
            ));
        }
        Ok(())
    }

    fn add_file(&mut self, bytes: u64, operation: &str, path: &Path) -> Result<(), String> {
        self.check_operation()?;
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
        if self.files > self.limits.max_files {
            return Err(format!(
                "{operation} exceeded file count limit of {} at {}",
                self.limits.max_files,
                path.display()
            ));
        }
        if self.bytes > self.limits.max_bytes {
            return Err(format!(
                "{operation} exceeded total byte limit of {} at {}",
                self.limits.max_bytes,
                path.display()
            ));
        }
        Ok(())
    }

    fn add_entry(&mut self, operation: &str, path: &Path) -> Result<(), String> {
        self.check_operation()?;
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            format!(
                "{operation} directory entry count overflow while visiting {}",
                path.display()
            )
        })?;
        if self.entries > self.limits.max_entries {
            return Err(format!(
                "{operation} exceeded directory entry limit of {} at {}",
                self.limits.max_entries,
                path.display()
            ));
        }
        Ok(())
    }
}

fn read_bounded_sorted_entries(
    path: &Path,
    operation: &str,
    budget: &mut TreeBudget,
) -> Result<Vec<fs::DirEntry>, String> {
    let entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut bounded = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        budget.add_entry(operation, &entry.path())?;
        bounded.push(entry);
    }
    bounded.sort_by_key(|entry| entry.file_name());
    Ok(bounded)
}

fn copy_regular_file_bounded(
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
    let output_metadata = output
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", destination.display()))?;
    if !output_metadata.is_file() || is_package_link_like(&output_metadata) {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "package copy destination is not a regular file: {}",
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
    if copied > expected {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "package copy source grew while being copied: {}",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundedRegularFile {
    pub path: PathBuf,
    pub bytes: u64,
}

pub(crate) fn collect_bounded_regular_files(
    path: &Path,
    limits: TreeLimits,
    operation: &str,
    skip: impl Fn(&Path, &fs::DirEntry) -> bool,
) -> Result<Vec<BoundedRegularFile>, String> {
    fn visit(
        root: &Path,
        path: &Path,
        depth: usize,
        operation: &str,
        skip: &impl Fn(&Path, &fs::DirEntry) -> bool,
        budget: &mut TreeBudget,
        files: &mut Vec<BoundedRegularFile>,
    ) -> Result<(), String> {
        budget.check_depth(depth, operation, path)?;
        let metadata = package_path_metadata(path, operation)?;
        ensure_package_path_within_root(root, path, operation)?;
        if metadata.is_file() {
            let (_file, opened_metadata) = open_regular_file_within_root(root, path, operation)?;
            budget.add_file(opened_metadata.len(), operation, path)?;
            files.push(BoundedRegularFile {
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
        let entries = read_bounded_sorted_entries(path, operation, budget)?;
        for entry in entries {
            if skip(path, &entry) {
                continue;
            }
            visit(
                root,
                &entry.path(),
                depth + 1,
                operation,
                skip,
                budget,
                files,
            )?;
        }
        Ok(())
    }

    let root = canonical_checked_root(path, operation)?;
    let mut files = Vec::new();
    visit(
        &root,
        path,
        0,
        operation,
        &skip,
        &mut TreeBudget::with_limits(limits),
        &mut files,
    )?;
    Ok(files)
}

fn canonical_checked_root(path: &Path, operation: &str) -> Result<PathBuf, String> {
    let metadata = package_path_metadata(path, operation)?;
    if !(metadata.is_dir() || metadata.is_file()) {
        return Err(format!(
            "{operation} only accepts regular files or directories: {}",
            path.display()
        ));
    }
    fs::canonicalize(path)
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))
}

/// Atomically replace a regular package artifact without following symlinks in
/// its parent path or at the destination.
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

pub(super) fn package_path_metadata(path: &Path, operation: &str) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if is_package_link_like(&metadata) {
        return Err(format!(
            "{operation} rejects symlinks or reparse points because the path resolves outside the package root: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

pub(super) fn open_regular_file_no_follow(
    path: &Path,
    operation: &str,
) -> Result<(File, fs::Metadata), String> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};

        let fd = rustix::fs::open(
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
        File::from(fd)
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
    if !metadata.is_file() || is_package_link_like(&metadata) {
        return Err(format!(
            "{operation} requires a regular file and rejects symlinks or reparse points: {}",
            path.display()
        ));
    }
    Ok((file, metadata))
}

pub(super) fn open_regular_file_within_root(
    root: &Path,
    path: &Path,
    operation: &str,
) -> Result<(File, fs::Metadata), String> {
    let relative = confined_relative_path(root, path, operation)?;
    if relative.as_os_str().is_empty() {
        return open_regular_file_no_follow(path, operation);
    }
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};

        let root_fd = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| format!("failed to open package root {}: {error}", root.display()))?;
        let mut current = File::from(root_fd);
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
            let fd =
                rustix::fs::openat(&current, component, flags, Mode::empty()).map_err(|error| {
                    format!(
                        "failed to open confined {operation} path {}: {error}",
                        path.display()
                    )
                })?;
            current = File::from(fd);
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
        ensure_package_path_components(root, path, operation)?;
        let opened = open_regular_file_no_follow(path, operation)?;
        ensure_package_path_components(root, path, operation)?;
        Ok(opened)
    }
}

fn confined_relative_path(root: &Path, path: &Path, operation: &str) -> Result<PathBuf, String> {
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
fn ensure_package_path_components(root: &Path, path: &Path, operation: &str) -> Result<(), String> {
    let relative = confined_relative_path(root, path, operation)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!(
                "{operation} path is not confined: {}",
                path.display()
            ));
        };
        current.push(component);
        package_path_metadata(&current, operation)?;
    }
    ensure_package_path_within_root(root, path, operation)
}

pub(super) fn is_package_link_like(metadata: &fs::Metadata) -> bool {
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

pub(crate) fn read_utf8_file_bounded(
    path: &Path,
    max_bytes: u64,
    operation: &str,
) -> Result<String, String> {
    let (file, metadata) = open_regular_file_no_follow(path, operation)?;
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

pub(super) fn ensure_package_path_within_root(
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

fn should_skip_vendor_copy_entry(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "vendor"
            | ".rsscript-artifacts.lock"
            | source_set::SNAPSHOT_MANIFEST_SOURCE_FILE
    )
}

pub(super) fn dedup_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.code.clone(),
            diagnostic.summary.clone(),
            diagnostic.span.file.clone(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.span.length,
        ))
    });
}

fn canonical_path_label(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn normalized_path_label(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        ".".to_string()
    } else {
        normalized.display().to_string()
    }
}

fn package_path_source(path: &Path) -> String {
    format!("path+{}", normalized_path_label(path))
}

fn is_rsscript_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rss" | "rssi"))
}

fn package_manifest_key_span(package_dir: &Path, key: &str) -> crate::diagnostic::Span {
    let path = package_dir.join("rsspkg.toml");
    let file = path.display().to_string();
    let source = read_utf8_file_bounded(
        &path,
        PACKAGE_MANIFEST_MAX_BYTES,
        "package manifest diagnostic read",
    )
    .unwrap_or_default();
    for (index, line) in source.lines().enumerate() {
        if let Some(column) = line.find(key) {
            return crate::diagnostic::Span {
                file,
                line: index + 1,
                column: column + 1,
                length: key.len().max(1),
            };
        }
    }
    crate::diagnostic::Span {
        file,
        line: 1,
        column: 1,
        length: key.len().max(1),
    }
}

fn package_dependency_span(package_dir: &Path, dependency: &str) -> crate::diagnostic::Span {
    package_manifest_key_span(package_dir, dependency)
}

fn collect_package_feature_boundary_reasons(
    features: &BTreeMap<String, Vec<String>>,
    reasons: &mut Vec<String>,
) {
    for (name, values) in features {
        if package_feature_may_change_boundary_risk(name, values) {
            reasons.push(format!(
                "package feature `{name}` may change native/unsafe/build risk"
            ));
        }
    }
}

fn package_feature_may_change_boundary_risk(name: &str, values: &[String]) -> bool {
    package_feature_token_is_boundary_risk(name)
        || values
            .iter()
            .any(|value| package_feature_token_is_boundary_risk(value))
}

fn package_feature_token_is_boundary_risk(token: &str) -> bool {
    let normalized = token.to_ascii_lowercase();
    ["native", "unsafe", "ffi", "build", "proc", "macro", "link"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn package_identity(manifest: &Manifest) -> PackageIdentity {
    PackageIdentity {
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        edition: manifest.package.edition.clone(),
    }
}

fn toml_value_label(value: &toml::Value) -> String {
    value.to_string()
}

fn feature_values_label(values: &[String]) -> String {
    if values.is_empty() {
        "[]".to_string()
    } else {
        values.join(", ")
    }
}

fn package_risk_label(risk: PackageRisk) -> &'static str {
    match risk {
        PackageRisk::Low => "low",
        PackageRisk::Elevated => "elevated",
        PackageRisk::High => "high",
        PackageRisk::Unknown => "unknown",
    }
}

#[cfg(test)]
mod preparation_limit_tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rss-package-preparation-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn bounded_tree_scan_enforces_file_byte_and_depth_limits() {
        let root = test_dir("tree-limits");
        fs::create_dir_all(root.join("one/two")).expect("fixture directories");
        fs::write(root.join("a"), b"1234").expect("first fixture");
        fs::write(root.join("one/b"), b"5678").expect("second fixture");
        fs::write(root.join("one/two/c"), b"9").expect("deep fixture");

        let file_error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 2,
                max_entries: 10,
                max_bytes: 100,
                max_depth: 10,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("third file must exceed file budget");
        assert!(file_error.contains("file count limit"), "{file_error}");

        let byte_error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 10,
                max_entries: 10,
                max_bytes: 7,
                max_depth: 10,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("eight bytes must exceed byte budget");
        assert!(byte_error.contains("total byte limit"), "{byte_error}");

        let depth_error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 10,
                max_entries: 10,
                max_bytes: 100,
                max_depth: 1,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("nested fixture must exceed depth budget");
        assert!(
            depth_error.contains("directory depth limit"),
            "{depth_error}"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn bounded_tree_scan_counts_directory_entries_before_sorting() {
        let root = test_dir("entry-limits");
        fs::create_dir_all(root.join("z")).expect("first directory");
        fs::create_dir_all(root.join("a")).expect("second directory");
        fs::create_dir_all(root.join("m")).expect("third directory");

        let error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 10,
                max_entries: 2,
                max_bytes: 100,
                max_depth: 10,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("third directory entry must exceed entry budget");
        assert!(error.contains("directory entry limit"), "{error}");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn bounded_utf8_read_rejects_oversized_input_before_reading_it_whole() {
        let root = test_dir("bounded-text");
        fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("Cargo.toml");
        fs::write(&path, b"12345").expect("fixture file");

        let error = read_utf8_file_bounded(&path, 4, "test manifest read")
            .expect_err("oversized manifest must be rejected");
        assert!(error.contains("byte limit of 4"), "{error}");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn package_artifact_write_replaces_regular_files_atomically() {
        let root = test_dir("artifact-write");
        fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("rsspkg.lock");
        fs::write(&path, b"old").expect("old fixture");

        write_package_artifact_atomic(&root, &path, b"new", "test package lock")
            .expect("regular artifact should update");

        assert_eq!(fs::read(&path).expect("artifact should read"), b"new");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn package_artifact_write_rejects_symlink_destinations() {
        use std::os::unix::fs::symlink;

        let root = test_dir("artifact-symlink");
        let outside = test_dir("artifact-outside");
        fs::create_dir_all(&root).expect("fixture directory");
        fs::write(&outside, b"outside").expect("outside fixture");
        let path = root.join("rsspkg.lock");
        symlink(&outside, &path).expect("fixture symlink");

        let error = write_package_artifact_atomic(&root, &path, b"new", "test package lock")
            .expect_err("symlink artifact must be rejected");

        assert!(error.contains("not a symlink"), "{error}");
        assert_eq!(fs::read(&outside).expect("outside should read"), b"outside");
        fs::remove_dir_all(root).expect("fixture cleanup");
        fs::remove_file(outside).expect("outside cleanup");
    }

    #[test]
    fn package_copy_fails_before_copying_file_beyond_budget() {
        let source = test_dir("copy-source");
        let destination = test_dir("copy-destination");
        fs::create_dir_all(&source).expect("source directory");
        fs::write(source.join("large"), b"12345").expect("source file");

        let error = copy_package_directory_with_limits(
            &source,
            &destination,
            TreeLimits {
                max_files: 10,
                max_entries: 10,
                max_bytes: 4,
                max_depth: 10,
            },
        )
        .expect_err("oversized package copy must fail");
        assert!(error.contains("total byte limit"), "{error}");
        assert!(!destination.join("large").exists());
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(destination);
    }

    #[cfg(unix)]
    #[test]
    fn package_copy_does_not_follow_substituted_destination_file() {
        use std::os::unix::fs::symlink;

        let source = test_dir("copy-link-source");
        let destination = test_dir("copy-link-destination");
        let outside = test_dir("copy-link-outside");
        fs::create_dir_all(&source).expect("source directory");
        fs::create_dir_all(&destination).expect("destination directory");
        fs::write(source.join("file"), b"inside").expect("source file");
        fs::write(&outside, b"outside").expect("outside file");
        symlink(&outside, destination.join("file")).expect("destination symlink");

        copy_package_directory(&source, &destination)
            .expect_err("copy destination symlink must be rejected");
        assert_eq!(fs::read(&outside).expect("outside read"), b"outside");
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(destination);
        let _ = fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn bounded_tree_scan_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = test_dir("tree-symlink");
        fs::create_dir_all(&root).expect("fixture directory");
        fs::write(root.join("target"), b"x").expect("target fixture");
        symlink(root.join("target"), root.join("link")).expect("fixture symlink");

        let error =
            collect_bounded_regular_files(&root, TreeLimits::default(), "test scan", |_, _| false)
                .expect_err("symlink must be rejected");
        assert!(error.contains("rejects symlinks"), "{error}");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
