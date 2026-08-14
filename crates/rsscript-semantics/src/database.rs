use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use rsscript_diagnostics::Diagnostic;
use rsscript_operation::{OperationAbort, OperationContext};
use rsscript_source_model::{FileId, SourceRevision};
use rsscript_syntax::{
    ast::{Item, Program, merge_programs},
    parse_source,
};

use crate::SemanticTypeFacts;
use crate::hir::Hir;
use crate::{InterfaceDescriptorError, InterfaceDescriptorV1};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendStopReason {
    SourceBytes,
    Tokens,
    ParseDepth,
    AstNodes,
    SemanticNodes,
    Substitutions,
    Diagnostics,
    SemanticRecursion,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendCompletion {
    Complete,
    Incomplete(FrontendStopReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileSnapshot {
    file_id: FileId,
    revision: SourceRevision,
    path: Arc<str>,
    text: Arc<str>,
}

impl SourceFileSnapshot {
    pub(crate) fn new(
        file_id: FileId,
        revision: SourceRevision,
        path: impl Into<Arc<str>>,
        text: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            file_id,
            revision,
            path: path.into(),
            text: text.into(),
        }
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn revision(&self) -> SourceRevision {
        self.revision
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Share the immutable source bytes retained by a session snapshot.
    ///
    /// Editor adapters may retain this handle for one response, but must not
    /// maintain a second mutable document-text store alongside the session.
    pub fn text_arc(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }
}

/// Immutable source bytes used by one frontend operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSnapshot {
    files: Arc<[SourceFileSnapshot]>,
}

impl SourceSnapshot {
    pub fn single(path: &str, text: &str) -> Self {
        Self::from_sources(std::iter::once((path, text)))
    }

    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            files: sources
                .into_iter()
                .enumerate()
                .map(|(index, (path, text))| {
                    SourceFileSnapshot::new(
                        FileId::new(
                            u32::try_from(index).expect("source snapshot exceeds u32 file IDs"),
                        ),
                        SourceRevision::INITIAL,
                        Arc::from(path),
                        Arc::from(text),
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn from_files(files: impl IntoIterator<Item = SourceFileSnapshot>) -> Self {
        Self {
            files: files.into_iter().collect(),
        }
    }

    pub fn files(&self) -> &[SourceFileSnapshot] {
        &self.files
    }

    pub fn file(&self, id: FileId) -> Option<&SourceFileSnapshot> {
        self.files.iter().find(|file| file.file_id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Immutable source and interface inputs for one frontend operation.
///
/// The compiler consumes this value without reading paths, the current
/// directory, or any host service. Workspace/package loaders may construct it
/// after capture, while embedders can construct it directly from in-memory
/// buffers. Source and interface files remain separate because interfaces
/// contribute external semantic contracts but are not executable source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendInputSnapshot {
    sources: SourceSnapshot,
    interfaces: SourceSnapshot,
}

impl FrontendInputSnapshot {
    pub fn single(path: &str, source: &str) -> Self {
        Self {
            sources: SourceSnapshot::single(path, source),
            interfaces: SourceSnapshot::from_sources(std::iter::empty()),
        }
    }

    pub fn from_sources<'a>(
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
        interfaces: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        Self {
            sources: SourceSnapshot::from_sources(sources),
            interfaces: SourceSnapshot::from_sources(interfaces),
        }
    }

    pub fn from_snapshots(sources: SourceSnapshot, interfaces: SourceSnapshot) -> Self {
        Self {
            sources,
            interfaces,
        }
    }

    pub fn sources(&self) -> &SourceSnapshot {
        &self.sources
    }

    pub fn interfaces(&self) -> &SourceSnapshot {
        &self.interfaces
    }
}

/// The outcome of adding or replacing one session-owned source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceUpdate {
    pub file_id: FileId,
    pub revision: SourceRevision,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStoreError {
    EmptyPath,
    FileIdExhausted,
    RevisionExhausted { file_id: FileId },
}

impl fmt::Display for SourceStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("source path must not be empty"),
            Self::FileIdExhausted => formatter.write_str("source session exhausted file IDs"),
            Self::RevisionExhausted { file_id } => {
                write!(
                    formatter,
                    "source file {} exhausted revisions",
                    file_id.get()
                )
            }
        }
    }
}

impl Error for SourceStoreError {}

#[derive(Debug, Clone)]
struct SessionFile {
    file_id: FileId,
    revision: SourceRevision,
    text: Arc<str>,
}

/// Immutable-revision source store used by one compilation session.
///
/// Paths are ordered in the snapshot, while identity is allocated once and is
/// never reused after deletion. Replacing unchanged bytes is deliberately a
/// no-op so editor refreshes do not invalidate dependent queries later.
#[derive(Debug, Clone, Default)]
pub struct SessionSourceStore {
    files: BTreeMap<String, SessionFile>,
    next_file_id: u32,
}

impl SessionSourceStore {
    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        let path = path.into();
        if path.is_empty() {
            return Err(SourceStoreError::EmptyPath);
        }
        let text: Arc<str> = Arc::from(text.into());
        if let Some(file) = self.files.get_mut(&path) {
            if file.text == text {
                return Ok(SourceUpdate {
                    file_id: file.file_id,
                    revision: file.revision,
                    changed: false,
                });
            }
            let revision = file
                .revision
                .next()
                .ok_or(SourceStoreError::RevisionExhausted {
                    file_id: file.file_id,
                })?;
            file.revision = revision;
            file.text = text;
            return Ok(SourceUpdate {
                file_id: file.file_id,
                revision,
                changed: true,
            });
        }
        let file_id = FileId::new(self.next_file_id);
        self.next_file_id = self
            .next_file_id
            .checked_add(1)
            .ok_or(SourceStoreError::FileIdExhausted)?;
        self.files.insert(
            path,
            SessionFile {
                file_id,
                revision: SourceRevision::INITIAL,
                text,
            },
        );
        Ok(SourceUpdate {
            file_id,
            revision: SourceRevision::INITIAL,
            changed: true,
        })
    }

    pub fn remove_file(&mut self, path: &str) -> Option<SourceUpdate> {
        self.files.remove(path).map(|file| SourceUpdate {
            file_id: file.file_id,
            revision: file.revision,
            changed: true,
        })
    }

    pub fn snapshot(&self) -> SourceSnapshot {
        SourceSnapshot::from_files(self.files.iter().map(|(path, file)| {
            SourceFileSnapshot::new(
                file.file_id,
                file.revision,
                Arc::from(path.as_str()),
                file.text.clone(),
            )
        }))
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The shared frontend input boundary. Query caching is layered on top of this
/// revisioned store; callers cannot mutate a snapshot after it is captured.
#[derive(Debug, Clone, Default)]
pub struct CompilationSession {
    sources: SessionSourceStore,
    interfaces: SessionSourceStore,
    parse_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<Program>>,
    hir_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<Hir>>,
    workspace_hir_cache: Option<Arc<Hir>>,
    workspace_type_cache: Option<Arc<SemanticTypeFacts>>,
    workspace_analysis_cache: Option<Arc<AnalysisResult>>,
    module_header_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<ModuleHeader>>,
    workspace_module_graph_cache: Option<Arc<WorkspaceModuleGraph>>,
    workspace_diagnostic_cache: Option<Arc<[Diagnostic]>>,
    parse_cache_hits: u64,
    parse_cache_misses: u64,
    hir_cache_hits: u64,
    hir_cache_misses: u64,
    workspace_hir_cache_hits: u64,
    workspace_hir_cache_misses: u64,
    workspace_type_cache_hits: u64,
    workspace_type_cache_misses: u64,
    workspace_analysis_cache_hits: u64,
    workspace_analysis_cache_misses: u64,
    module_header_cache_hits: u64,
    module_header_cache_misses: u64,
    workspace_module_graph_cache_hits: u64,
    workspace_module_graph_cache_misses: u64,
    workspace_diagnostic_cache_hits: u64,
    workspace_diagnostic_cache_misses: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompilationSessionStats {
    pub parse_cache_hits: u64,
    pub parse_cache_misses: u64,
    pub hir_cache_hits: u64,
    pub hir_cache_misses: u64,
    pub workspace_hir_cache_hits: u64,
    pub workspace_hir_cache_misses: u64,
    pub workspace_type_cache_hits: u64,
    pub workspace_type_cache_misses: u64,
    pub workspace_analysis_cache_hits: u64,
    pub workspace_analysis_cache_misses: u64,
    pub module_header_cache_hits: u64,
    pub module_header_cache_misses: u64,
    pub workspace_module_graph_cache_hits: u64,
    pub workspace_module_graph_cache_misses: u64,
    pub workspace_diagnostic_cache_hits: u64,
    pub workspace_diagnostic_cache_misses: u64,
}

/// Parsed module and import facts for one immutable document revision.
///
/// This is intentionally syntax-level: callers can build dependency graphs
/// without implementing a second textual grammar or depending on HIR.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleHeader {
    modules: Arc<[String]>,
    imports: Arc<[String]>,
}

/// One source or interface file in the session-owned workspace module graph.
///
/// The graph contains only syntax facts: declared module paths and imports.
/// It deliberately does not decide which interface imports are semantically
/// valid; that remains a resolve/type query. Keeping this query in the
/// session prevents editor clients from maintaining a second textual import
/// parser or a stale dependency cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceModuleNode {
    path: Arc<str>,
    is_interface: bool,
    modules: Arc<[String]>,
    imports: Arc<[String]>,
}

impl WorkspaceModuleNode {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn is_interface(&self) -> bool {
        self.is_interface
    }

    pub fn modules(&self) -> &[String] {
        &self.modules
    }

    pub fn imports(&self) -> &[String] {
        &self.imports
    }
}

/// Cached parsed module facts for every file in one immutable session input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceModuleGraph {
    nodes: Arc<[WorkspaceModuleNode]>,
}

impl WorkspaceModuleGraph {
    pub fn nodes(&self) -> &[WorkspaceModuleNode] {
        &self.nodes
    }

    pub fn source(&self, path: &str) -> Option<&WorkspaceModuleNode> {
        self.nodes
            .iter()
            .find(|node| !node.is_interface && node.path.as_ref() == path)
    }

    pub fn interface(&self, path: &str) -> Option<&WorkspaceModuleNode> {
        self.nodes
            .iter()
            .find(|node| node.is_interface && node.path.as_ref() == path)
    }

    /// Find interface files reachable from one document's parsed import set.
    ///
    /// This deliberately follows only interface declarations: source files
    /// retain their own module scope, while interfaces supply the visible
    /// external contract closure used by editor and package clients.
    pub fn visible_interface_paths(
        &self,
        current_path: &str,
        root_imports: impl IntoIterator<Item = String>,
    ) -> BTreeSet<String> {
        let mut imports = root_imports.into_iter().collect::<BTreeSet<_>>();
        let mut visible = BTreeSet::new();
        loop {
            let mut changed = false;
            for node in self.nodes.iter().filter(|node| node.is_interface) {
                if node.path() == current_path {
                    continue;
                }
                let selected = node.modules().iter().any(|module| {
                    imports
                        .iter()
                        .any(|import| import_matches_module(import, module))
                });
                if selected && visible.insert(node.path().to_string()) {
                    imports.extend(node.imports().iter().cloned());
                    changed = true;
                }
            }
            if !changed {
                return visible;
            }
        }
    }

    /// Find source and interface files whose diagnostics depend on a changed
    /// interface module closure. Callers pass both the old and new declared
    /// modules so renames/removals invalidate consumers fail-closed.
    pub fn interface_dependent_paths(
        &self,
        changed_modules: &BTreeSet<String>,
        changed_path: &str,
    ) -> BTreeSet<String> {
        let mut affected_modules = changed_modules.clone();
        loop {
            let mut changed = false;
            for node in self.nodes.iter().filter(|node| node.is_interface) {
                if node.path() == changed_path {
                    continue;
                }
                if node.imports().iter().any(|import| {
                    affected_modules
                        .iter()
                        .any(|module| import_matches_module(import, module))
                }) {
                    for module in node.modules() {
                        changed |= affected_modules.insert(module.clone());
                    }
                }
            }
            if !changed {
                break;
            }
        }

        self.nodes
            .iter()
            .filter(|node| node.path() != changed_path)
            .filter(|node| {
                node.imports().iter().any(|import| {
                    affected_modules
                        .iter()
                        .any(|module| import_matches_module(import, module))
                })
            })
            .map(|node| node.path().to_string())
            .collect()
    }
}

impl ModuleHeader {
    pub fn modules(&self) -> &[String] {
        &self.modules
    }

    pub fn imports(&self) -> &[String] {
        &self.imports
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SessionFileRole {
    Source,
    Interface,
}

impl CompilationSession {
    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        let update = self.sources.set_file(path, text)?;
        if update.changed {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
            self.workspace_module_graph_cache = None;
            self.workspace_diagnostic_cache = None;
        }
        Ok(update)
    }

    pub fn remove_file(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.sources.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
            self.workspace_module_graph_cache = None;
            self.workspace_diagnostic_cache = None;
        }
        update
    }

    pub fn set_interface(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        let update = self.interfaces.set_file(path, text)?;
        if update.changed {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
            self.workspace_module_graph_cache = None;
            self.workspace_diagnostic_cache = None;
        }
        Ok(update)
    }

    pub fn remove_interface(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.interfaces.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
            self.workspace_module_graph_cache = None;
            self.workspace_diagnostic_cache = None;
        }
        update
    }

    pub fn source_snapshot(&self) -> SourceSnapshot {
        self.sources.snapshot()
    }

    pub fn interface_snapshot(&self) -> SourceSnapshot {
        self.interfaces.snapshot()
    }

    /// Return one immutable source-file revision owned by this session.
    pub fn source_file_snapshot(&self, path: &str) -> Option<SourceFileSnapshot> {
        self.source_snapshot()
            .files()
            .iter()
            .find(|file| file.path() == path)
            .cloned()
    }

    /// Return one immutable interface-file revision owned by this session.
    pub fn interface_file_snapshot(&self, path: &str) -> Option<SourceFileSnapshot> {
        self.interface_snapshot()
            .files()
            .iter()
            .find(|file| file.path() == path)
            .cloned()
    }

    /// Capture both session-owned input roles for one workspace semantic query.
    ///
    /// The returned input owns immutable bytes and stable file revisions. A
    /// caller can therefore not analyze a document set different from the one
    /// whose query result the session caches.
    pub fn frontend_input_snapshot(&self) -> FrontendInputSnapshot {
        FrontendInputSnapshot::from_snapshots(self.source_snapshot(), self.interface_snapshot())
    }

    /// Run the complete source/interface semantic analysis for the current
    /// immutable session revision set. This is the single cached query for
    /// resolve, type, checked-HIR, and source diagnostics; callers cannot
    /// accidentally assemble an equivalent analysis from independently read
    /// files or a second analyzer instance.
    pub fn workspace_analysis(&mut self) -> Arc<AnalysisResult> {
        self.workspace_analysis_inner(None)
            .expect("an unchecked workspace analysis query cannot abort")
    }

    fn workspace_analysis_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(analysis) = &self.workspace_analysis_cache {
            self.workspace_analysis_cache_hits =
                self.workspace_analysis_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(analysis));
        }

        let input = self.frontend_input_snapshot();
        let sources = input
            .sources()
            .files()
            .iter()
            .map(|file| (file.path(), file.text()))
            .collect::<Vec<_>>();
        let interfaces = input
            .interfaces()
            .files()
            .iter()
            .map(|file| (file.path(), file.text()))
            .collect::<Vec<_>>();
        let analysis = Arc::new(match operation {
            Some(operation) => crate::analyze_sources_with_interfaces_result_with_operation(
                &sources,
                &interfaces,
                operation,
            ),
            None => crate::analyze_sources_with_interfaces_result(&sources, &interfaces),
        });
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_analysis_cache_misses =
            self.workspace_analysis_cache_misses.saturating_add(1);
        self.workspace_analysis_cache = Some(Arc::clone(&analysis));
        Ok(analysis)
    }

    /// Operation-aware complete semantic analysis. Both cold and cached paths
    /// check the shared cancellation/deadline boundary before returning facts.
    pub fn workspace_analysis_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        self.workspace_analysis_inner(Some(operation))
    }

    /// Convert the cached workspace analysis into the phase-gated valid
    /// program. Invalid source stays an ordinary diagnostic result; only
    /// cancellation and deadline expiry abort the query itself.
    pub fn workspace_validated_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Result<ValidatedProgram, Vec<Diagnostic>>, OperationAbort> {
        let analysis = self.workspace_analysis_with_operation(operation)?;
        Ok((*analysis).clone().into_validated())
    }

    /// Convert the current cached workspace analysis into its checked phase
    /// without introducing an unchecked analyzer entry point for callers that
    /// do not need cancellation or a deadline.
    pub fn workspace_validated(&mut self) -> Result<ValidatedProgram, Vec<Diagnostic>> {
        (*self.workspace_analysis()).clone().into_validated()
    }

    /// Diagnose the current immutable workspace through the semantic-owned
    /// frontend implementation, caching by the session's source/interface
    /// revisions. No caller can inject a competing diagnostic pipeline.
    pub fn semantic_workspace_diagnostics_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<[Diagnostic]>, OperationAbort> {
        operation.check()?;
        if let Some(diagnostics) = &self.workspace_diagnostic_cache {
            self.workspace_diagnostic_cache_hits =
                self.workspace_diagnostic_cache_hits.saturating_add(1);
            operation.check()?;
            return Ok(Arc::clone(diagnostics));
        }

        self.workspace_diagnostic_cache_misses =
            self.workspace_diagnostic_cache_misses.saturating_add(1);
        let input = self.frontend_input_snapshot();
        let diagnostics: Arc<[Diagnostic]> =
            crate::analyze_frontend_input_snapshot_with_operation(&input, operation)?.into();
        operation.check()?;
        self.workspace_diagnostic_cache = Some(Arc::clone(&diagnostics));
        Ok(diagnostics)
    }

    /// Return the parsed workspace module graph for the current immutable
    /// source/interface revision set. Interface files without an explicit
    /// `module` declaration keep the historical filename fallback, so all
    /// session clients resolve the same graph.
    pub fn workspace_module_graph(&mut self) -> Arc<WorkspaceModuleGraph> {
        self.workspace_module_graph_inner(None)
            .expect("an unchecked workspace module-graph query cannot abort")
    }

    fn workspace_module_graph_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<WorkspaceModuleGraph>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(graph) = &self.workspace_module_graph_cache {
            self.workspace_module_graph_cache_hits =
                self.workspace_module_graph_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(graph));
        }

        let mut nodes = Vec::new();
        let sources = self.source_snapshot();
        for file in sources.files() {
            if let Some(operation) = operation {
                operation.check()?;
            }
            let header = self
                .module_header_snapshot_file(SessionFileRole::Source, file)
                .expect("session source snapshot file must remain parseable");
            nodes.push(WorkspaceModuleNode {
                path: Arc::clone(&file.path),
                is_interface: false,
                modules: Arc::clone(&header.modules),
                imports: Arc::clone(&header.imports),
            });
        }
        let interfaces = self.interface_snapshot();
        for file in interfaces.files() {
            if let Some(operation) = operation {
                operation.check()?;
            }
            let header = self
                .module_header_snapshot_file(SessionFileRole::Interface, file)
                .expect("session interface snapshot file must remain parseable");
            let modules = if header.modules.is_empty() {
                interface_filename_module(file.path())
                    .into_iter()
                    .collect::<Vec<_>>()
                    .into()
            } else {
                Arc::clone(&header.modules)
            };
            nodes.push(WorkspaceModuleNode {
                path: Arc::clone(&file.path),
                is_interface: true,
                modules,
                imports: Arc::clone(&header.imports),
            });
        }
        let graph = Arc::new(WorkspaceModuleGraph {
            nodes: nodes.into(),
        });
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_module_graph_cache_misses =
            self.workspace_module_graph_cache_misses.saturating_add(1);
        self.workspace_module_graph_cache = Some(Arc::clone(&graph));
        Ok(graph)
    }

    /// Operation-aware module graph query. Cached syntax facts still obey a
    /// caller's cancellation and deadline boundary before being returned.
    pub fn workspace_module_graph_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<WorkspaceModuleGraph>, OperationAbort> {
        self.workspace_module_graph_inner(Some(operation))
    }

    /// Parse one source revision exactly once. Replacing or removing a file
    /// evicts its cached syntax tree; identical writes preserve the cached
    /// query result because the revision is unchanged.
    pub fn parse_file(&mut self, path: &str) -> Option<Arc<Program>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.parse_snapshot_file(SessionFileRole::Source, file)
    }

    /// Parse a source revision while honoring the shared frontend operation
    /// boundary. The post-query check is deliberate: a cached result must not
    /// escape after a caller has cancelled or timed out the request.
    pub fn parse_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Program>>, OperationAbort> {
        operation.check()?;
        let program = self.parse_file(path);
        operation.check()?;
        Ok(program)
    }

    /// Parse one interface revision through the same cache. File IDs are
    /// session-local across both stores, so the query key also records its role.
    pub fn parse_interface(&mut self, path: &str) -> Option<Arc<Program>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.parse_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Interface parsing uses the same cancellation/deadline contract as
    /// source parsing; callers cannot accidentally bypass it through the
    /// separate interface store.
    pub fn parse_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Program>>, OperationAbort> {
        operation.check()?;
        let program = self.parse_interface(path);
        operation.check()?;
        Ok(program)
    }

    /// Return source parse trees in canonical snapshot path order.
    pub fn parsed_sources(&mut self) -> Vec<Arc<Program>> {
        self.source_snapshot()
            .files()
            .iter()
            .filter_map(|file| self.parse_snapshot_file(SessionFileRole::Source, file))
            .collect()
    }

    /// Return canonical source parse trees while polling the shared operation
    /// context between individual file queries.
    pub fn parsed_sources_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Vec<Arc<Program>>, OperationAbort> {
        let paths = self
            .source_snapshot()
            .files()
            .iter()
            .map(|file| file.path().to_string())
            .collect::<Vec<_>>();
        let mut programs = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(program) = self.parse_file_with_operation(&path, operation)? {
                programs.push(program);
            }
        }
        Ok(programs)
    }

    /// Build the source-shaped HIR for one immutable source revision exactly
    /// once. Interface-aware workspace HIR remains a higher-level query, but
    /// this per-file cache gives editor and incremental callers a stable local
    /// semantic fact boundary without re-parsing unchanged text.
    pub fn hir_file(&mut self, path: &str) -> Option<Arc<Hir>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.hir_snapshot_file(SessionFileRole::Source, file)
    }

    /// Interface HIR uses a role-separated key because source and interface
    /// stores allocate file IDs independently.
    pub fn hir_interface(&mut self, path: &str) -> Option<Arc<Hir>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.hir_snapshot_file(SessionFileRole::Interface, file)
    }

    pub fn hir_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Hir>>, OperationAbort> {
        operation.check()?;
        let hir = self.hir_file(path);
        operation.check()?;
        Ok(hir)
    }

    pub fn hir_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<Hir>>, OperationAbort> {
        operation.check()?;
        let hir = self.hir_interface(path);
        operation.check()?;
        Ok(hir)
    }

    /// Build and cache the interface-aware, namespace-isolated HIR for the
    /// current immutable source/interface revisions.
    ///
    /// This is the session's workspace HIR query: it reuses cached parse trees,
    /// applies the same source/interface namespace rewrite as compiler analysis,
    /// and keeps host interfaces separate from executable source declarations.
    /// Full type checking remains a transitional compiler query, but consumers
    /// can no longer build a competing workspace HIR from ad-hoc file reads.
    pub fn workspace_hir(&mut self) -> Arc<Hir> {
        self.workspace_hir_inner(None)
            .expect("an unchecked workspace HIR query cannot abort")
    }

    fn workspace_hir_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<Hir>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(hir) = &self.workspace_hir_cache {
            self.workspace_hir_cache_hits = self.workspace_hir_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(hir));
        }

        let source_files = self.source_snapshot().files().to_vec();
        let mut source_programs = Vec::with_capacity(source_files.len());
        for file in &source_files {
            if let Some(operation) = operation {
                operation.check()?;
            }
            if let Some(program) = self.parse_snapshot_file(SessionFileRole::Source, file) {
                source_programs.push((*program).clone());
            }
        }
        let mut sources = merge_programs(source_programs);
        let interface_files = self.interface_snapshot().files().to_vec();
        let mut interfaces = Vec::with_capacity(interface_files.len());
        for file in &interface_files {
            if let Some(operation) = operation {
                operation.check()?;
            }
            if let Some(program) = self.parse_snapshot_file(SessionFileRole::Interface, file) {
                interfaces.push((*program).clone());
            }
        }
        if let Some(operation) = operation {
            operation.check()?;
        }
        crate::isolate_sources_with_interfaces(&mut sources, &mut interfaces);
        if let Some(operation) = operation {
            operation.check()?;
        }
        let hir = Arc::new(Hir::from_syntax_with_interfaces(&sources, &interfaces));
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_hir_cache_misses = self.workspace_hir_cache_misses.saturating_add(1);
        self.workspace_hir_cache = Some(Arc::clone(&hir));
        Ok(hir)
    }

    /// Operation-aware workspace-HIR query. Cached HIR cannot escape a
    /// cancelled or expired request boundary.
    pub fn workspace_hir_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<Hir>, OperationAbort> {
        self.workspace_hir_inner(Some(operation))
    }

    /// Return structural type facts for the current namespace-isolated
    /// workspace HIR. The facts share the HIR's immutable type arena, so the
    /// session never reparses or re-interns types for a second consumer.
    ///
    /// Full resolve/type diagnostics are still migrating into semantics. This
    /// query is the stable cache boundary for already-resolved declaration and
    /// signature facts, not a replacement for those remaining checks.
    pub fn workspace_type_facts(&mut self) -> Arc<SemanticTypeFacts> {
        self.workspace_type_facts_inner(None)
            .expect("an unchecked workspace type-fact query cannot abort")
    }

    fn workspace_type_facts_inner(
        &mut self,
        operation: Option<&OperationContext>,
    ) -> Result<Arc<SemanticTypeFacts>, OperationAbort> {
        if let Some(operation) = operation {
            operation.check()?;
        }
        if let Some(types) = &self.workspace_type_cache {
            self.workspace_type_cache_hits = self.workspace_type_cache_hits.saturating_add(1);
            if let Some(operation) = operation {
                operation.check()?;
            }
            return Ok(Arc::clone(types));
        }

        let hir = match operation {
            Some(operation) => self.workspace_hir_with_operation(operation)?,
            None => self.workspace_hir(),
        };
        let types = hir.semantic_types_arc();
        if let Some(operation) = operation {
            operation.check()?;
        }
        self.workspace_type_cache_misses = self.workspace_type_cache_misses.saturating_add(1);
        self.workspace_type_cache = Some(Arc::clone(&types));
        Ok(types)
    }

    /// Operation-aware type-fact query. Cached facts must not escape a
    /// cancellation or deadline boundary after they have been obtained.
    pub fn workspace_type_facts_with_operation(
        &mut self,
        operation: &OperationContext,
    ) -> Result<Arc<SemanticTypeFacts>, OperationAbort> {
        self.workspace_type_facts_inner(Some(operation))
    }

    /// Return parsed module and import paths for one source revision.
    pub fn module_header(&mut self, path: &str) -> Option<Arc<ModuleHeader>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.module_header_snapshot_file(SessionFileRole::Source, file)
    }

    /// Query one source module header while observing the shared operation
    /// boundary. Cached syntax facts must not escape after cancellation or a
    /// deadline, just like parse and HIR query results.
    pub fn module_header_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<ModuleHeader>>, OperationAbort> {
        operation.check()?;
        let header = self.module_header(path);
        operation.check()?;
        Ok(header)
    }

    /// Return parsed module and import paths for one interface revision.
    pub fn interface_module_header(&mut self, path: &str) -> Option<Arc<ModuleHeader>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.module_header_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Interface headers use the same cancellation/deadline contract as
    /// source headers, so editor clients cannot bypass it through the separate
    /// interface store.
    pub fn interface_module_header_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<ModuleHeader>>, OperationAbort> {
        operation.check()?;
        let header = self.interface_module_header(path);
        operation.check()?;
        Ok(header)
    }

    pub fn stats(&self) -> CompilationSessionStats {
        CompilationSessionStats {
            parse_cache_hits: self.parse_cache_hits,
            parse_cache_misses: self.parse_cache_misses,
            hir_cache_hits: self.hir_cache_hits,
            hir_cache_misses: self.hir_cache_misses,
            workspace_hir_cache_hits: self.workspace_hir_cache_hits,
            workspace_hir_cache_misses: self.workspace_hir_cache_misses,
            workspace_type_cache_hits: self.workspace_type_cache_hits,
            workspace_type_cache_misses: self.workspace_type_cache_misses,
            workspace_analysis_cache_hits: self.workspace_analysis_cache_hits,
            workspace_analysis_cache_misses: self.workspace_analysis_cache_misses,
            module_header_cache_hits: self.module_header_cache_hits,
            module_header_cache_misses: self.module_header_cache_misses,
            workspace_module_graph_cache_hits: self.workspace_module_graph_cache_hits,
            workspace_module_graph_cache_misses: self.workspace_module_graph_cache_misses,
            workspace_diagnostic_cache_hits: self.workspace_diagnostic_cache_hits,
            workspace_diagnostic_cache_misses: self.workspace_diagnostic_cache_misses,
        }
    }

    fn parse_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<Program>> {
        // Source and interface stores allocate FileId independently, so role
        // is part of the private cache key.
        let key = (role, file.file_id(), file.revision());
        if let Some(program) = self.parse_cache.get(&key) {
            self.parse_cache_hits = self.parse_cache_hits.saturating_add(1);
            return Some(Arc::clone(program));
        }
        let program = Arc::new(parse_source(file.path(), file.text()));
        self.parse_cache.insert(key, Arc::clone(&program));
        self.parse_cache_misses = self.parse_cache_misses.saturating_add(1);
        Some(program)
    }

    fn invalidate_parse_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.parse_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    fn hir_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<Hir>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(hir) = self.hir_cache.get(&key) {
            self.hir_cache_hits = self.hir_cache_hits.saturating_add(1);
            return Some(Arc::clone(hir));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let hir = Arc::new(Hir::from_syntax(&program));
        self.hir_cache.insert(key, Arc::clone(&hir));
        self.hir_cache_misses = self.hir_cache_misses.saturating_add(1);
        Some(hir)
    }

    fn invalidate_hir_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.hir_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    fn module_header_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<ModuleHeader>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(header) = self.module_header_cache.get(&key) {
            self.module_header_cache_hits = self.module_header_cache_hits.saturating_add(1);
            return Some(Arc::clone(header));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let header = Arc::new(module_header_from_program(&program));
        self.module_header_cache.insert(key, Arc::clone(&header));
        self.module_header_cache_misses = self.module_header_cache_misses.saturating_add(1);
        Some(header)
    }

    fn invalidate_module_header_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.module_header_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }
}

