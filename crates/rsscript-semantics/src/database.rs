use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use rsscript_diagnostics::Diagnostic;
use rsscript_operation::{OperationAbort, OperationContext};
use rsscript_source_model::{FileId, SourceRevision};
use rsscript_syntax::{
    ast::{Item, Program},
    format_source, lint_source, parse_source,
};

use crate::hir::Hir;
use crate::{InterfaceDescriptorError, InterfaceDescriptorV1};
use crate::{
    RssDocumentSymbol, SemanticTypeFacts, SymbolIndex, document_symbols_from_program,
    symbol_index_from_program,
};

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
    semantic_document_analysis_cache: BTreeMap<SemanticDocumentCacheKey, Arc<AnalysisResult>>,
    semantic_document_diagnostic_cache: BTreeMap<SemanticDocumentCacheKey, Arc<[Diagnostic]>>,
    syntax_diagnostic_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<[Diagnostic]>>,
    lint_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<[Diagnostic]>>,
    format_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<str>>,
    symbol_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<SymbolIndex>>,
    document_symbol_cache:
        BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<[RssDocumentSymbol]>>,
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
    lint_cache_hits: u64,
    lint_cache_misses: u64,
    format_cache_hits: u64,
    format_cache_misses: u64,
    symbol_cache_hits: u64,
    symbol_cache_misses: u64,
    document_symbol_cache_hits: u64,
    document_symbol_cache_misses: u64,
    module_header_cache_hits: u64,
    module_header_cache_misses: u64,
    workspace_module_graph_cache_hits: u64,
    workspace_module_graph_cache_misses: u64,
    workspace_diagnostic_cache_hits: u64,
    workspace_diagnostic_cache_misses: u64,
    semantic_document_analysis_cache_hits: u64,
    semantic_document_analysis_cache_misses: u64,
    semantic_document_diagnostic_cache_hits: u64,
    semantic_document_diagnostic_cache_misses: u64,
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
    pub lint_cache_hits: u64,
    pub lint_cache_misses: u64,
    pub format_cache_hits: u64,
    pub format_cache_misses: u64,
    pub symbol_cache_hits: u64,
    pub symbol_cache_misses: u64,
    pub document_symbol_cache_hits: u64,
    pub document_symbol_cache_misses: u64,
    pub module_header_cache_hits: u64,
    pub module_header_cache_misses: u64,
    pub workspace_module_graph_cache_hits: u64,
    pub workspace_module_graph_cache_misses: u64,
    pub workspace_diagnostic_cache_hits: u64,
    pub workspace_diagnostic_cache_misses: u64,
    /// Hits for one-document semantic analyses keyed by the document revision
    /// and the resolved interface closure.
    pub semantic_document_analysis_cache_hits: u64,
    /// Misses for one-document semantic analyses keyed by the document
    /// revision and the resolved interface closure.
    pub semantic_document_analysis_cache_misses: u64,
    /// Hits for one-file semantic diagnostics keyed by the source revision and
    /// its resolved interface closure.
    pub semantic_document_diagnostic_cache_hits: u64,
    /// Misses for one-file semantic diagnostics keyed by the source revision
    /// and its resolved interface closure.
    pub semantic_document_diagnostic_cache_misses: u64,
}

/// Immutable dependency key for a document-level semantic query.
///
/// A source file's semantic diagnostics depend on its own revision plus the
/// revisions of the interface files reachable through parsed `use` imports.
/// Keeping that closure in the key makes unrelated interface edits cache hits
/// without allowing an edited imported contract to serve stale diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SemanticDocumentCacheKey {
    role: SessionFileRole,
    file_id: FileId,
    revision: SourceRevision,
    visible_sources: Vec<(FileId, SourceRevision)>,
    visible_interfaces: Vec<(FileId, SourceRevision)>,
}

