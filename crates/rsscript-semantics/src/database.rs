use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use rsscript_diagnostics::Diagnostic;
use rsscript_operation::{OperationAbort, OperationContext};
use rsscript_source_model::{FileId, SourceRevision};
use rsscript_syntax::{
    ast::{Item, Program},
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
    module_header_cache: BTreeMap<(SessionFileRole, FileId, SourceRevision), Arc<ModuleHeader>>,
    parse_cache_hits: u64,
    parse_cache_misses: u64,
    hir_cache_hits: u64,
    hir_cache_misses: u64,
    module_header_cache_hits: u64,
    module_header_cache_misses: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompilationSessionStats {
    pub parse_cache_hits: u64,
    pub parse_cache_misses: u64,
    pub hir_cache_hits: u64,
    pub hir_cache_misses: u64,
    pub module_header_cache_hits: u64,
    pub module_header_cache_misses: u64,
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
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
        }
        Ok(update)
    }

    pub fn remove_file(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.sources.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Source, update.file_id);
            self.invalidate_module_header_cache(SessionFileRole::Source, update.file_id);
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
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
        }
        Ok(update)
    }

    pub fn remove_interface(&mut self, path: &str) -> Option<SourceUpdate> {
        let update = self.interfaces.remove_file(path);
        if let Some(update) = update {
            self.invalidate_parse_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_hir_cache(SessionFileRole::Interface, update.file_id);
            self.invalidate_module_header_cache(SessionFileRole::Interface, update.file_id);
        }
        update
    }

    pub fn source_snapshot(&self) -> SourceSnapshot {
        self.sources.snapshot()
    }

    pub fn interface_snapshot(&self) -> SourceSnapshot {
        self.interfaces.snapshot()
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

    /// Return parsed module and import paths for one source revision.
    pub fn module_header(&mut self, path: &str) -> Option<Arc<ModuleHeader>> {
        let snapshot = self.source_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.module_header_snapshot_file(SessionFileRole::Source, file)
    }

    /// Return parsed module and import paths for one interface revision.
    pub fn interface_module_header(&mut self, path: &str) -> Option<Arc<ModuleHeader>> {
        let snapshot = self.interface_snapshot();
        let file = snapshot.files().iter().find(|file| file.path() == path)?;
        self.module_header_snapshot_file(SessionFileRole::Interface, file)
    }

    pub fn stats(&self) -> CompilationSessionStats {
        CompilationSessionStats {
            parse_cache_hits: self.parse_cache_hits,
            parse_cache_misses: self.parse_cache_misses,
            hir_cache_hits: self.hir_cache_hits,
            hir_cache_misses: self.hir_cache_misses,
            module_header_cache_hits: self.module_header_cache_hits,
            module_header_cache_misses: self.module_header_cache_misses,
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
#[derive(Debug)]
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
#[derive(Debug)]
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
#[derive(Debug)]
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
                module_header_cache_hits: 0,
                module_header_cache_misses: 0,
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
                module_header_cache_hits: 1,
                module_header_cache_misses: 1,
            }
        );
    }
}
