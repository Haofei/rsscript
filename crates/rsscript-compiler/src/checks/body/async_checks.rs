use super::*;

pub(super) fn check_await_operand(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    await_expr: &HirExpr,
) {
    if await_expr_targets_async_call(value) {
        return;
    }
    // Allow `await x` where x is an async let binding (task_group pending)
    if let Some(async_let_name) =
        await_targets_async_let_binding(value, &analyzer.async_let_names).map(str::to_string)
    {
        analyzer
            .async_let_names
            .retain(|name| name != &async_let_name);
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::AWAIT_NON_ASYNC,
            "`await` must consume an async call.",
            expr_span(await_expr).clone(),
            "await non-async expression",
        )
        .with_cause("RSScript does not expose Future or Task values in source; the executable async MVP only awaits direct async calls.")
        .with_fix("await_async_call", "Await an `async fn` call directly.", "manual"),
    );
}

pub(super) fn await_targets_async_let_binding<'a>(
    expr: &'a HirExpr,
    async_let_names: &'a [String],
) -> Option<&'a str> {
    match expr {
        HirExpr::Ident { name, .. } if async_let_names.contains(name) => Some(name.as_str()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            await_targets_async_let_binding(value, async_let_names)
        }
        _ => None,
    }
}

pub(super) fn await_expr_targets_async_call(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { resolution, .. } => {
            matches!(resolution, CallResolution::Resolved { signature, .. } if signature.is_async)
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            await_expr_targets_async_call(value)
        }
        _ => false,
    }
}

pub(super) fn check_await_live_values(
    analyzer: &mut Analyzer<'_>,
    state: &BodyState,
    await_expr: &HirExpr,
    live_after: &HashSet<String>,
) {
    let span = expr_span(await_expr).clone();
    for resource in &state.resources {
        await_live_value_diagnostic(analyzer, "resource", resource, &span);
    }
    for local in &state.locals {
        if !live_after.contains(local) {
            continue;
        }
        if state.value_type(local).is_some_and(is_copy_type_name) {
            continue;
        }
        await_live_value_diagnostic(analyzer, "local value", local, &span);
    }
}

pub(super) fn await_live_value_diagnostic(
    analyzer: &mut Analyzer<'_>,
    kind: &str,
    name: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::AWAIT_LIVE_LOCAL,
            format!("{kind} `{name}` cannot live across `await`."),
            span.clone(),
            "value live across await",
        )
        .with_cause("Suspending an RSScript async frame may keep managed handles and Copy snapshots, but local values, resources, and runtime guards must not be retained across suspension.")
        .with_fix("drop_before_await", format!("End the lifetime of `{name}` before this `await`."), "manual"),
    );
}

pub(super) fn expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Char { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

pub(super) fn check_async_call_consumed(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    resolution: &CallResolution,
    span: &Span,
    async_call_consumed: bool,
) {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return;
    };
    if !signature.is_async || async_call_consumed {
        return;
    }

    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ASYNC_CALL_NOT_CONSUMED,
            format!(
                "async call `{}` must be awaited.",
                body_callee_display(callee)
            ),
            span.clone(),
            "async call must be awaited",
        )
        .with_cause(
            "Async calls introduce suspension boundaries that must be visible in source; `spawn` is reserved but not executable in v0.7.",
        )
        .with_fix(
            "await_async_call",
            format!(
                "Write `await {}(...)`.",
                body_callee_display(callee)
            ),
            "manual",
        ),
    );
}

pub(super) fn is_weak_upgrade_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "Weak" && type_root_name(name) == "upgrade"
    )
}

pub(super) fn check_weak_field_requires_upgrade(analyzer: &mut Analyzer<'_>, value: &HirExpr) {
    let Some(access) = weak_field_access_requiring_upgrade(value) else {
        return;
    };

    analyzer.diagnostics.push(
        Diagnostic::error(
            code::WEAK_FIELD_REQUIRES_UPGRADE,
            format!(
                "weak field `{}` must be upgraded before it is used as a value.",
                access.name
            ),
            access.span.clone(),
            "weak field requires upgrade",
        )
        .with_cause("A weak field is a non-owning handle and may no longer point to a live value.")
        .with_fix(
            "upgrade_weak_field",
            format!(
                "Use `Weak.upgrade(value: read {})` and handle `None`.",
                access.name
            ),
            "manual",
        ),
    );
}
