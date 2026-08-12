use super::*;
use crate::checks::diagnostic_helpers::error_cause_manual_fix;

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
            read_view_mutation_diagnostic(analyzer, name, span.clone());
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

pub(super) fn read_view_mutation_diagnostic(analyzer: &mut Analyzer<'_>, name: &str, span: Span) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::READ_VIEW_MUTATION,
        format!("`{name}` is a read view from a `for` loop and cannot be used as an exclusive value."),
        span,
        "read view mutation",
        "RSScript `for` iterates `List<T>` by read view for non-Copy struct elements, so the loop variable does not own the element.",
        "copy_before_mutating",
        "Create a fresh local copy before mutation, or use an explicit partitioning API that grants exclusive element ownership.",
    ));
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

pub(super) fn noescape_consumes_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    access: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::NOESCAPE_CONSUMES_CAPTURE,
            format!(
                "noescape closure cannot consume captured local value `{}`.",
                access.path.base
            ),
            access.span.clone(),
            "captured local consumed here",
        )
        .with_cause(
            "`noescape Fn()` callbacks are non-consuming; the callee may call this closure more than once.",
        )
        .with_cause(
            "Read or mutate the captured local inside the noescape closure, or move/manage it before constructing the closure.",
        )
        .with_fix(
            "avoid_consuming_capture",
            "Do not use `take` or `manage` on captured local values inside noescape callbacks.",
            "manual",
        ),
    );
}

pub(super) fn explicit_closure_missing_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    actual: ParamEffect,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CLOSURE_CAPTURE_CONTRACT,
            format!("closure uses `{name}` without declaring it in captures"),
            span,
            "missing closure capture",
        )
        .with_cause(
            "Escaping function values must make every external input explicit in `captures(...)`.",
        )
        .with_cause(format!(
            "Add `captures({} {name})` or remove the external use.",
            actual.as_str()
        )),
    );
}

pub(super) fn explicit_closure_unused_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CLOSURE_CAPTURE_CONTRACT,
            format!("closure declares capture `{name}` but does not use it"),
            span,
            "unused closure capture",
        )
        .with_cause(
            "A closure capture list is review evidence and must describe the function value's real inputs.",
        )
        .with_cause("Remove the capture entry or use the value inside the closure body."),
    );
}

pub(super) fn explicit_closure_capture_contract_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    declared: ParamEffect,
    actual: ParamEffect,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CLOSURE_CAPTURE_CONTRACT,
            format!(
                "closure capture `{name}` is declared as `{}` but used as `{}`",
                declared.as_str(),
                actual.as_str()
            ),
            span,
            "closure capture effect mismatch",
        )
        .with_cause("Closure captures use the same read/mut/take ownership vocabulary as parameters.")
        .with_cause(format!(
            "Change the capture to `{} {name}` or change the closure body to match the declared access.",
            actual.as_str()
        )),
    );
}
