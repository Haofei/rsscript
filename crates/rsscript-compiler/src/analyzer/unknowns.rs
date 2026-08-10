use super::*;

impl Analyzer<'_> {
    pub(crate) fn check_unknown_types(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_type_diagnostics(
                &self.hir,
                &self.syntax_program,
                &self.visible_protocol_names(),
            ));
    }

    pub(crate) fn check_unknown_fields(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_field_diagnostics(&self.hir));
    }

    pub(crate) fn check_unknown_bindings(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_binding_diagnostics(
                &self.hir,
                &self.syntax_program,
            ));
    }
}
