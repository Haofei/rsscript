//! Declaration-level semantic checks.
//!
//! This is the Rust-side ownership boundary mirrored by
//! `selfhost/semantics/declarations.rss`.  The individual implementations still
//! live with their analysis helpers while the larger checker refactor proceeds;
//! this module fixes the pipeline's stable semantic grouping first.

use crate::analyzer::Analyzer;

#[path = "declarations/duplicate_decls.rs"]
mod duplicate_decls;
#[path = "declarations/signatures.rs"]
mod signatures;

/// Checks declaration identity and contract shape in the established diagnostic
/// order. Keep this separate from type-name resolution, which depends on the
/// declaration inventory built here but is a distinct semantic phase.
pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    duplicate_decls::check(analyzer);
    analyzer.check_protocol_contracts();
    analyzer
        .diagnostics
        .extend(rsscript_semantics::signature_diagnostics(
            &analyzer.syntax_program,
        ));
}

/// Generic bounds and resource-generic contracts belong to declaration checking
/// even though they run after type-name resolution for diagnostic stability.
pub(crate) fn check_generic_constraints(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::generic_constraint_diagnostics(
            &analyzer.syntax_program,
        ));
}
