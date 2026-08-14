//! Type-name, field-shape, and resource-type validation.
//!
//! Kept as a named phase so the self-hosted checker can mirror this boundary
//! without coupling its partial AST to Rust implementation details.

use crate::analyzer::Analyzer;

pub(crate) fn check_names(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::cyclic_type_alias_diagnostics(
            &analyzer.interface_programs,
            &analyzer.syntax_program,
        ));
    analyzer.check_unknown_types();
    analyzer.check_unknown_fields();
    analyzer.check_unknown_bindings();
    analyzer
        .diagnostics
        .extend(rsscript_semantics::fd_surface_diagnostics(
            &analyzer.syntax_program,
        ));
}

pub(crate) fn check_resource_shapes(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::resource_field_diagnostics(
            &analyzer.hir,
            &analyzer.syntax_program,
        ));
    analyzer
        .diagnostics
        .extend(rsscript_semantics::weak_field_diagnostics(
            &analyzer.hir,
            &analyzer.syntax_program,
        ));
    analyzer.check_resource_generic_arguments();
}