fn interface_filename_module(path: &str) -> Option<String> {
    let fallback = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".rssi")
        .trim_end_matches(".rss");
    (!fallback.is_empty()).then(|| fallback.to_string())
}

fn import_matches_module(import: &str, module: &str) -> bool {
    import == module
        || import
            .strip_prefix(module)
            .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('{'))
}

fn module_header_from_program(program: &Program) -> ModuleHeader {
    let modules = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Module(module) => (!module.path.is_empty()).then(|| module.path.join(".")),
            Item::Use(_)
            | Item::Type(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_)
            | Item::Function(_) => None,
        })
        .collect::<Vec<_>>();
    let imports = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(import) => (!import.path.is_empty()).then(|| import.path.join(".")),
            Item::Module(_)
            | Item::Type(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_)
            | Item::Function(_) => None,
        })
        .collect::<Vec<_>>();
    ModuleHeader {
        modules: modules.into(),
        imports: imports.into(),
    }
}

/// Parsed and resolved facts produced by one bounded frontend run.
///
/// This type owns the immutable relationship between source bytes, parsed
/// programs, checked HIR, and structural type facts. Runtime, Provider, package,
/// and review layers cannot construct or alter those facts.
#[derive(Debug, Clone)]
pub struct SemanticDatabase {
    sources: SourceSnapshot,
    interfaces: SourceSnapshot,
    source_programs: Vec<Program>,
    program: Program,
    interface_programs: Vec<Program>,
    hir: Hir,
    types: Arc<SemanticTypeFacts>,
}

