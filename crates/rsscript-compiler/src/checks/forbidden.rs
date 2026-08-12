//! Compiler adapter for syntax and operator semantic queries.

use crate::analyzer::Analyzer;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::forbidden_surface_syntax_diagnostics(
            analyzer.tokens,
        ));
    analyzer
        .diagnostics
        .extend(rsscript_semantics::builtin_operator_diagnostics(
            &analyzer.hir,
        ));
}
