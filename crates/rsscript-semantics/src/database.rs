use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use rsscript_diagnostics::Diagnostic;
use rsscript_source_model::{FileId, SourceRevision};
use rsscript_syntax::ast::Program;

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
}

impl CompilationSession {
    pub fn set_file(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        self.sources.set_file(path, text)
    }

    pub fn remove_file(&mut self, path: &str) -> Option<SourceUpdate> {
        self.sources.remove_file(path)
    }

    pub fn set_interface(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<SourceUpdate, SourceStoreError> {
        self.interfaces.set_file(path, text)
    }

    pub fn remove_interface(&mut self, path: &str) -> Option<SourceUpdate> {
        self.interfaces.remove_file(path)
    }

    pub fn source_snapshot(&self) -> SourceSnapshot {
        self.sources.snapshot()
    }

    pub fn interface_snapshot(&self) -> SourceSnapshot {
        self.interfaces.snapshot()
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
}