/// Immutable input for one document-level semantic query.
///
/// It intentionally retains the exact interface revisions selected from the
/// session-owned module graph. Resolve, type, HIR, and diagnostics can then
/// share the same dependency key instead of each editor request rebuilding a
/// competing partial workspace.
#[derive(Debug, Clone)]
struct SemanticDocumentInput {
    key: SemanticDocumentCacheKey,
    path: Arc<str>,
    text: Arc<str>,
    sources: Vec<SourceFileSnapshot>,
    interfaces: Vec<SourceFileSnapshot>,
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
        self.visible_paths(current_path, root_imports, true)
    }

    /// Find source files reachable from one source document's parsed imports.
    ///
    /// Source imports participate in the same namespace resolver as interface
    /// imports, so an editor query must analyze their closure rather than
    /// silently type-checking the current file against only host interfaces.
    pub fn visible_source_paths(
        &self,
        current_path: &str,
        root_imports: impl IntoIterator<Item = String>,
    ) -> BTreeSet<String> {
        self.visible_paths(current_path, root_imports, false)
    }

    fn visible_paths(
        &self,
        current_path: &str,
        root_imports: impl IntoIterator<Item = String>,
        is_interface: bool,
    ) -> BTreeSet<String> {
        let mut imports = root_imports.into_iter().collect::<BTreeSet<_>>();
        let mut visible = BTreeSet::new();
        loop {
            let mut changed = false;
            for node in self
                .nodes
                .iter()
                .filter(|node| node.is_interface == is_interface)
            {
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
        let path = path.into();
        let previous_header = self.cached_module_header(SessionFileRole::Source, &path);
        let update = self.sources.set_file(path.clone(), text)?;
        let graph_unchanged = update.changed
            && self.module_header_is_unchanged(
                SessionFileRole::Source,
                &path,
                previous_header.as_deref(),
            );
        if update.changed {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_semantic_document_cache_for_source(update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
            if !graph_unchanged {
                self.workspace_module_graph_cache = None;
            }
            self.workspace_diagnostic_cache = None;
        }
        Ok(update)
    }

    pub fn remove_file(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.sources.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_semantic_document_cache_for_source(update.file_id);
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
        let path = path.into();
        let previous_header = self.cached_module_header(SessionFileRole::Interface, &path);
        let update = self.interfaces.set_file(path.clone(), text)?;
        let graph_unchanged = update.changed
            && self.module_header_is_unchanged(
                SessionFileRole::Interface,
                &path,
                previous_header.as_deref(),
            );
        if update.changed {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_semantic_document_cache_for_interface(update.file_id);
            self.workspace_hir_cache = None;
            self.workspace_type_cache = None;
            self.workspace_analysis_cache = None;
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
            if !graph_unchanged {
                self.workspace_module_graph_cache = None;
            }
            self.workspace_diagnostic_cache = None;
        }
        Ok(update)
    }

    pub fn remove_interface(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.interfaces.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_syntax_diagnostic_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_editor_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_semantic_document_cache_for_interface(update.file_id);
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
        // A session owns one immutable workspace input, but a one-source
        // workspace must retain the historical single-document semantic path.
        // That path carries source-local protocol and generic facts which are
        // not yet represented by the package merge query. Selecting it here
        // preserves semantics without letting callers bypass the session's
        // revision, cancellation, or cache boundary.
        let analysis = Arc::new(match (sources.as_slice(), operation) {
            ([(path, source)], Some(operation)) => {
                crate::analyze_source_with_interfaces_result_with_operation(
                    path,
                    source,
                    &interfaces,
                    operation,
                )
            }
            ([(path, source)], None) => {
                crate::analyze_source_with_interfaces_result(path, source, &interfaces)
            }
            (_, Some(operation)) => crate::analyze_sources_with_interfaces_result_with_operation(
                &sources,
                &interfaces,
                operation,
            ),
            (_, None) => crate::analyze_sources_with_interfaces_result(&sources, &interfaces),
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

    /// Diagnose one source document through the session-owned semantic query
    /// cache. The cache key contains only this source revision and the
    /// interface closure selected by its parsed imports, so changing an
    /// unrelated interface cannot invalidate an editor request for this file.
    pub fn semantic_diagnostics_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.source_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let diagnostics = self
            .semantic_diagnostics_snapshot_file(SessionFileRole::Source, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(diagnostics)
    }

    /// Resolve, type-check, and build HIR for one source document through the
    /// session-owned dependency query. The result is keyed by the document's
    /// immutable revision plus only the interface files visible from its
    /// parsed imports, so unrelated interface edits retain this work.
    pub fn semantic_analysis_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<AnalysisResult>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.source_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let analysis = self
            .semantic_analysis_snapshot_file(SessionFileRole::Source, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(analysis)
    }

    /// Diagnose one interface document through the same dependency-precise
    /// semantic query cache used for source files.
    pub fn semantic_diagnostics_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.interface_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let diagnostics = self
            .semantic_diagnostics_snapshot_file(SessionFileRole::Interface, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(diagnostics)
    }

    /// Resolve, type-check, and build HIR for one interface document through
    /// the same dependency-precise semantic query used for source files.
    pub fn semantic_analysis_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<AnalysisResult>>, OperationAbort> {
        operation.check()?;
        let snapshot = self.interface_snapshot();
        let Some(file) = snapshot.files().iter().find(|file| file.path() == path) else {
            return Ok(None);
        };
        let analysis = self
            .semantic_analysis_snapshot_file(SessionFileRole::Interface, file, operation)
            .map(Some)?;
        operation.check()?;
        Ok(analysis)
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

    /// Return syntax-only diagnostics for one source-file revision. This is a
    /// session-owned query for formatter/editor preflight: it shares stable
    /// file identity and invalidation with parse/HIR queries, but deliberately
    /// does not pull in workspace resolution or builtin interfaces.
    pub fn syntax_diagnostics_file(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.syntax_diagnostics_snapshot_file(SessionFileRole::Source, file)
    }

    /// Operation-aware syntax-diagnostics query. A cached result cannot escape
    /// after cancellation or deadline expiry, matching the other session
    /// query boundaries.
    pub fn syntax_diagnostics_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.syntax_diagnostics_file(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Return syntax-only diagnostics for one interface-file revision.
    pub fn syntax_diagnostics_interface(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.syntax_diagnostics_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Interface syntax diagnostics obey the same operation boundary as every
    /// other session-owned query. This matters for editor requests that only
    /// touch an interface document and would otherwise bypass cancellation on
    /// a cached result.
    pub fn syntax_diagnostics_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.syntax_diagnostics_interface(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Format one source revision through the session-owned editor cache.
    /// Formatting is syntax-only, but its result still belongs to the same
    /// immutable file identity as parse, HIR, lint, and symbols so editor
    /// clients cannot retain a competing revision cache.
    pub fn format_file(&mut self, path: &str) -> Option<Arc<str>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.format_snapshot_file(SessionFileRole::Source, file)
    }

    /// Format one interface revision through the same role-separated cache.
    pub fn format_interface(&mut self, path: &str) -> Option<Arc<str>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.format_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Format one source revision while observing cancellation and deadline
    /// both before a cache lookup and before returning a cached value.
    pub fn format_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<str>>, OperationAbort> {
        operation.check()?;
        let formatted = self.format_file(path);
        operation.check()?;
        Ok(formatted)
    }

    /// Operation-aware interface formatting with the same cache boundary as
    /// source formatting.
    pub fn format_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<str>>, OperationAbort> {
        operation.check()?;
        let formatted = self.format_interface(path);
        operation.check()?;
        Ok(formatted)
    }

    /// Return lint diagnostics for one source revision. Unlike complete
    /// workspace diagnostics this intentionally stays local and syntax-only,
    /// which keeps formatter/editor preflight cheap without creating a second
    /// cache in the language service.
    pub fn lint_file(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.lint_snapshot_file(SessionFileRole::Source, file)
    }

    /// Interface linting uses the same session cache and stable identity as
    /// source linting.
    pub fn lint_interface(&mut self, path: &str) -> Option<Arc<[Diagnostic]>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.lint_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Operation-aware source lint query.
    pub fn lint_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.lint_file(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Operation-aware interface lint query.
    pub fn lint_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[Diagnostic]>>, OperationAbort> {
        operation.check()?;
        let diagnostics = self.lint_interface(path);
        operation.check()?;
        Ok(diagnostics)
    }

    /// Return syntax-derived symbols for one source revision from the shared
    /// session. Semantic consumers can layer richer facts over this stable
    /// document query without reparsing the text.
    pub fn symbol_index_file(&mut self, path: &str) -> Option<Arc<SymbolIndex>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.symbol_index_snapshot_file(SessionFileRole::Source, file)
    }

    pub fn symbol_index_interface(&mut self, path: &str) -> Option<Arc<SymbolIndex>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.symbol_index_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Operation-aware source symbol-index query.
    pub fn symbol_index_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<SymbolIndex>>, OperationAbort> {
        operation.check()?;
        let symbols = self.symbol_index_file(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Operation-aware interface symbol-index query.
    pub fn symbol_index_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<SymbolIndex>>, OperationAbort> {
        operation.check()?;
        let symbols = self.symbol_index_interface(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Return document symbols for one source revision from the session-owned
    /// parse-derived cache.
    pub fn document_symbols_file(&mut self, path: &str) -> Option<Arc<[RssDocumentSymbol]>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.document_symbols_snapshot_file(SessionFileRole::Source, file)
    }

    pub fn document_symbols_interface(&mut self, path: &str) -> Option<Arc<[RssDocumentSymbol]>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.document_symbols_snapshot_file(SessionFileRole::Interface, file)
    }

    /// Operation-aware source document-symbol query.
    pub fn document_symbols_file_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[RssDocumentSymbol]>>, OperationAbort> {
        operation.check()?;
        let symbols = self.document_symbols_file(path);
        operation.check()?;
        Ok(symbols)
    }

    /// Operation-aware interface document-symbol query.
    pub fn document_symbols_interface_with_operation(
        &mut self,
        path: &str,
        operation: &OperationContext,
    ) -> Result<Option<Arc<[RssDocumentSymbol]>>, OperationAbort> {
        operation.check()?;
        let symbols = self.document_symbols_interface(path);
        operation.check()?;
        Ok(symbols)
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

    /// Return the checked, namespace-isolated workspace HIR for the current
    /// immutable source/interface revisions.
    ///
    /// The HIR is projected from the session's complete analysis query rather
    /// than independently merging parse trees. That makes resolve, type, HIR,
    /// and diagnostics facts originate from one immutable revision set and
    /// prevents editor consumers from accidentally observing a second,
    /// structurally-similar but semantically different workspace HIR.
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

        let analysis = match operation {
            Some(operation) => self.workspace_analysis_with_operation(operation)?,
            None => self.workspace_analysis(),
        };
        let hir = Arc::new(analysis.database().hir().clone());
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
            lint_cache_hits: self.lint_cache_hits,
            lint_cache_misses: self.lint_cache_misses,
            format_cache_hits: self.format_cache_hits,
            format_cache_misses: self.format_cache_misses,
            symbol_cache_hits: self.symbol_cache_hits,
            symbol_cache_misses: self.symbol_cache_misses,
            document_symbol_cache_hits: self.document_symbol_cache_hits,
            document_symbol_cache_misses: self.document_symbol_cache_misses,
            module_header_cache_hits: self.module_header_cache_hits,
            module_header_cache_misses: self.module_header_cache_misses,
            workspace_module_graph_cache_hits: self.workspace_module_graph_cache_hits,
            workspace_module_graph_cache_misses: self.workspace_module_graph_cache_misses,
            workspace_diagnostic_cache_hits: self.workspace_diagnostic_cache_hits,
            workspace_diagnostic_cache_misses: self.workspace_diagnostic_cache_misses,
            semantic_document_analysis_cache_hits: self.semantic_document_analysis_cache_hits,
            semantic_document_analysis_cache_misses: self.semantic_document_analysis_cache_misses,
            semantic_document_diagnostic_cache_hits: self.semantic_document_diagnostic_cache_hits,
            semantic_document_diagnostic_cache_misses: self
                .semantic_document_diagnostic_cache_misses,
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

    fn invalidate_syntax_diagnostic_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.syntax_diagnostic_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    /// Evict every syntax-derived editor query for one changed/deleted file.
    /// The language service deliberately does not mirror these maps: file ID,
    /// role, and revision remain the single cache key owned by the session.
    fn invalidate_editor_cache(&mut self, role: SessionFileRole, file_id: FileId) {
        self.lint_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
        self.format_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
        self.symbol_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
        self.document_symbol_cache
            .retain(|(cached_role, cached_id, _), _| *cached_role != role || *cached_id != file_id);
    }

    fn invalidate_semantic_document_cache_for_source(&mut self, file_id: FileId) {
        let retains_source = |key: &SemanticDocumentCacheKey| {
            !(key.role == SessionFileRole::Source && key.file_id == file_id)
                && !key
                    .visible_sources
                    .iter()
                    .any(|(dependency_id, _)| *dependency_id == file_id)
        };
        self.semantic_document_analysis_cache
            .retain(|key, _| retains_source(key));
        self.semantic_document_diagnostic_cache
            .retain(|key, _| retains_source(key));
    }

    fn invalidate_semantic_document_cache_for_interface(&mut self, file_id: FileId) {
        self.semantic_document_analysis_cache.retain(|key, _| {
            !(key.role == SessionFileRole::Interface && key.file_id == file_id)
                && !key
                    .visible_interfaces
                    .iter()
                    .any(|(dependency_id, _)| *dependency_id == file_id)
        });
        self.semantic_document_diagnostic_cache.retain(|key, _| {
            !(key.role == SessionFileRole::Interface && key.file_id == file_id)
                && !key
                    .visible_interfaces
                    .iter()
                    .any(|(dependency_id, _)| *dependency_id == file_id)
        });
    }

    fn semantic_diagnostics_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
        operation: &OperationContext,
    ) -> Result<Arc<[Diagnostic]>, OperationAbort> {
        let input = self.semantic_document_input(role, file, operation)?;
        if let Some(diagnostics) = self.semantic_document_diagnostic_cache.get(&input.key) {
            self.semantic_document_diagnostic_cache_hits = self
                .semantic_document_diagnostic_cache_hits
                .saturating_add(1);
            operation.check()?;
            return Ok(Arc::clone(diagnostics));
        }

        let diagnostics: Arc<[Diagnostic]> = self
            .semantic_analysis_input(&input, operation)?
            .diagnostics()
            .iter()
            .cloned()
            .into_iter()
            .filter(|diagnostic| diagnostic.span.file == input.path.as_ref())
            .collect::<Vec<_>>()
            .into();
        operation.check()?;
        self.semantic_document_diagnostic_cache_misses = self
            .semantic_document_diagnostic_cache_misses
            .saturating_add(1);
        self.semantic_document_diagnostic_cache
            .insert(input.key, Arc::clone(&diagnostics));
        Ok(diagnostics)
    }

    fn semantic_analysis_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
        operation: &OperationContext,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        let input = self.semantic_document_input(role, file, operation)?;
        self.semantic_analysis_input(&input, operation)
    }

    fn semantic_document_input(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
        operation: &OperationContext,
    ) -> Result<SemanticDocumentInput, OperationAbort> {
        operation.check()?;
        let graph = self.workspace_module_graph_with_operation(operation)?;
        let root_imports = match role {
            SessionFileRole::Source => graph.source(file.path()),
            SessionFileRole::Interface => graph.interface(file.path()),
        }
        .map(|node| node.imports().to_vec())
        .unwrap_or_default();
        let visible_paths = graph.visible_interface_paths(file.path(), root_imports);
        let source_paths = if role == SessionFileRole::Source {
            let root_imports = graph
                .source(file.path())
                .map(|node| node.imports().to_vec())
                .unwrap_or_default();
            graph.visible_source_paths(file.path(), root_imports)
        } else {
            BTreeSet::new()
        };
        let sources = self
            .source_snapshot()
            .files()
            .iter()
            .filter(|candidate| candidate.path() != file.path())
            .filter(|candidate| source_paths.contains(candidate.path()))
            .cloned()
            .collect::<Vec<_>>();
        let interfaces = self
            .interface_snapshot()
            .files()
            .iter()
            .filter(|candidate| candidate.path() != file.path())
            .filter(|candidate| visible_paths.contains(candidate.path()))
            .cloned()
            .collect::<Vec<_>>();
        Ok(SemanticDocumentInput {
            key: SemanticDocumentCacheKey {
                role,
                file_id: file.file_id(),
                revision: file.revision(),
                visible_sources: sources
                    .iter()
                    .map(|dependency| (dependency.file_id(), dependency.revision()))
                    .collect(),
                visible_interfaces: interfaces
                    .iter()
                    .map(|dependency| (dependency.file_id(), dependency.revision()))
                    .collect(),
            },
            path: Arc::clone(&file.path),
            text: Arc::clone(&file.text),
            sources,
            interfaces,
        })
    }

    fn semantic_analysis_input(
        &mut self,
        input: &SemanticDocumentInput,
        operation: &OperationContext,
    ) -> Result<Arc<AnalysisResult>, OperationAbort> {
        operation.check()?;
        if let Some(analysis) = self.semantic_document_analysis_cache.get(&input.key) {
            self.semantic_document_analysis_cache_hits =
                self.semantic_document_analysis_cache_hits.saturating_add(1);
            operation.check()?;
            return Ok(Arc::clone(analysis));
        }
        let interface_slices = input
            .interfaces
            .iter()
            .map(|dependency| (dependency.path(), dependency.text()))
            .collect::<Vec<_>>();
        let analysis = if input.sources.is_empty() {
            Arc::new(crate::analyze_source_with_interfaces_result_with_operation(
                input.path.as_ref(),
                input.text.as_ref(),
                &interface_slices,
                operation,
            ))
        } else {
            let mut source_slices = Vec::with_capacity(input.sources.len().saturating_add(1));
            source_slices.push((input.path.as_ref(), input.text.as_ref()));
            source_slices.extend(
                input
                    .sources
                    .iter()
                    .map(|dependency| (dependency.path(), dependency.text())),
            );
            Arc::new(
                crate::analyze_sources_with_interfaces_result_with_operation(
                    &source_slices,
                    &interface_slices,
                    operation,
                ),
            )
        };
        operation.check()?;
        self.semantic_document_analysis_cache_misses = self
            .semantic_document_analysis_cache_misses
            .saturating_add(1);
        self.semantic_document_analysis_cache
            .insert(input.key.clone(), Arc::clone(&analysis));
        Ok(analysis)
    }

    fn syntax_diagnostics_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<[Diagnostic]>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(diagnostics) = self.syntax_diagnostic_cache.get(&key) {
            return Some(Arc::clone(diagnostics));
        }
        let diagnostics = Arc::from(crate::analyze_syntax_source(file.path(), file.text()));
        self.syntax_diagnostic_cache
            .insert(key, Arc::clone(&diagnostics));
        Some(diagnostics)
    }

    fn lint_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<[Diagnostic]>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(diagnostics) = self.lint_cache.get(&key) {
            self.lint_cache_hits = self.lint_cache_hits.saturating_add(1);
            return Some(Arc::clone(diagnostics));
        }
        let diagnostics = Arc::from(lint_source(file.path(), file.text()));
        self.lint_cache.insert(key, Arc::clone(&diagnostics));
        self.lint_cache_misses = self.lint_cache_misses.saturating_add(1);
        Some(diagnostics)
    }

    fn format_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<str>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(formatted) = self.format_cache.get(&key) {
            self.format_cache_hits = self.format_cache_hits.saturating_add(1);
            return Some(Arc::clone(formatted));
        }
        let formatted: Arc<str> = format_source(file.path(), file.text()).into();
        self.format_cache.insert(key, Arc::clone(&formatted));
        self.format_cache_misses = self.format_cache_misses.saturating_add(1);
        Some(formatted)
    }

    fn symbol_index_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<SymbolIndex>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(symbols) = self.symbol_cache.get(&key) {
            self.symbol_cache_hits = self.symbol_cache_hits.saturating_add(1);
            return Some(Arc::clone(symbols));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let symbols = Arc::new(symbol_index_from_program(file.text(), &program));
        self.symbol_cache.insert(key, Arc::clone(&symbols));
        self.symbol_cache_misses = self.symbol_cache_misses.saturating_add(1);
        Some(symbols)
    }

    fn document_symbols_snapshot_file(
        &mut self,
        role: SessionFileRole,
        file: &SourceFileSnapshot,
    ) -> Option<Arc<[RssDocumentSymbol]>> {
        let key = (role, file.file_id(), file.revision());
        if let Some(symbols) = self.document_symbol_cache.get(&key) {
            self.document_symbol_cache_hits = self.document_symbol_cache_hits.saturating_add(1);
            return Some(Arc::clone(symbols));
        }
        let program = self.parse_snapshot_file(role, file)?;
        let symbols: Arc<[RssDocumentSymbol]> = document_symbols_from_program(&program).into();
        self.document_symbol_cache.insert(key, Arc::clone(&symbols));
        self.document_symbol_cache_misses = self.document_symbol_cache_misses.saturating_add(1);
        Some(symbols)
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

    /// Return the current header only when a workspace graph is already cached.
    ///
    /// The graph contains exactly the path, role, module declarations and import
    /// declarations represented by this header. Recomputing a graph after an
    /// implementation-only edit therefore wastes a whole-workspace query and,
    /// for editor clients, causes avoidable dependency churn. We intentionally
    /// keep the comparison local to this syntax query: type/HIR/diagnostic
    /// caches still invalidate on every changed byte until their dependency
    /// edges are independently precise.
    fn cached_module_header(
        &mut self,
        role: SessionFileRole,
        path: &str,
    ) -> Option<Arc<ModuleHeader>> {
        self.workspace_module_graph_cache.as_ref()?;
        match role {
            SessionFileRole::Source => self.module_header(path),
            SessionFileRole::Interface => self.interface_module_header(path),
        }
    }

    fn module_header_is_unchanged(
        &mut self,
        role: SessionFileRole,
        path: &str,
        previous: Option<&ModuleHeader>,
    ) -> bool {
        let Some(previous) = previous else {
            return false;
        };
        let current = match role {
            SessionFileRole::Source => self.module_header(path),
            SessionFileRole::Interface => self.interface_module_header(path),
        };
        current.as_deref() == Some(previous)
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
    use crate::hir::{HirExpr, HirStmt};
    use crate::validate_sources_with_interfaces;
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
    fn checked_hir_retains_declared_closure_contracts() {
        let validated = validate_sources_with_interfaces(
            &[(
                "closure-contract.rss",
                r#"
fn main() -> Int {
    let offset = 40
    let add: Fn(Int) -> Int = fn(value) captures(read offset) {
        return value + offset
    }
    return add(2)
}
"#,
            )],
            &[],
        )
        .expect("annotated closure source validates");
        let body = validated
            .database()
            .hir()
            .function_body("main")
            .expect("main HIR body exists");
        let block = body.block.as_ref().expect("main body is lowered");
        let HirStmt::Let {
            value: Some(HirExpr::Closure { ty: Some(ty), .. }),
            ..
        } = &block.statements[1]
        else {
            panic!("closure must retain its structural Fn contract")
        };

        assert!(ty.is_function());
        assert_eq!(ty.to_string(), "Fn(read Int) -> Int");
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
                ..CompilationSessionStats::default()
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
    fn compilation_session_owns_revisioned_editor_queries() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main()->Int{return 1}\n")
            .expect("source enters session");

        let first_format = session.format_file("main.rss").expect("format");
        let second_format = session.format_file("main.rss").expect("cached format");
        assert!(Arc::ptr_eq(&first_format, &second_format));
        let first_lint = session.lint_file("main.rss").expect("lint");
        let second_lint = session.lint_file("main.rss").expect("cached lint");
        assert!(Arc::ptr_eq(&first_lint, &second_lint));
        let first_symbols = session.symbol_index_file("main.rss").expect("symbols");
        let second_symbols = session
            .symbol_index_file("main.rss")
            .expect("cached symbols");
        assert!(Arc::ptr_eq(&first_symbols, &second_symbols));
        let first_document_symbols = session
            .document_symbols_file("main.rss")
            .expect("document symbols");
        let second_document_symbols = session
            .document_symbols_file("main.rss")
            .expect("cached document symbols");
        assert!(Arc::ptr_eq(
            &first_document_symbols,
            &second_document_symbols
        ));

        let stats = session.stats();
        assert_eq!((stats.format_cache_misses, stats.format_cache_hits), (1, 1));
        assert_eq!((stats.lint_cache_misses, stats.lint_cache_hits), (1, 1));
        assert_eq!((stats.symbol_cache_misses, stats.symbol_cache_hits), (1, 1));
        assert_eq!(
            (
                stats.document_symbol_cache_misses,
                stats.document_symbol_cache_hits,
            ),
            (1, 1)
        );

        session
            .set_file("main.rss", "fn main() -> Int { return 2 }\n")
            .expect("replacement invalidates editor queries");
        let formatted = session.format_file("main.rss").expect("replacement format");
        assert!(formatted.contains("return 2"));
        let symbols = session
            .document_symbols_file("main.rss")
            .expect("replacement document symbols");
        assert_eq!(symbols[0].name, "main");
        let stats = session.stats();
        assert_eq!(stats.format_cache_misses, 2);
        assert_eq!(stats.document_symbol_cache_misses, 2);
    }

    #[test]
    fn editor_queries_reject_cancelled_and_expired_cached_requests() {
        use std::time::{Duration, Instant};

        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }\n")
            .expect("source enters session");
        session
            .set_interface("host.rssi", "module host\npub fn emit() -> Unit\n")
            .expect("interface enters session");

        // Warm every cache that is exposed to editor clients. Operation-aware
        // queries must reject a later cancelled/expired request rather than
        // letting these cached values escape.
        session.format_file("main.rss");
        session.format_interface("host.rssi");
        session.lint_file("main.rss");
        session.lint_interface("host.rssi");
        session.symbol_index_file("main.rss");
        session.symbol_index_interface("host.rssi");
        session.document_symbols_file("main.rss");
        session.document_symbols_interface("host.rssi");
        session.syntax_diagnostics_interface("host.rssi");

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert_eq!(
            session.format_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.format_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.lint_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.lint_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert!(matches!(
            session.symbol_index_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert!(matches!(
            session.symbol_index_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert!(matches!(
            session.document_symbols_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert!(matches!(
            session.document_symbols_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert_eq!(
            session.syntax_diagnostics_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert_eq!(
            session.format_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );
        assert!(matches!(
            session.symbol_index_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        ));
        assert!(matches!(
            session.document_symbols_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        ));
    }

    #[test]
    fn compilation_session_caches_syntax_diagnostics_by_revision() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();

        let first = session
            .syntax_diagnostics_file("main.rss")
            .expect("source exists");
        let second = session
            .syntax_diagnostics_file("main.rss")
            .expect("cached source exists");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(first.is_empty());

        session
            .set_file("main.rss", "fn main( { return Unit }")
            .unwrap();
        let replacement = session
            .syntax_diagnostics_file("main.rss")
            .expect("replacement source exists");
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert!(
            replacement
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            session.syntax_diagnostics_file_with_operation(
                "main.rss",
                &OperationContext {
                    cancellation: Some(cancellation),
                    ..OperationContext::default()
                },
            ),
            Err(OperationAbort::Cancelled)
        );
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
    fn document_semantic_diagnostics_track_only_visible_interface_revisions() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.*\nfn main() -> Int { return value() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> Int\n")
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(
            first
                .iter()
                .all(|diagnostic| !diagnostic.severity.is_error())
        );
        let second = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &second));

        // The source imports `host`, not `other`, so this edit must retain the
        // same document query result while broad workspace diagnostics remain
        // free to recompute independently.
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> String\n")
            .unwrap();
        let unrelated = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &unrelated));
        assert_eq!(session.stats().semantic_document_diagnostic_cache_misses, 1);
        assert_eq!(session.stats().semantic_document_diagnostic_cache_hits, 2);

        // An imported contract changes the closure key and must force a fresh
        // check; the resulting return type mismatch proves the new contract is
        // the one observed by the document query.
        session
            .set_interface("host.rssi", "module host\npub fn value() -> String\n")
            .unwrap();
        let changed = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(
            changed
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
        assert_eq!(session.stats().semantic_document_diagnostic_cache_misses, 2);
    }

    #[test]
    fn document_semantic_analysis_reuses_resolve_type_and_hir_by_interface_closure() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.*\nfn main() -> Int { return value() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> Int\n")
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(first.database().hir().function_body("main").is_some());
        assert!(
            first
                .database()
                .hir()
                .semantic_types()
                .functions()
                .any(|(name, _)| name == "main")
        );
        let second = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &second));

        // The selected contract closure excludes `other`, so its edit cannot
        // invalidate already-resolved type/HIR facts for `main`.
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> String\n")
            .unwrap();
        let unrelated = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &unrelated));

        // A visible interface revision must invalidate the complete semantic
        // result, not merely document diagnostics.
        session
            .set_interface("host.rssi", "module host\npub fn value() -> String\n")
            .unwrap();
        let changed = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(
            changed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
        assert_eq!(session.stats().semantic_document_analysis_cache_misses, 2);
        assert_eq!(session.stats().semantic_document_analysis_cache_hits, 2);
    }

    #[test]
    fn document_semantic_analysis_tracks_imported_source_revisions() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse lib.value\nfn main() -> Int { return value() }\n",
            )
            .unwrap();
        session
            .set_file("lib.rss", "module lib\nfn value() -> Int { return 1 }\n")
            .unwrap();
        session
            .set_file(
                "other.rss",
                "module other\nfn ignored() -> Int { return 1 }\n",
            )
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(first.diagnostics().is_empty());
        assert!(first.database().hir().function_body("lib__value").is_some());

        // Unrelated source edits retain the imported-source closure and its
        // cached resolve/type/HIR facts.
        session
            .set_file(
                "other.rss",
                "module other\nfn ignored() -> Int { return 2 }\n",
            )
            .unwrap();
        let unrelated = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &unrelated));

        // A source module selected through `use` must invalidate the consumer
        // and produce diagnostics from the updated cross-source contract.
        session
            .set_file(
                "lib.rss",
                "module lib\nfn value() -> String { return \"changed\" }\n",
            )
            .unwrap();
        let changed = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(
            changed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
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
    fn workspace_module_graph_survives_implementation_only_edits() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.api\nfn main() -> Int { return Host.value() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host.api\npub fn value() -> Int\n")
            .unwrap();

        let first = session.workspace_module_graph();
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);

        // Changing executable bodies invalidates semantic queries, but does not
        // alter this syntax-only graph's node identity or import closure.
        session
            .set_file(
                "main.rss",
                "module app\nuse host.api\nfn main() -> Int { return Host.value() + 1 }\n",
            )
            .unwrap();
        let body_edit = session.workspace_module_graph();
        assert!(Arc::ptr_eq(&first, &body_edit));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);

        // An interface signature edit likewise requires fresh semantic facts,
        // while leaving the module/import graph valid for editor queries.
        session
            .set_interface(
                "host.rssi",
                "module host.api\npub fn value() -> Int\npub fn next() -> Int\n",
            )
            .unwrap();
        let signature_edit = session.workspace_module_graph();
        assert!(Arc::ptr_eq(&first, &signature_edit));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);

        // Import changes are graph changes and must rebuild instead of serving
        // the stale cached node closure.
        session
            .set_interface(
                "host.rssi",
                "module host.api\nuse host.base\npub fn value() -> Int\n",
            )
            .unwrap();
        let import_edit = session.workspace_module_graph();
        assert!(!Arc::ptr_eq(&first, &import_edit));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 2);
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

        let analysis = session.workspace_analysis();
        let analysis_types = analysis.database().hir().semantic_types_arc();
        let first = session.workspace_type_facts();
        assert!(first.functions().any(|(name, _)| name == "main"));
        assert!(Arc::ptr_eq(&first, &analysis_types));
        let second = session.workspace_type_facts();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_type_cache_misses, 1);
        assert_eq!(session.stats().workspace_type_cache_hits, 1);

        session
            .set_interface("host.rssi", "module host\npub fn next() -> Int\n")
            .unwrap();
        let replacement = session.workspace_type_facts();
        assert!(!Arc::ptr_eq(&first, &replacement));
        let replacement_analysis = session.workspace_analysis();
        assert!(Arc::ptr_eq(
            &replacement,
            &replacement_analysis.database().hir().semantic_types_arc()
        ));
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
                ..CompilationSessionStats::default()
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