impl SemanticDatabase {
    /// Frontend integration boundary used while analyzer orchestration migrates
    /// into this crate. Callers must supply programs and HIR from the same
    /// immutable snapshots. The constructor remains deliberately specific so it
    /// cannot become a general-purpose database mutation API.
    #[doc(hidden)]
    pub fn from_frontend_parts(
        sources: SourceSnapshot,
        interfaces: SourceSnapshot,
        source_programs: Vec<Program>,
        program: Program,
        interface_programs: Vec<Program>,
        hir: Hir,
    ) -> Self {
        let types = hir.semantic_types_arc();
        Self {
            sources,
            interfaces,
            source_programs,
            program,
            interface_programs,
            hir,
            types,
        }
    }

    pub fn sources(&self) -> &SourceSnapshot {
        &self.sources
    }

    pub fn interfaces(&self) -> &SourceSnapshot {
        &self.interfaces
    }

    pub fn source_programs(&self) -> &[Program] {
        &self.source_programs
    }

    /// Namespace-isolated merged syntax consumed by transitional executable
    /// backends. Semantic consumers must use checked HIR facts instead.
    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn interface_programs(&self) -> &[Program] {
        &self.interface_programs
    }

    /// Versioned Provider-facing contracts derived from the same immutable
    /// parsed interface snapshot used by semantic analysis.
    pub fn interface_descriptors(
        &self,
    ) -> Result<Vec<InterfaceDescriptorV1>, InterfaceDescriptorError> {
        self.interface_programs
            .iter()
            .map(InterfaceDescriptorV1::from_interface_program)
            .collect()
    }

