use std::sync::Arc;

use crate::diagnostic::Diagnostic;
use crate::hir::Hir;
use crate::syntax::ast::Program;

pub(crate) use rsscript_semantics::{ResolvedType, SemanticTypeFacts};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendCompletion {
    Complete,
    Incomplete(FrontendStopReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileSnapshot {
    path: Arc<str>,
    text: Arc<str>,
}

impl SourceFileSnapshot {
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
                .map(|(path, text)| SourceFileSnapshot {
                    path: Arc::from(path),
                    text: Arc::from(text),
                })
                .collect(),
        }
    }

    pub fn files(&self) -> &[SourceFileSnapshot] {
        &self.files
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Parsed and resolved facts produced by one bounded frontend run.
///
/// The constructor is crate-private so consumers cannot pair an arbitrary AST
/// or HIR with unrelated source bytes.
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
    pub(crate) fn new(
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

    /// Namespace-isolated merged syntax consumed by executable backends.
    ///
    /// Consumers may project declaration syntax needed for code emission, but
    /// must not repeat parsing or use this AST to replace checked HIR facts.
    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn interface_programs(&self) -> &[Program] {
        &self.interface_programs
    }

    pub fn hir(&self) -> &Hir {
        &self.hir
    }

    pub(crate) fn types(&self) -> &SemanticTypeFacts {
        self.types.as_ref()
    }

    pub fn interned_type_count(&self) -> usize {
        self.types.arena().len()
    }
}

/// Complete frontend output, including diagnostics for invalid programs.
#[derive(Debug)]
pub struct AnalysisResult {
    database: SemanticDatabase,
    diagnostics: Vec<Diagnostic>,
    completion: FrontendCompletion,
}

impl AnalysisResult {
    pub(crate) fn new(
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
    fn validated_program_requires_complete_error_free_analysis() {
        let validated =
            crate::analyzer::validate_source("main.rss", "fn main() -> Unit { return Unit }\n")
                .expect("clean source should validate");
        assert_eq!(validated.database().sources().files()[0].path(), "main.rss");
        assert_eq!(validated.database().source_programs().len(), 1);

        let diagnostics = crate::analyzer::validate_source(
            "invalid.rss",
            "fn main() -> Int { return Missing.value }\n",
        )
        .expect_err("frontend errors must not construct ValidatedProgram");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
    }

    #[test]
    fn semantic_database_interns_shared_signature_and_field_types() {
        let source = r#"
struct Holder<U> {
    value: U
}

fn first<U, W>(left: read U, right: read W) -> U {
    return left
}

fn main() -> Unit {
    let value: Int = first(left: read 1, right: read "unused")
    return Unit
}
"#;
        let validated =
            crate::analyzer::validate_source("structural-types.rss", source).expect("valid source");
        let database = validated.database();
        let types = database.types();
        let first = types
            .functions()
            .find(|(name, _)| *name == "first")
            .map(|(_, facts)| facts)
            .expect("first signature facts");
        let holder = types.named_type("Holder").expect("Holder type facts");

        assert_eq!(first.type_parameters.as_ref(), ["U", "W"]);
        assert_eq!(types.arena().get(first.parameters[0].1).to_string(), "U");
        assert_eq!(types.arena().get(first.parameters[1].1).to_string(), "W");
        assert_eq!(
            first
                .return_type
                .map(|ty| types.arena().get(ty).to_string())
                .as_deref(),
            Some("U")
        );
        assert_eq!(
            first.parameters[0].1, holder.fields[0].1,
            "structurally identical U facts must share one TypeId"
        );
        assert!(database.interned_type_count() >= 3);
    }
}
