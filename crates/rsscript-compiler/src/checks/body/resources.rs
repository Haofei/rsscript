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
    if expr_is_resource_producer(analyzer, expr) {
        if allowed_resource_context {
            check_resource_producer_children(analyzer, expr);
        } else {
            analyzer
                .diagnostics
                .push(rsscript_semantics::resource_producer_escape_diagnostic(
                    hir_expr_type_name(expr).unwrap_or("resource"),
                    hir_expr_span(expr).clone(),
                ));
        }
        return;
    }

    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_resource_producer_expr(analyzer, &arg.value, false);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_producer_expr(analyzer, value, allowed_resource_context);
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_producer_expr(analyzer, left, false);
            check_resource_producer_expr(analyzer, right, false);
        }
        HirExpr::Field { base, .. } => check_resource_producer_expr(analyzer, base, false),
        HirExpr::Index { base, index, .. } => {
            check_resource_producer_expr(analyzer, base, false);
            check_resource_producer_expr(analyzer, index, false);
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_producer_expr(analyzer, value, allowed_resource_context);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_producer_expr(analyzer, &entry.key, false);
                check_resource_producer_expr(analyzer, &entry.value, false);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn check_result_resource_with_has_try(analyzer: &mut Analyzer<'_>, resource: &HirExpr) {
    if matches!(resource, HirExpr::Try { .. }) {
        return;
    }
    let Some(resource_type) = result_resource_ok_type(analyzer, resource) else {
        return;
    };

    analyzer.diagnostics.push(
        rsscript_semantics::resource_producer_missing_try_diagnostic(
            &resource_type,
            hir_expr_span(resource).clone(),
        ),
    );
}

pub(super) fn check_resource_producer_children(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_resource_producer_expr(analyzer, &arg.value, false);
            }
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            check_resource_producer_expr(analyzer, value, true);
        }
        _ => {}
    }
}

pub(super) fn check_resource_producer_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => check_resource_producer_expr(analyzer, value, false),
        HirStmt::With { resource, body, .. } => {
            check_resource_producer_expr(analyzer, resource, true);
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_producer_expr(analyzer, condition, false);
            for statement in &then_body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_producer_expr(analyzer, condition, false);
            }
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_producer_expr(analyzer, iterable, false);
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_producer_expr(analyzer, value, false);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_producer_expr(analyzer, &arm.operation, false);
                for statement in &arm.body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn expr_is_resource_producer(analyzer: &Analyzer<'_>, expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { .. } => {
            expr_type_is_resource(analyzer, expr)
                || result_resource_ok_type(analyzer, expr).is_some()
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            expr_type_is_resource(analyzer, expr) && expr_is_resource_producer(analyzer, value)
        }
        _ => false,
    }
}

pub(super) fn expr_type_is_resource(analyzer: &Analyzer<'_>, expr: &HirExpr) -> bool {
    hir_expr_type_name(expr).is_some_and(|type_name| {
        analyzer.hir.type_kind(type_root_name(type_name)) == Some(HirTypeKind::Resource)
    })
}

pub(super) fn result_resource_ok_type(analyzer: &Analyzer<'_>, expr: &HirExpr) -> Option<String> {
    let type_name = hir_expr_type_name(expr)?;
    let ok_type = result_ok_type_name(type_name)?;
    if analyzer.hir.type_kind(type_root_name(ok_type)) == Some(HirTypeKind::Resource) {
        Some(ok_type.to_string())
    } else {
        None
    }
}

pub(super) fn result_ok_type_name(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).into_iter().next()
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