    pub fn hir(&self) -> &Hir {
        &self.hir
    }

    pub fn interned_type_count(&self) -> usize {
        self.types.arena().len()
    }
}

/// Complete semantic output, including diagnostics for invalid programs.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    database: SemanticDatabase,
    diagnostics: Vec<Diagnostic>,
    completion: FrontendCompletion,
}

impl AnalysisResult {
    /// Frontend integration boundary. Only the semantic analyzer should create
    /// this value; downstream consumers receive immutable accessors or attempt
    /// the checked transition to [`ValidatedProgram`].
    #[doc(hidden)]
    pub fn from_frontend(
        database: SemanticDatabase,
        diagnostics: Vec<Diagnostic>,
        completion: FrontendCompletion,
    ) -> Self {
        Self {
            database,
            diagnostics,
            completion,
        }
    }

    pub fn database(&self) -> &SemanticDatabase {
        &self.database
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn completion(&self) -> FrontendCompletion {
        self.completion
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn into_validated(self) -> Result<ValidatedProgram, Vec<Diagnostic>> {
        if self.completion != FrontendCompletion::Complete
            || self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        {
            return Err(self.diagnostics);
        }
        Ok(ValidatedProgram {
            database: self.database,
            diagnostics: self.diagnostics,
        })
    }
}

/// A semantic database whose frontend diagnostics contain no errors.
#[derive(Debug, Clone)]
pub struct ValidatedProgram {
    database: SemanticDatabase,
    diagnostics: Vec<Diagnostic>,
}

impl ValidatedProgram {
    pub fn database(&self) -> &SemanticDatabase {
        &self.database
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_operation::{CancellationToken, MonotonicDeadline};
    use std::time::{Duration, Instant};

    #[test]
    fn source_snapshot_owns_the_captured_text() {
        let mut source = "fn main() -> Unit { return Unit }\n".to_string();
        let snapshot = SourceSnapshot::single("main.rss", &source);
        source.clear();

        assert_eq!(
            snapshot.files()[0].text(),
            "fn main() -> Unit { return Unit }\n"
        );
    }

    #[test]
    fn frontend_input_snapshot_keeps_source_and_interface_roles_separate() {
        let input = FrontendInputSnapshot::from_sources(
            [("main.rss", "fn main() -> Unit { return Unit }")],
            [("host.rssi", "module host\npub fn value() -> Int")],
        );
        assert_eq!(input.sources().files()[0].path(), "main.rss");
        assert_eq!(input.interfaces().files()[0].path(), "host.rssi");
        assert_eq!(input.sources().files()[0].file_id(), FileId::new(0));
        assert_eq!(input.interfaces().files()[0].file_id(), FileId::new(0));
    }

    #[test]
    fn snapshot_assigns_repeatable_file_identity_and_initial_revision() {
        let snapshot = SourceSnapshot::from_sources([("a.rss", "a"), ("b.rss", "b")]);
        let first = &snapshot.files()[0];
        let second = &snapshot.files()[1];
        assert_eq!(first.file_id(), FileId::new(0));
        assert_eq!(second.file_id(), FileId::new(1));
        assert_eq!(first.revision(), SourceRevision::INITIAL);
        assert_eq!(snapshot.file(FileId::new(1)).unwrap().path(), "b.rss");
        assert!(snapshot.file(FileId::new(2)).is_none());
    }

    #[test]
    fn compilation_session_tracks_replacements_removals_and_deterministic_snapshots() {
        let mut session = CompilationSession::default();
        let beta = session.set_file("b.rss", "one").unwrap();
        let alpha = session.set_file("a.rss", "two").unwrap();
        assert_eq!(beta.file_id, FileId::new(0));
        assert_eq!(alpha.file_id, FileId::new(1));

        let first = session.source_snapshot();
        assert_eq!(
            first
                .files()
                .iter()
                .map(SourceFileSnapshot::path)
                .collect::<Vec<_>>(),
            ["a.rss", "b.rss"]
        );
        assert_eq!(first.file(beta.file_id).unwrap().text(), "one");

        let unchanged = session.set_file("b.rss", "one").unwrap();
        assert_eq!(
            unchanged,
            SourceUpdate {
                changed: false,
                ..beta
            }
        );
        let replacement = session.set_file("b.rss", "three").unwrap();
        assert_eq!(replacement.file_id, beta.file_id);
        assert_eq!(replacement.revision, SourceRevision::new(1));
        assert!(replacement.changed);

        assert_eq!(session.remove_file("a.rss").unwrap(), alpha);
        assert!(session.remove_file("a.rss").is_none());
        assert_eq!(session.source_snapshot().files().len(), 1);
        let interface = session.set_interface("host.rssi", "module host").unwrap();
        assert_eq!(interface.file_id, FileId::new(0));
        assert_eq!(session.interface_snapshot().files()[0].path(), "host.rssi");
    }

    #[test]
    fn compilation_session_caches_parse_queries_by_immutable_revision() {
        let mut session = CompilationSession::default();
        let source = session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        let first = session.parse_file("main.rss").unwrap();
        let second = session.parse_file("main.rss").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            session.stats(),
            CompilationSessionStats {
                parse_cache_hits: 1,
                parse_cache_misses: 1,
                hir_cache_hits: 0,
                hir_cache_misses: 0,
                workspace_hir_cache_hits: 0,
                workspace_hir_cache_misses: 0,
                workspace_type_cache_hits: 0,
                workspace_type_cache_misses: 0,
                workspace_analysis_cache_hits: 0,
                workspace_analysis_cache_misses: 0,
                module_header_cache_hits: 0,
                module_header_cache_misses: 0,
                workspace_module_graph_cache_hits: 0,
                workspace_module_graph_cache_misses: 0,
                workspace_diagnostic_cache_hits: 0,
                workspace_diagnostic_cache_misses: 0,
            }
        );

        let unchanged = session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        assert_eq!(unchanged.file_id, source.file_id);
        assert!(!unchanged.changed);
        assert!(Arc::ptr_eq(
            &first,
            &session.parse_file("main.rss").unwrap()
        ));

        session
            .set_file("main.rss", "fn main() -> Unit { let x = Unit return x }")
            .unwrap();
        let replacement = session.parse_file("main.rss").unwrap();
        assert!(!Arc::ptr_eq(&first, &replacement));
        session.remove_file("main.rss");
        assert!(session.parse_file("main.rss").is_none());
    }

    #[test]
    fn compilation_session_caches_workspace_diagnostics_for_one_input_snapshot() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn emit() -> Unit")
            .unwrap();
        let operation = OperationContext::default();
        let first = session
            .semantic_workspace_diagnostics_with_operation(&operation)
            .unwrap();
        let second = session
            .semantic_workspace_diagnostics_with_operation(&operation)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        session
            .set_interface("host.rssi", "module host\npub fn replacement() -> Unit")
            .unwrap();
        session
            .semantic_workspace_diagnostics_with_operation(&operation)
            .unwrap();
        assert_eq!(
            session.stats().workspace_diagnostic_cache_hits,
            1,
            "the unchanged query must be served from the session cache"
        );
        assert_eq!(session.stats().workspace_diagnostic_cache_misses, 2);
    }

    #[test]
    fn compilation_session_caches_complete_workspace_analysis_and_validation() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Int { return Host.value() }")
            .unwrap();
        session
            .set_interface("host.rssi", "module Host\npub fn value() -> Int\n")
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        let second = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(
            session
                .workspace_validated_with_operation(&operation)
                .unwrap()
                .is_ok()
        );
        assert_eq!(session.stats().workspace_analysis_cache_misses, 1);
        assert_eq!(session.stats().workspace_analysis_cache_hits, 2);

        session
            .set_file("main.rss", "fn main() -> Int { return Host.next() }")
            .unwrap();
        let replacement = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().workspace_analysis_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_analysis_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
    }

