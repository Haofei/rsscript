use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use rsscript_project::{CapturedProjectGraph, capture_project_graph};
use sha2::{Digest, Sha256};

use super::analysis::analyze_package_dir_captured;
use super::check::check_package_dir_captured;
use super::dependency::{DependencyResolutionScope, resolve_dependency_graph};
use super::{
    NativePluginBuildDependency, PackageAnalysis, PackageCheck, PackageLock, PackageLoweringInput,
    PackageReview, PackageTree, PackageTreeNode, TreeLimits, collect_bounded_regular_files,
    package_lock_toml, package_lowering_input, package_native_plugin_build_dependencies,
    package_path_source,
};

#[derive(Debug)]
pub(super) struct PackageGraphSnapshot {
    captured: CapturedProjectGraph,
    root: PathBuf,
}

impl PackageGraphSnapshot {
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn original_path(&self, snapshot_path: &Path) -> Option<PathBuf> {
        self.captured.original_path(snapshot_path)
    }

    fn remap_path_label(&self, value: &str) -> String {
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

    pub(super) fn remap_error(&self, error: String) -> String {
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

    fn remap_span(&self, span: &mut crate::diagnostic::Span) {
        span.file = self.remap_path_label(&span.file);
    }

    fn remap_diagnostic(&self, diagnostic: &mut crate::diagnostic::Diagnostic) {
        self.remap_span(&mut diagnostic.span);
        for fix in &mut diagnostic.fixes {
            if let Some(edit) = &mut fix.edit {
                self.remap_span(&mut edit.span);
            }
        }
    }

    pub(super) fn remap_review(&self, review: &mut PackageReview) {
        review.manifest_path = self.remap_path_label(&review.manifest_path);
        for dependency in &mut review.dependencies {
            dependency.source = self.remap_path_label(&dependency.source);
        }
        for file in &mut review.files {
            file.path = self.remap_path_label(&file.path);
        }
        for external_binding in &mut review.external_bindings {
            if let Some(span) = &mut external_binding.span {
                self.remap_span(span);
            }
        }
        for await_site in &mut review.await_sites {
            self.remap_span(&mut await_site.span);
        }
        for file in &mut review.review_map.files {
            file.file = self.remap_path_label(&file.file);
        }
        for module in &mut review.review_map.modules {
            module.file = self.remap_path_label(&module.file);
        }
        for diagnostic in &mut review.diagnostics {
            self.remap_diagnostic(diagnostic);
        }
    }

    pub(super) fn remap_analysis(&self, analysis: &mut PackageAnalysis) {
        for file in &mut analysis.files {
            file.path = self.remap_path_label(&file.path);
        }
        for external_import in &mut analysis.external_imports {
            if let Some(span) = &mut external_import.span {
                self.remap_span(span);
            }
        }
        for await_site in &mut analysis.await_sites {
            self.remap_span(&mut await_site.span);
        }
        for diagnostic in &mut analysis.diagnostics {
            self.remap_diagnostic(diagnostic);
        }
    }

    pub(super) fn remap_lock(&self, lock: &mut PackageLock) {
        for package in &mut lock.packages {
            package.source = self.remap_path_label(&package.source);
        }
    }

    pub(super) fn remap_tree(&self, tree: &mut PackageTree) {
        self.remap_tree_node(&mut tree.root);
    }

    fn remap_tree_node(&self, node: &mut PackageTreeNode) {
        node.source = self.remap_path_label(&node.source);
        for dependency in &mut node.dependencies {
            self.remap_tree_node(dependency);
        }
    }

    pub(super) fn remap_check(&self, check: &mut PackageCheck) {
        check.package_dir = self.remap_path_label(&check.package_dir);
        check.lock.path = self.remap_path_label(&check.lock.path);
        for change in &mut check.lock.package_changes {
            for field in &mut change.changes {
                if field.field == "source" {
                    field.before = field
                        .before
                        .as_deref()
                        .map(|value| self.remap_path_label(value));
                    field.after = field
                        .after
                        .as_deref()
                        .map(|value| self.remap_path_label(value));
                }
            }
        }
        for diagnostic in &mut check.diagnostics {
            self.remap_diagnostic(diagnostic);
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
struct PrivateContentSnapshot {
    root: PathBuf,
    native_abi_path: PathBuf,
}

#[allow(dead_code)]
impl PrivateContentSnapshot {
    fn root(&self) -> &Path {
        &self.root
    }
}

/// A package whose review, lock, dependency graph, and native policy checks
/// succeeded.
///
/// Values can only be created by authorizing a [`PreparedPackage`] or through
/// [`prepare_executable_package`]. Native build and load code consumes this type
/// instead of accepting an unchecked path.
#[derive(Debug)]
pub struct ExecutablePackageSnapshot {
    package_dir: PathBuf,
    lowering_input: PackageLoweringInput,
    #[allow(dead_code)]
    native_build_dependencies: Vec<NativePluginBuildDependency>,
    _package_snapshot: PackageGraphSnapshot,
    #[allow(dead_code)]
    content_snapshot: Option<PrivateContentSnapshot>,
}

/// An immutable package dependency graph prepared for lowering or review.
///
/// Pure packages can be consumed directly. Packages with native dependencies
/// must be converted into an [`ExecutablePackageSnapshot`] before lowering or loading.
#[derive(Debug)]
pub struct PreparedPackage {
    package_dir: PathBuf,
    lowering_input: PackageLoweringInput,
    package_snapshot: PackageGraphSnapshot,
}

/// One immutable package graph used by analysis and compilation.
///
/// The private temporary directory keeps every captured file alive for the
/// lifetime of the value. Consumers receive semantic data and lowering input,
/// never a mutable checkout path.
#[derive(Debug)]
pub struct WorkspaceSnapshot {
    package_dir: PathBuf,
    lowering_input: PackageLoweringInput,
    analysis: PackageAnalysis,
    digest: String,
    _package_snapshot: PackageGraphSnapshot,
}

impl WorkspaceSnapshot {
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    pub fn lowering_input(&self) -> &PackageLoweringInput {
        &self.lowering_input
    }

    pub fn analysis(&self) -> &PackageAnalysis {
        &self.analysis
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Capture a package graph once and derive analysis and lowering inputs from
/// that same immutable content.
pub fn load_workspace_snapshot(package_dir: &Path) -> Result<WorkspaceSnapshot, String> {
    load_workspace_snapshot_inner(package_dir, None)
}

pub fn load_workspace_snapshot_with_operation(
    package_dir: &Path,
    operation: &rsscript_operation::OperationContext,
) -> Result<WorkspaceSnapshot, String> {
    load_workspace_snapshot_inner(package_dir, Some(operation))
}

fn load_workspace_snapshot_inner(
    package_dir: &Path,
    operation: Option<&rsscript_operation::OperationContext>,
) -> Result<WorkspaceSnapshot, String> {
    check_operation(operation)?;
    let package_dir = package_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize workspace before content snapshot {}: {error}",
            package_dir.display()
        )
    })?;
    let package_snapshot = snapshot_package_graph_inputs_inner(&package_dir, operation)?;
    check_operation(operation)?;
    let lowering_input = package_lowering_input(package_snapshot.root())?;
    check_operation(operation)?;
    let digest = lowering_input_digest(&lowering_input);
    let mut analysis = analyze_package_dir_captured(package_snapshot.root())
        .map_err(|error| package_snapshot.remap_error(error))?;
    check_operation(operation)?;
    package_snapshot.remap_analysis(&mut analysis);
    analysis.snapshot_digest = digest.clone();
    Ok(WorkspaceSnapshot {
        package_dir,
        lowering_input,
        analysis,
        digest,
        _package_snapshot: package_snapshot,
    })
}

fn check_operation(operation: Option<&rsscript_operation::OperationContext>) -> Result<(), String> {
    operation.map_or(Ok(()), |operation| {
        operation
            .check()
            .map_err(|abort| format!("workspace snapshot stopped: {abort:?}"))
    })
}

fn lowering_input_digest(input: &PackageLoweringInput) -> String {
    let mut hasher = Sha256::new();
    for value in [
        input.package.name.as_str(),
        input.package.version.as_str(),
        input.package.edition.as_str(),
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    for (kind, files) in [(b'S', &input.sources), (b'I', &input.interfaces)] {
        for (path, contents) in files {
            hasher.update([kind]);
            hasher.update((path.len() as u64).to_be_bytes());
            hasher.update(path.as_bytes());
            hasher.update((contents.len() as u64).to_be_bytes());
            hasher.update(contents.as_bytes());
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

impl PreparedPackage {
    /// Return whether this graph must pass external provider verification before use.
    pub fn requires_external_provider(&self) -> bool {
        !self.lowering_input.native_dependencies.is_empty()
    }

    /// Consume a pure package graph and return its captured lowering input.
    pub fn into_lowering_input(self) -> Result<PackageLoweringInput, String> {
        if self.requires_external_provider() {
            return Err(
                "native package execution requires an ExecutablePackageSnapshot; explicitly authorize the prepared snapshot before lowering".to_string(),
            );
        }
        Ok(self.lowering_input)
    }

    /// Review and authorize the captured graph for native build and loading.
    pub fn verify(self) -> Result<ExecutablePackageSnapshot, String> {
        verify_prepared_package(self)
    }
}

impl ExecutablePackageSnapshot {
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    /// Return the checked lowering snapshot captured during snapshot verification.
    ///
    /// AOT callers must use this snapshot rather than re-reading the package
    /// path after authorization.
    pub fn lowering_input(&self) -> &PackageLoweringInput {
        &self.lowering_input
    }

    #[allow(dead_code)]
    pub(crate) fn native_build_dependencies(&self) -> &[NativePluginBuildDependency] {
        &self.native_build_dependencies
    }

    #[allow(dead_code)]
    pub(crate) fn native_snapshot_root(&self) -> Option<&Path> {
        self.content_snapshot
            .as_ref()
            .map(PrivateContentSnapshot::root)
    }

    #[allow(dead_code)]
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
pub fn prepare_executable_package(package_dir: &Path) -> Result<ExecutablePackageSnapshot, String> {
    prepare_package_for_execution(package_dir)?.verify()
}

/// Capture a complete package dependency graph before lowering or review.
pub fn prepare_package_for_execution(package_dir: &Path) -> Result<PreparedPackage, String> {
    let package_dir = package_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize package before content snapshot {}: {error}",
            package_dir.display()
        )
    })?;
    let package_snapshot = snapshot_package_graph_inputs(&package_dir)?;
    let lowering_input = package_lowering_input(package_snapshot.root())?;
    Ok(PreparedPackage {
        package_dir,
        lowering_input,
        package_snapshot,
    })
}

fn verify_prepared_package(prepared: PreparedPackage) -> Result<ExecutablePackageSnapshot, String> {
    let PreparedPackage {
        package_dir,
        mut lowering_input,
        package_snapshot,
    } = prepared;
    let snapshot_root = package_snapshot.root();

    let check = check_package_dir_captured(snapshot_root)?;
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

    let native_build_dependencies = package_native_plugin_build_dependencies(snapshot_root)?;
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

    Ok(ExecutablePackageSnapshot {
        package_dir,
        lowering_input,
        native_build_dependencies,
        _package_snapshot: package_snapshot,
        content_snapshot,
    })
}

#[cfg(test)]
fn authorize_package_snapshot(
    package_dir: PathBuf,
    package_snapshot: PackageGraphSnapshot,
) -> Result<ExecutablePackageSnapshot, String> {
    let lowering_input = package_lowering_input(package_snapshot.root())?;
    verify_prepared_package(PreparedPackage {
        package_dir,
        lowering_input,
        package_snapshot,
    })
}

pub(super) fn snapshot_package_graph_inputs(
    package_dir: &Path,
) -> Result<PackageGraphSnapshot, String> {
    snapshot_package_graph_inputs_inner(package_dir, None)
}

fn snapshot_package_graph_inputs_inner(
    package_dir: &Path,
    operation: Option<&rsscript_operation::OperationContext>,
) -> Result<PackageGraphSnapshot, String> {
    check_operation(operation)?;
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Development)?;
    check_operation(operation)?;
    let captured = capture_project_graph(
        graph.nodes.values().map(|node| node.package_dir.clone()),
        [
            ".git",
            "target",
            "vendor",
            ".rsscript-artifacts.lock",
            super::source_set::SNAPSHOT_MANIFEST_SOURCE_FILE,
        ],
        operation,
    )?;

    let mut destinations = BTreeMap::new();
    for (key, node) in &graph.nodes {
        check_operation(operation)?;
        destinations.insert(
            key.clone(),
            captured
                .captured_path(&node.package_dir)
                .map(Path::to_path_buf)
                .ok_or_else(|| {
                    format!(
                        "project graph capture omitted package root {}",
                        node.package_dir.display()
                    )
                })?,
        );
    }

    for (key, node) in &graph.nodes {
        check_operation(operation)?;
        let destination = &destinations[key];
        validate_captured_manifest(node, destination)?;
        fs::write(
            destination.join(super::source_set::SNAPSHOT_MANIFEST_SOURCE_FILE),
            &node.manifest_source,
        )
        .map_err(|error| {
            format!(
                "failed to preserve original manifest identity for {}: {error}",
                node.package_dir.display()
            )
        })?;
    }
    rewrite_snapshot_manifests(&graph, &destinations)?;
    check_operation(operation)?;
    rewrite_snapshot_locks(&graph, &destinations)?;
    check_operation(operation)?;

    let root = destinations
        .get(&graph.root)
        .cloned()
        .ok_or_else(|| "package graph snapshot did not contain its root package".to_string())?;
    Ok(PackageGraphSnapshot { captured, root })
}

fn validate_captured_manifest(
    node: &super::dependency::ResolvedDependencyNode,
    destination: &Path,
) -> Result<(), String> {
    let manifest_path = destination.join("rsspkg.toml");
    let captured_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    if captured_source != node.manifest_source {
        return Err(format!(
            "package manifest changed while the content snapshot was captured: {}",
            node.package_dir.display()
        ));
    }
    Ok(())
}

fn rewrite_snapshot_manifests(
    graph: &super::dependency::ResolvedDependencyGraph,
    destinations: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    for (key, node) in &graph.nodes {
        let manifest_path = destinations[key].join("rsspkg.toml");
        let mut document: toml::Value = toml::from_str(&node.manifest_source)
            .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;
        let mut changed = false;
        for section in ["dependencies", "dev-dependencies"] {
            let Some(dependencies) = document
                .get_mut(section)
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            for (_, dependency) in dependencies.iter_mut() {
                let Some(specification) = dependency.as_table_mut() else {
                    continue;
                };
                let Some(path) = specification
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .map(str::to_string)
                else {
                    continue;
                };
                let path = Path::new(&path);
                if !path.is_absolute() {
                    continue;
                }
                let target = super::canonical_path_label(path);
                let Some(destination) = destinations.get(&target) else {
                    return Err(format!(
                        "absolute path dependency is outside the captured package graph: {}",
                        path.display()
                    ));
                };
                specification.insert(
                    "path".to_string(),
                    toml::Value::String(destination.display().to_string()),
                );
                changed = true;
            }
        }
        if changed {
            fs::write(
                &manifest_path,
                toml::to_string_pretty(&document).map_err(|error| {
                    format!("failed to encode {}: {error}", manifest_path.display())
                })?,
            )
            .map_err(|error| format!("failed to rewrite {}: {error}", manifest_path.display()))?;
        }
    }
    Ok(())
}

fn rewrite_snapshot_locks(
    graph: &super::dependency::ResolvedDependencyGraph,
    destinations: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let source_map = graph
        .nodes
        .keys()
        .map(|key| (key.clone(), package_path_source(&destinations[key])))
        .collect::<BTreeMap<_, _>>();

    for destination in destinations.values() {
        let lock_path = destination.join("rsspkg.lock");
        if !lock_path.is_file() {
            continue;
        }
        let mut lock = super::lock::read_package_lock(&lock_path)?;
        for package in &mut lock.packages {
            let Some(path) = package.source.strip_prefix("path+") else {
                continue;
            };
            let canonical = super::canonical_path_label(Path::new(path));
            if let Some(snapshot_source) = source_map.get(&canonical) {
                package.source = snapshot_source.clone();
            }
        }
        fs::write(&lock_path, package_lock_toml(&lock))
            .map_err(|error| format!("failed to rewrite {}: {error}", lock_path.display()))?;
    }
    Ok(())
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

    // The cache layout is versioned because earlier layouts did not verify a
    // reused entry before returning it.  Do not let a partially populated
    // legacy cache become an authorized native build input.
    let cache_root = std::env::var_os("RSS_NATIVE_SNAPSHOT_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rss-native-snapshots-v2"));
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

    let native_abi_source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../experiments/native-abi");
    let native_abi_path = directory.path().join("native-abi");
    snapshot_tree(&native_abi_source, &native_abi_path)?;
    let digest = snapshot_tree_digest(directory.path())?;
    let published = entries_root.join(&digest);
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
        let published_digest = snapshot_tree_digest(&published)?;
        if published_digest != digest {
            return Err(format!(
                "authorized native snapshot cache entry failed integrity verification: {}",
                published.display()
            ));
        }
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
            "native input changed while content snapshot was captured: {}",
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
            "native input changed while content snapshot was captured: {}",
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
        "external provider verification snapshots require verifiable private directory ownership and ACLs; this platform backend is unavailable for {}",
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::package::{
        check_package_dir, lock_package_dir, package_lock_toml, package_tree, review_package_dir,
    };

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
            "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[native.rust]\nenabled = true\npath = \"native/rust\"\ncrate = \"authorized_test_native\"\n\n[native.rust.policy]\nbuild_scripts = \"forbid\"\nproc_macros = \"forbid\"\nrss_unsafe_apis = \"forbid\"\nwrapper_unsafe_blocks = \"forbid\"\ntransitive_unsafe_blocks = \"forbid\"\n",
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
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let package = prepare_executable_package(&root).expect("checked fixture should authorize");
        assert_eq!(
            package.package_dir(),
            root.canonicalize().expect("canonical fixture")
        );
        assert_eq!(package.lowering_input().package.name, "authorized-test");
        assert!(package.native_build_dependencies().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_check_cannot_produce_an_authorized_package() {
        let root = pure_package_fixture();

        let error =
            prepare_executable_package(&root).expect_err("missing lock must prevent authorization");
        assert!(error.contains("native build/load denied"), "{error}");
        assert!(error.contains("rsspkg.lock missing"), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn package_graph_snapshot_preserves_lock_semantics() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let original = lock_package_dir(&root).expect("original lock");
        let snapshot = snapshot_package_graph_inputs(&root).expect("package graph snapshot");
        let captured =
            super::super::lock::lock_package_dir_captured(snapshot.root()).expect("snapshot lock");
        let original_files = collect_regular_files_for_test(&root.join("native/rust"), &root);
        let captured_files =
            collect_regular_files_for_test(&snapshot.root().join("native/rust"), snapshot.root());
        assert_eq!(original_files, captured_files, "native snapshot files");

        assert_eq!(original.packages.len(), captured.packages.len());
        for (original, captured) in original.packages.iter().zip(&captured.packages) {
            assert_eq!(original.name, captured.name);
            assert_eq!(original.version, captured.version);
            assert_eq!(
                original.interface_hash, captured.interface_hash,
                "interface hash"
            );
            assert_eq!(original.review_hash, captured.review_hash, "review hash");
            assert_eq!(original.native_hash, captured.native_hash, "native hash");
            assert_eq!(original.checksum, captured.checksum, "package checksum");
            assert_eq!(original.features, captured.features);
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn authorization_checks_the_captured_source_not_a_later_checkout_mutation() {
        let root = pure_package_fixture();
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let original_root = root.canonicalize().expect("canonical fixture");
        let snapshot =
            snapshot_package_graph_inputs(&original_root).expect("package graph snapshot");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { Missing.call(); return Unit }\n",
        )
        .expect("mutate original checkout after capture");

        let package = authorize_package_snapshot(original_root, snapshot)
            .expect("authorization must inspect captured source");
        assert_eq!(
            package.lowering_input().source,
            "fn main() -> Unit { return Unit }\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn captured_read_operations_ignore_later_checkout_mutation() {
        let root = pure_package_fixture();
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let snapshot = snapshot_package_graph_inputs(&root).expect("package graph snapshot");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { Missing.call(); return Unit }\n",
        )
        .expect("mutate original checkout after capture");

        let review =
            super::super::review::review_package_dir_captured_with_features(snapshot.root(), None)
                .expect("captured review");
        let mut captured_lock =
            super::super::lock::lock_package_dir_captured(snapshot.root()).expect("captured lock");
        snapshot.remap_lock(&mut captured_lock);
        let check = super::super::check::check_package_dir_captured(snapshot.root())
            .expect("captured check");

        assert!(
            review
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.severity.is_error()),
            "{:?}",
            review.diagnostics
        );
        assert_eq!(captured_lock.packages, lock.packages);
        assert!(check.ok, "{:?}", check.reasons);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_package_reads_preserve_checkout_paths() {
        let root = pure_package_fixture();
        let dependency = root.with_file_name(format!(
            "{}-absolute-dependency",
            root.file_name()
                .and_then(|name| name.to_str())
                .expect("fixture name")
        ));
        fs::create_dir_all(dependency.join("src")).expect("dependency source directory");
        fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"absolute-dependency\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("dependency manifest");
        fs::write(
            dependency.join("src/lib.rss"),
            "fn dependency_value() -> Int { return 1 }\n",
        )
        .expect("dependency source");
        fs::write(
            root.join("rsspkg.toml"),
            format!(
                "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[dependencies]\nabsolute-dependency = {{ path = {:?} }}\n",
                dependency.display().to_string()
            ),
        )
        .expect("root dependency manifest");
        let lock = lock_package_dir(&root).expect("fixture dependency lock");
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let review = review_package_dir(&root).expect("public review");
        let check = check_package_dir(&root).expect("public check");
        let lock = lock_package_dir(&root).expect("public lock");
        let tree = package_tree(&root).expect("public tree");
        let outputs = [
            serde_json::to_string(&review).expect("review JSON"),
            serde_json::to_string(&check).expect("check JSON"),
            serde_json::to_string(&lock).expect("lock JSON"),
            serde_json::to_string(&tree).expect("tree JSON"),
        ];

        for output in outputs {
            assert!(
                !output.contains("rsscript-package-graph-"),
                "snapshot path leaked into public output: {output}"
            );
        }
        assert_eq!(
            review.manifest_path,
            root.join("rsspkg.toml").display().to_string()
        );
        assert_eq!(check.package_dir, root.display().to_string());
        assert!(
            lock.packages
                .iter()
                .any(|package| { package.source == format!("path+{}", dependency.display()) }),
            "{:?}",
            lock.packages
        );
        assert!(
            tree.root
                .dependencies
                .iter()
                .any(|node| node.source == format!("path+{}", dependency.display())),
            "{:?}",
            tree.root.dependencies
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(dependency);
    }

    #[test]
    fn public_package_read_errors_do_not_expose_snapshot_paths() {
        let root = pure_package_fixture();
        fs::write(root.join("src/main.rss"), [0xff])
            .expect("replace fixture with invalid UTF-8 source");

        let error = review_package_dir(&root).expect_err("invalid source encoding must fail");
        assert!(!error.contains("rsscript-package-graph-"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn authorization_captures_path_dependency_sources_before_check() {
        let root = pure_package_fixture();
        let dependency = root.with_file_name(format!(
            "{}-dependency",
            root.file_name()
                .and_then(|name| name.to_str())
                .expect("fixture name")
        ));
        fs::create_dir_all(dependency.join("src")).expect("dependency source directory");
        fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"authorized-dependency\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("dependency manifest");
        fs::write(
            dependency.join("src/lib.rss"),
            "fn dependency_value() -> Int { return 1 }\n",
        )
        .expect("dependency source");
        fs::write(
            root.join("rsspkg.toml"),
            format!(
                "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[dependencies]\nauthorized-dependency = {{ path = \"../{}\" }}\n",
                dependency
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("dependency fixture name")
            ),
        )
        .expect("root dependency manifest");
        let lock = lock_package_dir(&root).expect("fixture dependency lock");
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let original_root = root.canonicalize().expect("canonical fixture");
        let snapshot =
            snapshot_package_graph_inputs(&original_root).expect("package graph snapshot");
        fs::write(
            dependency.join("src/lib.rss"),
            "fn dependency_value() -> Int { return 999 }\n",
        )
        .expect("mutate dependency after capture");

        let package = authorize_package_snapshot(original_root, snapshot)
            .expect("captured dependency graph should authorize");
        assert!(
            package
                .lowering_input()
                .sources
                .iter()
                .any(|(_, source)| { source == "fn dependency_value() -> Int { return 1 }\n" })
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(dependency);
    }

    fn collect_regular_files_for_test(root: &Path, package_root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        super::super::collect_regular_files(root, &mut files).expect("collect native files");
        files.sort();
        files
            .into_iter()
            .map(|path| {
                (
                    super::super::relative_path(package_root, &path),
                    fs::read(path).expect("read native file"),
                )
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn successful_native_authorization_captures_checked_build_inputs() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let package =
            prepare_executable_package(&root).expect("checked native fixture should authorize");
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
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let package =
            prepare_executable_package(&root).expect("checked native fixture should authorize");
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
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let package =
            prepare_executable_package(&root).expect("checked native fixture should authorize");
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
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let first = prepare_executable_package(&root).expect("first authorization");
        let second = prepare_executable_package(&root).expect("second authorization");
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
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let error = prepare_executable_package(&root)
            .expect_err("external provider verification must require private cache enforcement");
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
        fs::write(root.join("rsspkg.lock"), package_lock_toml(&lock)).expect("fixture lockfile");

        let error = prepare_executable_package(&root)
            .expect_err("native package without Cargo.lock must fail closed");
        assert!(
            error.contains("cargo metadata failed") || error.contains("Cargo.lock"),
            "{error}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
