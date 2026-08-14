use super::*;

impl Analyzer<'_> {
    pub(crate) fn unknown_type_name_diagnostic(
        &mut self,
        name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics
            .push(rsscript_semantics::unknown_type_name_diagnostic(name, span));
    }

    pub(crate) fn protocol_impl_mismatch_diagnostic(
        &mut self,
        protocol: &str,
        type_name: &str,
        method: &str,
        span: &crate::diagnostic::Span,
        label: impl Into<String>,
        cause: impl Into<String>,
    ) {
        self.diagnostics
            .push(rsscript_semantics::protocol_impl_mismatch_diagnostic(
                protocol, type_name, method, span, label, cause,
            ));
    }
}