    #[test]
    fn compilation_session_owns_and_invalidates_the_workspace_module_graph() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.api\nfn main() -> Unit {}\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host.api\npub fn emit() -> Unit\n")
            .unwrap();
        session
            .set_interface(
                "host-base.rssi",
                "module host.base\npub fn base() -> Unit\n",
            )
            .unwrap();
        session
            .set_interface("fallback.rssi", "pub fn fallback() -> Unit\n")
            .unwrap();

        let first = session.workspace_module_graph();
        assert_eq!(first.source("main.rss").unwrap().imports(), ["host.api"]);
        assert_eq!(
            first.interface("host.rssi").unwrap().modules(),
            ["host.api"]
        );
        assert_eq!(
            first.interface("fallback.rssi").unwrap().modules(),
            ["fallback"]
        );
        assert_eq!(
            first.visible_interface_paths("main.rss", ["host.api".to_string()]),
            BTreeSet::from(["host.rssi".to_string()])
        );
        let second = session.workspace_module_graph();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);
        assert_eq!(session.stats().workspace_module_graph_cache_hits, 1);

        session
            .set_interface(
                "host.rssi",
                "module host.api\nuse host.base\npub fn emit() -> Unit\n",
            )
            .unwrap();
        let replacement = session.workspace_module_graph();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(
            replacement.interface("host.rssi").unwrap().imports(),
            ["host.base"]
        );
        assert_eq!(
            replacement.interface_dependent_paths(
                &BTreeSet::from(["host.base".to_string()]),
                "host-base.rssi",
            ),
            BTreeSet::from(["host.rssi".to_string(), "main.rss".to_string()])
        );
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            session.workspace_module_graph_with_operation(&OperationContext {
                cancellation: Some(cancellation),
                ..OperationContext::default()
            }),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(session.stats().workspace_module_graph_cache_hits, 1);
    }

    #[test]
    fn cached_workspace_diagnostics_obey_cancellation_and_deadline() {
        use std::time::{Duration, Instant};

        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .semantic_workspace_diagnostics_with_operation(&OperationContext::default())
            .unwrap();

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let cancelled_operation = OperationContext {
            cancellation: Some(cancelled),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.semantic_workspace_diagnostics_with_operation(&cancelled_operation),
            Err(OperationAbort::Cancelled)
        ));

        let expired_operation = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.semantic_workspace_diagnostics_with_operation(&expired_operation),
            Err(OperationAbort::DeadlineExceeded)
        ));
        assert_eq!(session.stats().workspace_diagnostic_cache_hits, 0);
    }

    #[test]
    fn source_and_interface_parse_caches_do_not_alias_the_same_file_id() {
        let mut session = CompilationSession::default();
        session
            .set_file("shared.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .set_interface("shared.rssi", "module host.shared\npub fn value() -> Int\n")
            .unwrap();
        let source = session.parse_file("shared.rss").unwrap();
        let interface = session.parse_interface("shared.rssi").unwrap();
        assert!(!Arc::ptr_eq(&source, &interface));
        assert_eq!(session.stats().parse_cache_misses, 2);
    }

    #[test]
    fn session_parse_queries_reject_cancelled_and_expired_requests_before_cache_access() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        let cached = session.parse_file("main.rss").unwrap();

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert_eq!(
            session.parse_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert_eq!(
            session.parse_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );

        let live = OperationContext::default();
        let reused = session
            .parse_file_with_operation("main.rss", &live)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&cached, &reused));
        assert_eq!(session.stats().parse_cache_hits, 1);
    }

    #[test]
    fn compilation_session_caches_hir_by_role_and_immutable_revision() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn emit() -> Unit\n")
            .unwrap();

        let first = session.hir_file("main.rss").expect("source HIR");
        let second = session.hir_file("main.rss").expect("cached source HIR");
        let interface = session.hir_interface("host.rssi").expect("interface HIR");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &interface));
        assert_eq!(session.stats().hir_cache_hits, 1);
        assert_eq!(session.stats().hir_cache_misses, 2);

        session
            .set_file("main.rss", "fn main() -> Int { return 1 }")
            .unwrap();
        let replacement = session.hir_file("main.rss").expect("replacement HIR");
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().hir_cache_misses, 3);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.hir_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        ));

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.hir_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        ));
        assert_eq!(session.stats().hir_cache_misses, 3);
    }

    #[test]
    fn compilation_session_caches_namespace_isolated_workspace_hir() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nfn helper() -> Int { return 1 }\nfn main() -> Int { return helper() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();

        let first = session.workspace_hir();
        assert!(first.function_body("app__helper").is_some());
        assert!(first.function_body("main").is_some());
        let second = session.workspace_hir();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_hir_cache_misses, 1);
        assert_eq!(session.stats().workspace_hir_cache_hits, 1);

        session
            .set_interface("host.rssi", "module host\npub fn next() -> Int\n")
            .unwrap();
        let replacement = session.workspace_hir();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().workspace_hir_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_hir_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
    }

    #[test]
    fn compilation_session_caches_workspace_type_facts_with_hir_revisions() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "fn main(value: read Int) -> Int { return value }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();

        let first = session.workspace_type_facts();
        assert!(first.functions().any(|(name, _)| name == "main"));
        let second = session.workspace_type_facts();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_type_cache_misses, 1);
        assert_eq!(session.stats().workspace_type_cache_hits, 1);

        session
            .set_interface("host.rssi", "module host\npub fn next() -> Int\n")
            .unwrap();
        let replacement = session.workspace_type_facts();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().workspace_type_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_type_facts_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert_eq!(session.stats().workspace_type_cache_hits, 1);
    }

    #[test]
    fn compilation_session_caches_parsed_module_headers() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "// use ignored.*\nmodule app.core\nuse host.api as host\n",
            )
            .unwrap();
        let first = session.module_header("main.rss").unwrap();
        let second = session.module_header("main.rss").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.modules(), ["app.core"]);
        assert_eq!(first.imports(), ["host.api"]);
        assert_eq!(
            session.stats(),
            CompilationSessionStats {
                parse_cache_hits: 0,
                parse_cache_misses: 1,
                hir_cache_hits: 0,
                hir_cache_misses: 0,
                workspace_hir_cache_hits: 0,
                workspace_hir_cache_misses: 0,
                workspace_type_cache_hits: 0,
                workspace_type_cache_misses: 0,
                workspace_analysis_cache_hits: 0,
                workspace_analysis_cache_misses: 0,
                module_header_cache_hits: 1,
                module_header_cache_misses: 1,
                workspace_module_graph_cache_hits: 0,
                workspace_module_graph_cache_misses: 0,
                workspace_diagnostic_cache_hits: 0,
                workspace_diagnostic_cache_misses: 0,
            }
        );
    }

    #[test]
    fn module_header_queries_reject_cancelled_and_expired_cached_requests() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "module app.core\nuse host.api\n")
            .unwrap();
        session
            .set_interface("host.rssi", "module host.api\npub fn value() -> Unit\n")
            .unwrap();
        let source = session.module_header("main.rss").unwrap();
        let interface = session.interface_module_header("host.rssi").unwrap();

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert_eq!(
            session.module_header_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.interface_module_header_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert_eq!(
            session.module_header_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );
        assert_eq!(
            session.interface_module_header_with_operation("host.rssi", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );

        let live = OperationContext::default();
        assert!(Arc::ptr_eq(
            &source,
            &session
                .module_header_with_operation("main.rss", &live)
                .unwrap()
                .unwrap()
        ));
        assert!(Arc::ptr_eq(
            &interface,
            &session
                .interface_module_header_with_operation("host.rssi", &live)
                .unwrap()
                .unwrap()
        ));
    }
}
