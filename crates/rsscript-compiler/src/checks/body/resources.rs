use super::*;

pub(super) fn check_managed_closure_captures(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    statement_span: &crate::diagnostic::Span,
    state: &BodyState,
) {
    let uses = local_analysis
        .managed_closure_ident_uses(statement_span)
        .unwrap_or(&[]);
    for (name, span) in uses {
        if state.is_local(name) {
            analyzer.diagnostics.push(
                rsscript_semantics::managed_closure_local_capture_diagnostic(name, span.clone()),
            );
        }
    }
}

pub(super) fn check_resource_escape(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    with_span: &crate::diagnostic::Span,
) {
    if let Some(escapes) = local_analysis.resource_escapes(with_span) {
        for escape in escapes {
            if !resource_is_active_at(local_analysis, &escape.binding, &escape.span) {
                continue;
            }
            match escape.kind {
                ResourceEscapeKind::Escape => {
                    analyzer
                        .diagnostics
                        .push(rsscript_semantics::resource_escape_diagnostic(
                            &escape.binding,
                            escape.span.clone(),
                        ));
                }
                ResourceEscapeKind::Capture => {
                    analyzer
                        .diagnostics
                        .push(rsscript_semantics::resource_capture_diagnostic(
                            &escape.binding,
                            escape.span.clone(),
                        ));
                }
            }
        }
    }
}

pub(super) fn check_resource_producer_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    allowed_resource_context: bool,
) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::resource_producer_diagnostics(
            &analyzer.hir,
            expr,
            allowed_resource_context,
        ));
}

pub(super) fn check_result_resource_with_has_try(analyzer: &mut Analyzer<'_>, resource: &HirExpr) {
    if let Some(diagnostic) =
        rsscript_semantics::result_resource_with_try_diagnostic(&analyzer.hir, resource)
    {
        analyzer.diagnostics.push(diagnostic);
    }
}

pub(super) fn resource_is_active_at(
    local_analysis: &LocalAnalysis<'_>,
    binding: &str,
    span: &crate::diagnostic::Span,
) -> bool {
    local_analysis
        .flow_entry_state(span)
        .is_none_or(|state| state.is_resource(binding))
}

pub(super) fn trusted_fresh_ident(analyzer: &Analyzer<'_>, name: &str) -> bool {
    analyzer.hir.type_kind(name) == Some(HirTypeKind::Struct)
        || analyzer
            .hir
            .resolve_function(None, name)
            .is_some_and(|signature| signature.returns_fresh)
}
