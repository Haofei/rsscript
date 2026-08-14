use super::*;

impl Analyzer<'_> {
    /// Resource containment is a semantic query over immutable source and HIR
    /// facts. Compiler orchestration appends its canonical diagnostics only.
    pub(crate) fn check_resource_generic_arguments(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::resource_generic_diagnostics(
                &self.hir,
                &self.syntax_program,
            ));
    }
}
