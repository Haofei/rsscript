use super::*;

pub(super) fn check_manage_operand_is_local(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) {
    let Some(name) = hir_ident_name(value) else {
        // A freshly-produced, owned rvalue (a struct constructor or a
        // `fresh`-returning call) is sound to `manage` inline: the value has
        // just been created here and is not an alias of any existing managed,
        // borrowed, or local binding. This is the same freshness model that
        // lets a `fresh` value materialize directly. Every other rvalue
        // (idents, field/index projections, an existing `manage`, etc.) may
        // alias live state and is still rejected below.
        if expr_is_fresh_shell(value) {
            return;
        }
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_manage_operand_diagnostic(
                "`manage` can only move a named local binding or a freshly produced value.",
                span.clone(),
            ));
        return;
    };
    if !state.is_local(name) {
        if state.is_read_view(name) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::read_view_mutation_diagnostic(
                    name,
                    span.clone(),
                ));
            return;
        }
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_manage_operand_diagnostic(
                format!("`{name}` is not a local binding and cannot be moved with `manage`."),
                span.clone(),
            ));
    }
}

pub(super) fn check_take_operand_is_local(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) {
    let Some(path) = place_path(value) else {
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_take_operand_diagnostic(
                "`take` can only consume a named local binding or a local field path.",
                span.clone(),
            ));
        return;
    };
    if !state.is_local(&path.base) {
        if state.is_resource(&path.base) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::resource_escape_diagnostic(
                    &path.base,
                    span.clone(),
                ));
            return;
        }
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_take_operand_diagnostic(
                format!(
                    "`{}` is not a local binding and cannot be consumed with `take`.",
                    path.base
                ),
                span.clone(),
            ));
    }
}

pub(super) fn tempdir_keep_consumes_resource_arg(
    callee: &Callee,
    arg: &HirCallArg,
    state: &BodyState,
) -> bool {
    let is_tempdir_keep = matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "TempDir" && name == "keep"
    ) || matches!(callee, Callee::Name(name) if name == "TempDir.keep");
    if !is_tempdir_keep || arg.name.as_deref().unwrap_or("dir") != "dir" {
        return false;
    }
    let HirExpr::Effect {
        effect: ParamEffect::Take,
        value,
        ..
    } = &arg.value
    else {
        return false;
    };
    matches!(value.as_ref(), HirExpr::Ident { name, .. } if state.is_resource(name))
}

pub(super) fn hir_ident_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

pub(super) fn apply_expr_effects(expr: &HirExpr, state: &mut BodyState) {
    match expr {
        HirExpr::Call { args, events, .. } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            for arg in args {
                apply_expr_effects(&arg.value, state);
            }
        }
        HirExpr::Effect { value, events, .. } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            apply_expr_effects(value, state);
        }
        HirExpr::Manage {
            value,
            events,
            span,
            ..
        } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            if let Some(path) = place_path(value) {
                state.mark_moved(&place_path_display(&path), span.clone());
            }
            apply_expr_effects(value, state);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            apply_expr_effects(value, state)
        }
        HirExpr::Try { value, .. } => apply_expr_effects(value, state),
        HirExpr::Match {
            value,
            scrutinee_effect,
            span,
            ..
        } => {
            apply_expr_effects(value, state);
            apply_match_scrutinee_effect(*scrutinee_effect, value, span, state);
        }
        HirExpr::Binary { left, right, .. } => {
            apply_expr_effects(left, state);
            apply_expr_effects(right, state);
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                apply_expr_effects(&entry.key, state);
                apply_expr_effects(&entry.value, state);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                apply_expr_effects(&field.value, state);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                apply_expr_effects(item, state);
            }
        }
        HirExpr::Field { base, .. } => apply_expr_effects(base, state),
        HirExpr::Index { base, index, .. } => {
            apply_expr_effects(base, state);
            apply_expr_effects(index, state);
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn apply_match_scrutinee_effect(
    effect: Option<DataEffect>,
    value: &HirExpr,
    span: &Span,
    state: &mut BodyState,
) {
    if effect != Some(DataEffect::Take) {
        return;
    }
    if let Some(path) = place_path(value) {
        state.mark_moved(&place_path_display(&path), span.clone());
    }
}

pub(super) fn arm_span(arms: &[HirMatchArm]) -> Span {
    arms.first().map_or_else(
        || Span {
            file: String::new(),
            line: 1,
            column: 1,
            length: 1,
        },
        |arm| arm.span.clone(),
    )
}

pub(super) fn check_moved_uses(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis<'_>) {
    for moved_use in local_analysis.moved_uses() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::moved_use_diagnostic(
                &moved_use.name,
                moved_use.use_span,
                &moved_use.move_span,
            ));
    }
}

pub(super) fn check_managed_to_local_uses(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for managed_to_local in local_analysis.managed_to_local_uses() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::managed_to_local_diagnostic(
                &managed_to_local.local_name,
                &managed_to_local.managed_name,
                managed_to_local.span,
            ));
    }
}

pub(super) fn check_retained_local_uses(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for retained in local_analysis.retained_local_uses() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::retained_local_diagnostic(
                &retained.name,
                &retained.callee,
                &retained.param,
                retained.span,
            ));
    }
}

pub(super) fn check_retained_closure_captures(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for capture in local_analysis.retained_closure_captures() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::retained_closure_capture_diagnostic(
                &capture.name,
                &capture.callee,
                &capture.param,
                capture.capture_span,
                &capture.closure_span,
            ));
    }
}

pub(super) fn check_take_handle_fields(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for field in local_analysis.take_handle_fields() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::take_handle_field_diagnostic(
                &field.name,
                field.span.clone(),
            ));
    }
}

pub(super) fn check_fresh_returns(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    function: &FunctionDecl,
) {
    if !function.returns_fresh {
        return;
    }
    check_fresh_return_type(analyzer, function);
    for issue in local_analysis.fresh_return_issues() {
        match &issue.kind {
            FreshReturnIssueKind::NotClean { name } => {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::fresh_return_not_clean_diagnostic(
                        &function.name,
                        name,
                        issue.span.clone(),
                    ));
            }
            FreshReturnIssueKind::UnknownIdent { name } if trusted_fresh_ident(analyzer, name) => {}
            FreshReturnIssueKind::UnknownIdent { .. } | FreshReturnIssueKind::Unknown => {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::freshness_unknown_diagnostic(
                        &function.name,
                        issue.span.clone(),
                    ));
            }
        }
    }
}

pub(super) fn check_fresh_return_type(analyzer: &mut Analyzer<'_>, function: &FunctionDecl) {
    let Some(return_ty) = &function.return_ty else {
        return;
    };
    let target = fresh_return_target_type(return_ty);
    match analyzer.hir.type_kind(&target.name) {
        Some(HirTypeKind::Struct) | Some(HirTypeKind::Sum) | None => {}
        Some(HirTypeKind::Class) | Some(HirTypeKind::Resource) => {
            analyzer
                .diagnostics
                .push(rsscript_semantics::invalid_fresh_return_type_diagnostic(
                    &function.name,
                    &target.name,
                    target.span.clone(),
                ));
        }
    }
}

pub(super) fn fresh_return_target_type(return_ty: &TypeRef) -> &TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}
