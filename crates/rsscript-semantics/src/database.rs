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

// `impl CompilationSession` lives in the child `session` module (module-size split).
mod session;

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
    InterfacesForbiddenByPolicy { policy: SessionInterfacePolicy },
    FileIdExhausted,
    RevisionExhausted { file_id: FileId },
}

/// Controls whether semantic workspace queries inject the language's Core
/// interfaces in addition to interfaces explicitly present in the session.
///
/// The policy belongs to the immutable session input boundary rather than a
/// caller-side analyzer choice. This keeps CLI, SDK, package, and editor
/// callers on the same query/cache path while making `--no-core` semantically
/// distinct from a normal workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionInterfacePolicy {
    #[default]
    WithCore,
    /// Use the historical standard-package prelude exactly as the legacy
    /// single-source analyzer does. This policy owns its prelude and therefore
    /// rejects caller-supplied interfaces.
    WithStandardPackages,
    WithoutCore,
}

impl fmt::Display for SourceStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("source path must not be empty"),
            Self::InterfacesForbiddenByPolicy { policy } => {
                write!(
                    formatter,
                    "session interface policy {policy:?} forbids explicit interfaces"
                )
            }
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
#[derive(Debug, Clone)]
pub struct CompilationSession {
    sources: SessionSourceStore,
    interfaces: SessionSourceStore,
    interface_policy: SessionInterfacePolicy,
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

impl Default for CompilationSession {
    fn default() -> Self {
        Self::with_interface_policy(SessionInterfacePolicy::WithCore)
    }
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
#[path = "database_tests.rs"]
mod tests;
