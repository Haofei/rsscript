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
    check_fresh_return_types(analyzer);
}

/// `fresh` names a newly created value, which only a struct or a sum can be: a
/// class is a managed identity and a resource is a host-owned slot, so neither
/// may be the target of a `fresh` return (§5.8, §8.2).
///
/// This reads the declared return type and nothing else, so it is a
/// signature-level rule and holds wherever the signature is written. It used to
/// run only inside the body pass, over the source program's functions, and a
/// bodyless `.rssi` declaration of `pub fn open() -> fresh File` therefore
/// escaped it entirely — an interface could export a contract the language does
/// not have. It cannot use `over_source_and_interfaces` because it needs the
/// HIR's type kinds and not just the program, so it walks the same two inputs
/// itself. It runs with the generic bounds because both need type-name
/// resolution to have happened first.
fn check_fresh_return_types(analyzer: &mut Analyzer<'_>) {
    let mut invalid = Vec::new();
    for program in
        std::iter::once(&analyzer.syntax_program).chain(analyzer.interface_programs.iter())
    {
        for item in &program.items {
            let crate::syntax::ast::Item::Function(function) = item else {
                continue;
            };
            if !function.returns_fresh {
                continue;
            }
            let Some(return_ty) = &function.return_ty else {
                continue;
            };
            let target = fresh_return_target_type(return_ty);
            if matches!(
                analyzer.hir.type_kind(&target.name),
                Some(crate::hir::HirTypeKind::Class | crate::hir::HirTypeKind::Resource)
            ) {
                invalid.push((
                    function.name.clone(),
                    target.name.clone(),
                    target.span.clone(),
                ));
            }
        }
    }
    for (function, type_name, span) in invalid {
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_fresh_return_type_diagnostic(
                &function, &type_name, span,
            ));
    }
}

/// `fresh` applies to the payload of a `Result`/`Option` return, not to the
/// wrapper: `-> Result<fresh File, String>` targets `File`.
fn fresh_return_target_type(
    return_ty: &crate::syntax::ast::TypeRef,
) -> &crate::syntax::ast::TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}
