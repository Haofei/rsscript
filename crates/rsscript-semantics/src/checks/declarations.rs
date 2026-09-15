//! Declaration-level semantic checks.
//!
//! This is the ownership boundary for declaration-level rules.  The
//! individual implementations still
//! live with their analysis helpers while the larger checker refactor proceeds;
//! this module fixes the pipeline's stable semantic grouping first.

use crate::analyzer::Analyzer;
use rsscript_diagnostics::Diagnostic;

#[path = "declarations/duplicate_decls.rs"]
mod duplicate_decls;
#[path = "declarations/signatures.rs"]
mod signatures;

/// Run a signature-level pass over the source program *and* over every supplied
/// interface program.
///
/// A signature-level rule reads a declaration's contract — its return type,
/// parameter effects, generic bounds — and nothing else. Such a rule is exactly
/// as true of a bodyless `.rssi` declaration as of a `.rss` one: `pub fn
/// make_default<T>() -> fresh T` is `RS0603` wherever it is written, and an
/// interface that escapes the rule silently exports a contract the language
/// does not have. The diagnostics keep the interface file's own span, because
/// each interface program is parsed under its own path.
///
/// Only signature-level rules belong here. Body rules have nothing to run on in
/// an interface, and declaration-*inventory* rules (duplicates, protocol
/// contracts) are decided against the merged program, not per file.
fn over_source_and_interfaces(
    analyzer: &mut Analyzer<'_>,
    pass: fn(&crate::syntax::ast::Program) -> Vec<Diagnostic>,
) {
    let mut diagnostics = pass(&analyzer.syntax_program);
    for interface in &analyzer.interface_programs {
        diagnostics.extend(pass(interface));
    }
    analyzer.diagnostics.extend(diagnostics);
}

/// Checks declaration identity and contract shape in the established diagnostic
/// order. Keep this separate from type-name resolution, which depends on the
/// declaration inventory built here but is a distinct semantic phase.
pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    duplicate_decls::check(analyzer);
    analyzer.check_protocol_contracts();
    over_source_and_interfaces(analyzer, rsscript_semantics::signature_diagnostics);
}

/// Generic bounds and resource-generic contracts belong to declaration checking
/// even though they run after type-name resolution for diagnostic stability.
pub(crate) fn check_generic_constraints(analyzer: &mut Analyzer<'_>) {
    over_source_and_interfaces(analyzer, rsscript_semantics::generic_constraint_diagnostics);
}
