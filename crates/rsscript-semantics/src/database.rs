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
                .map(|(index, (path, text))| SourceFileSnapshot {
                    file_id: FileId::new(
                        u32::try_from(index).expect("source snapshot exceeds u32 file IDs"),
                    ),
                    revision: SourceRevision::INITIAL,
                    path: Arc::from(path),
                    text: Arc::from(text),
                })
                .collect(),
        }
    }

    pub fn files(&self) -> &[SourceFileSnapshot] {
        &self.files
    }

    pub fn file(&self, id: FileId) -> Option<&SourceFileSnapshot> {
        self.files
            .get(id.get() as usize)
            .filter(|file| file.file_id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
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
}
