use super::*;
use crate::checks::diagnostic_helpers::{error_cause_fix, error_cause_manual_fix};

pub(super) fn check_resource_pool_bindings(
    analyzer: &mut Analyzer<'_>,
    body: &crate::hir::HirFunctionBody,
) {
    for binding in &body.bindings {
        if !binding
            .type_name
            .as_deref()
            .is_some_and(is_resource_pool_type)
        {
            continue;
        }
        let binding_is_local = binding.kind == HirBindingKind::LocalLet
            || (binding.kind == HirBindingKind::Param
                && matches!(binding.effect, Some(ParamEffect::Mut | ParamEffect::Take)));
        if !binding_is_local {
            resource_pool_not_local_diagnostic(analyzer, &binding.name, binding.span.clone());
        }
    }
}
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
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
                format!("managed closure captures local value `{name}`."),
                span.clone(),
                "local captured here",
                "Closures bound with `let` are managed closures.",
                "use_local_closure",
                "Bind the closure with `local` or use a noescape callback.",
            ));
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
                    resource_escape_diagnostic(analyzer, &escape.binding, escape.span.clone());
                }
                ResourceEscapeKind::Capture => {
                    resource_capture_diagnostic(analyzer, &escape.binding, escape.span.clone());
                }
            }
        }
    }
}

pub(super) fn check_resource_pool_lease_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    within_with_resource: bool,
) {
    match expr {
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if !within_with_resource && is_resource_pool_lease_producer(callee) {
                resource_pool_lease_escape_diagnostic(analyzer, span.clone());
            }
            for arg in args {
                check_resource_pool_lease_expr(analyzer, &arg.value, false);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_pool_lease_expr(analyzer, value, within_with_resource);
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_pool_lease_expr(analyzer, left, within_with_resource);
            check_resource_pool_lease_expr(analyzer, right, within_with_resource);
        }
        HirExpr::Field { base, .. } => {
            check_resource_pool_lease_expr(analyzer, base, within_with_resource);
        }
        HirExpr::Index { base, index, .. } => {
            check_resource_pool_lease_expr(analyzer, base, within_with_resource);
            check_resource_pool_lease_expr(analyzer, index, within_with_resource);
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_pool_lease_expr(analyzer, value, within_with_resource);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_pool_lease_expr(analyzer, &entry.key, within_with_resource);
                check_resource_pool_lease_expr(analyzer, &entry.value, within_with_resource);
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

pub(super) fn check_resource_producer_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    allowed_resource_context: bool,
) {
    if expr_is_resource_producer(analyzer, expr) {
        if allowed_resource_context {
            check_resource_producer_children(analyzer, expr);
        } else {
            resource_producer_escape_diagnostic(
                analyzer,
                hir_expr_span(expr).clone(),
                hir_expr_type_name(expr).unwrap_or("resource"),
            );
        }
        return;
    }

    match expr {
        HirExpr::Call { callee, args, .. } => {
            for arg in args {
                if is_resource_pool_constructor(callee) && arg.name.as_deref() == Some("create") {
                    check_resource_pool_factory_expr(analyzer, &arg.value);
                } else {
                    check_resource_producer_expr(analyzer, &arg.value, false);
                }
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

    analyzer.diagnostics.push(error_cause_fix(
        code::RESOURCE_PRODUCER_MISSING_TRY,
        format!(
            "`with` over `Result<{resource_type}, E>` must explicitly unwrap the resource producer with `?`."
        ),
        hir_expr_span(resource).clone(),
        "missing resource producer `?`",
        "Resource-producing `Result` values are transient; the successful resource must enter the `with` scope explicitly.",
        "add_try_to_resource_producer",
        "Write `with producer(...)? as resource { ... }`.",
        "machine-applicable",
    ));
}

pub(super) fn check_resource_producer_children(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    match expr {
        HirExpr::Call { callee, args, .. } => {
            for arg in args {
                if is_resource_pool_constructor(callee) && arg.name.as_deref() == Some("create") {
                    check_resource_pool_factory_expr(analyzer, &arg.value);
                } else {
                    check_resource_producer_expr(analyzer, &arg.value, false);
                }
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

pub(super) fn check_resource_pool_factory_expr(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    match expr {
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        _ => check_resource_producer_expr(analyzer, expr, true),
    }
}

pub(super) fn check_resource_pool_new_factory_contract(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
) {
    if !is_resource_pool_infallible_constructor(callee) {
        return;
    }
    let Some(create) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("create"))
        .or_else(|| args.first())
    else {
        return;
    };
    let Some(fallible) = resource_pool_fallible_factory_expr(&create.value) else {
        return;
    };
    let type_name = hir_expr_type_name(fallible).unwrap_or("Result<T, E>");
    resource_pool_fallible_factory_diagnostic(analyzer, hir_expr_span(fallible).clone(), type_name);
}

pub(super) fn check_resource_pool_try_new_factory_contract(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
) {
    if !is_resource_pool_fallible_constructor(callee) {
        return;
    }
    let Some(create) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("create"))
        .or_else(|| args.first())
    else {
        return;
    };
    if resource_pool_fallible_factory_expr(&create.value).is_none() {
        resource_pool_infallible_try_factory_diagnostic(
            analyzer,
            hir_expr_span(&create.value).clone(),
            hir_expr_type_name(&create.value).unwrap_or("T"),
        );
    }
}

pub(super) fn check_resource_pool_constructor_max_size_contract(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
) {
    if !is_resource_pool_constructor(callee) {
        return;
    }
    // Lazy pools create on demand, so `max_size` is a runtime soft cap and may be
    // any `Int` (e.g. a config-derived value).
    if is_resource_pool_lazy(callee) || is_resource_pool_try_lazy(callee) {
        return;
    }
    let Some(max_size) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("max_size"))
        .or_else(|| args.get(1))
    else {
        return;
    };
    // Eager pools allocate up front, so `max_size` must be a statically known
    // positive value: a positive `Int` literal, or a reference to a positive
    // `Int` `const` (a reviewable, controlled constant).
    if resource_pool_static_positive_int(analyzer, &max_size.value).is_none() {
        resource_pool_invalid_max_size_diagnostic(analyzer, hir_expr_span(&max_size.value).clone());
    }
}

/// Resolve `expr` to a statically known positive `i64`: a positive `Int` literal,
/// or an `Ident` naming a `const` whose value is a positive `Int` literal.
pub(super) fn resource_pool_static_positive_int(
    analyzer: &Analyzer<'_>,
    expr: &HirExpr,
) -> Option<i64> {
    match expr {
        HirExpr::Number { value, .. } => value.parse::<i64>().ok().filter(|value| *value > 0),
        HirExpr::Ident { name, .. } => analyzer.syntax_program.items.iter().find_map(|item| {
            let Item::Const(decl) = item else {
                return None;
            };
            if &decl.name != name {
                return None;
            }
            let crate::syntax::ast::Expr::Number(literal, _) = &decl.value else {
                return None;
            };
            literal.parse::<i64>().ok().filter(|value| *value > 0)
        }),
        _ => None,
    }
}

pub(super) fn check_resource_pool_factory_resource_captures(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    state: &BodyState,
) {
    if !is_resource_pool_constructor(callee) {
        return;
    }
    let Some(create) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("create"))
        .or_else(|| args.first())
    else {
        return;
    };
    let mut captures = Vec::new();
    collect_resource_pool_factory_resource_captures(&create.value, state, &mut captures);
    for (name, span) in captures {
        resource_pool_factory_resource_capture_diagnostic(analyzer, &name, span);
    }
}

pub(super) fn collect_resource_pool_factory_resource_captures(
    expr: &HirExpr,
    state: &BodyState,
    captures: &mut Vec<(String, Span)>,
) {
    if let HirExpr::Closure { params, body, .. } = expr {
        let mut bound = params.iter().cloned().collect::<HashSet<_>>();
        collect_resource_pool_factory_resource_captures_block(body, state, &mut bound, captures);
    }
}

pub(super) fn collect_resource_pool_factory_resource_captures_block(
    block: &HirBlock,
    state: &BodyState,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    for statement in &block.statements {
        collect_resource_pool_factory_resource_captures_stmt(statement, state, bound, captures);
    }
}

pub(super) fn collect_resource_pool_factory_resource_captures_stmt(
    statement: &HirStmt,
    state: &BodyState,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match statement {
        HirStmt::Let {
            name,
            value: Some(value),
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
            bound.insert(name.clone());
        }
        HirStmt::Let {
            name, value: None, ..
        } => {
            bound.insert(name.clone());
        }
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(resource, state, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut scoped,
                captures,
            );
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(condition, state, bound, captures);
            collect_resource_pool_factory_resource_captures_block(
                then_body,
                state,
                &mut bound.clone(),
                captures,
            );
            if let Some(else_body) = else_body {
                collect_resource_pool_factory_resource_captures_block(
                    else_body,
                    state,
                    &mut bound.clone(),
                    captures,
                );
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_resource_pool_factory_resource_captures_expr(
                    condition, state, bound, captures,
                );
            }
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut bound.clone(),
                captures,
            );
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(iterable, state, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut scoped,
                captures,
            );
        }
        HirStmt::Match { value, arms, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
            for arm in arms {
                let mut scoped = bound.clone();
                for binding in arm.pattern.binding_names() {
                    scoped.insert(binding.to_string());
                }
                collect_resource_pool_factory_resource_captures_block(
                    &arm.body,
                    state,
                    &mut scoped,
                    captures,
                );
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_resource_pool_factory_resource_captures_expr(
                    &arm.operation,
                    state,
                    bound,
                    captures,
                );
                let mut scoped = bound.clone();
                if arm.binding != "_" {
                    scoped.insert(arm.binding.clone());
                }
                collect_resource_pool_factory_resource_captures_block(
                    &arm.body,
                    state,
                    &mut scoped,
                    captures,
                );
            }
        }
        HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_resource_pool_factory_resource_captures_expr(
    expr: &HirExpr,
    state: &BodyState,
    bound: &HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if !bound.contains(name) && state.is_resource(name) {
                push_resource_pool_factory_capture(captures, name.clone(), span.clone());
            }
        }
        HirExpr::Field { base, .. } => {
            collect_resource_pool_factory_resource_captures_expr(base, state, bound, captures);
        }
        HirExpr::Index { base, index, .. } => {
            collect_resource_pool_factory_resource_captures_expr(base, state, bound, captures);
            collect_resource_pool_factory_resource_captures_expr(index, state, bound, captures);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_resource_pool_factory_resource_captures_expr(
                    &arg.value, state, bound, captures,
                );
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_resource_pool_factory_resource_captures_expr(left, state, bound, captures);
            collect_resource_pool_factory_resource_captures_expr(right, state, bound, captures);
        }
        HirExpr::Closure { params, body, .. } => {
            let mut scoped = bound.clone();
            scoped.extend(params.iter().cloned());
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut scoped,
                captures,
            );
        }
        HirExpr::Match { value, arms, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
            for arm in arms {
                collect_resource_pool_factory_resource_captures_block(
                    &arm.body,
                    state,
                    &mut bound.clone(),
                    captures,
                );
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_resource_pool_factory_resource_captures_expr(
                    &entry.key, state, bound, captures,
                );
                collect_resource_pool_factory_resource_captures_expr(
                    &entry.value,
                    state,
                    bound,
                    captures,
                );
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn push_resource_pool_factory_capture(
    captures: &mut Vec<(String, Span)>,
    name: String,
    span: Span,
) {
    if !captures
        .iter()
        .any(|(existing_name, existing_span)| existing_name == &name && existing_span == &span)
    {
        captures.push((name, span));
    }
}

/// Reject lazy pool factories (`owned Fn`) that capture anything other than an
/// owned `local`: the factory is stored and called on demand, so it must own what
/// it captures. A parameter is borrowed; a managed binding is shared/refcounted —
/// neither is a clean owned value to move in. Names that are not bindings (globals,
/// consts, functions) are `'static` and allowed.
pub(super) fn check_lazy_factory_captures_block(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    bindings: &std::collections::HashMap<String, HirBindingKind>,
) {
    for statement in &block.statements {
        check_lazy_factory_captures_stmt(analyzer, statement, bindings);
    }
}

pub(super) fn check_lazy_factory_captures_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    bindings: &std::collections::HashMap<String, HirBindingKind>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_lazy_factory_captures_expr(analyzer, value, bindings);
            }
        }
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            check_lazy_factory_captures_expr(analyzer, value, bindings)
        }
        HirStmt::With { resource, body, .. } => {
            check_lazy_factory_captures_expr(analyzer, resource, bindings);
            check_lazy_factory_captures_block(analyzer, body, bindings);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_lazy_factory_captures_expr(analyzer, condition, bindings);
            check_lazy_factory_captures_block(analyzer, then_body, bindings);
            if let Some(else_body) = else_body {
                check_lazy_factory_captures_block(analyzer, else_body, bindings);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_lazy_factory_captures_expr(analyzer, condition, bindings);
            }
            check_lazy_factory_captures_block(analyzer, body, bindings);
        }
        HirStmt::For { iterable, body, .. } => {
            check_lazy_factory_captures_expr(analyzer, iterable, bindings);
            check_lazy_factory_captures_block(analyzer, body, bindings);
        }
        HirStmt::Match { value, arms, .. } => {
            check_lazy_factory_captures_expr(analyzer, value, bindings);
            for arm in arms {
                check_lazy_factory_captures_block(analyzer, &arm.body, bindings);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_lazy_factory_captures_expr(analyzer, &arm.operation, bindings);
                check_lazy_factory_captures_block(analyzer, &arm.body, bindings);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_lazy_factory_captures_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    bindings: &std::collections::HashMap<String, HirBindingKind>,
) {
    if let HirExpr::Call { callee, args, .. } = expr
        && (is_resource_pool_lazy(callee) || is_resource_pool_try_lazy(callee))
    {
        if let Some(create) = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("create"))
            .or_else(|| args.first())
            && let HirExpr::Closure { params, body, .. } = &create.value
        {
            let mut captures = Vec::new();
            collect_lazy_factory_capture_idents(params, body, &mut captures);
            for (name, span) in captures {
                match bindings.get(&name) {
                    // Owned local: the only kind that can be cleanly moved in.
                    Some(HirBindingKind::LocalLet) => {}
                    // Not a binding at all (global/const/function): `'static`, fine.
                    None => {}
                    Some(kind) => {
                        resource_pool_lazy_factory_capture_diagnostic(analyzer, &name, *kind, span);
                    }
                }
            }
        }
    }
    // Recurse so nested lazy constructors are also checked.
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_lazy_factory_captures_expr(analyzer, &arg.value, bindings);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => check_lazy_factory_captures_expr(analyzer, value, bindings),
        HirExpr::Binary { left, right, .. } => {
            check_lazy_factory_captures_expr(analyzer, left, bindings);
            check_lazy_factory_captures_expr(analyzer, right, bindings);
        }
        HirExpr::Field { base, .. } => check_lazy_factory_captures_expr(analyzer, base, bindings),
        HirExpr::Index { base, index, .. } => {
            check_lazy_factory_captures_expr(analyzer, base, bindings);
            check_lazy_factory_captures_expr(analyzer, index, bindings);
        }
        HirExpr::Closure { body, .. } => {
            check_lazy_factory_captures_block(analyzer, body, bindings)
        }
        HirExpr::Match { value, arms, .. } => {
            check_lazy_factory_captures_expr(analyzer, value, bindings);
            for arm in arms {
                check_lazy_factory_captures_block(analyzer, &arm.body, bindings);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_lazy_factory_captures_expr(analyzer, &entry.key, bindings);
                check_lazy_factory_captures_expr(analyzer, &entry.value, bindings);
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

pub(super) fn collect_lazy_factory_capture_idents(
    params: &[String],
    block: &HirBlock,
    captures: &mut Vec<(String, Span)>,
) {
    let mut bound = params.iter().cloned().collect();
    collect_lazy_factory_capture_idents_block(block, &mut bound, captures);
}

pub(super) fn collect_lazy_factory_capture_idents_block(
    block: &HirBlock,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    for statement in &block.statements {
        collect_lazy_factory_capture_idents_stmt(statement, bound, captures);
    }
}

pub(super) fn collect_lazy_factory_capture_idents_stmt(
    statement: &HirStmt,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match statement {
        HirStmt::Let {
            name,
            value: Some(value),
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
            bound.insert(name.clone());
        }
        HirStmt::Let {
            name, value: None, ..
        } => {
            bound.insert(name.clone());
        }
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures)
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(resource, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_lazy_factory_capture_idents_block(body, &mut scoped, captures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(condition, bound, captures);
            collect_lazy_factory_capture_idents_block(then_body, &mut bound.clone(), captures);
            if let Some(else_body) = else_body {
                collect_lazy_factory_capture_idents_block(else_body, &mut bound.clone(), captures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_lazy_factory_capture_idents_expr(condition, bound, captures);
            }
            collect_lazy_factory_capture_idents_block(body, &mut bound.clone(), captures);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(iterable, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_lazy_factory_capture_idents_block(body, &mut scoped, captures);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
            for arm in arms {
                let mut scoped = bound.clone();
                for binding in arm.pattern.binding_names() {
                    scoped.insert(binding.to_string());
                }
                collect_lazy_factory_capture_idents_block(&arm.body, &mut scoped, captures);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_lazy_factory_capture_idents_expr(&arm.operation, bound, captures);
                let mut scoped = bound.clone();
                if arm.binding != "_" {
                    scoped.insert(arm.binding.clone());
                }
                collect_lazy_factory_capture_idents_block(&arm.body, &mut scoped, captures);
            }
        }
        HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_lazy_factory_capture_idents_expr(
    expr: &HirExpr,
    bound: &HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if !bound.contains(name) {
                push_resource_pool_factory_capture(captures, name.clone(), span.clone());
            }
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_lazy_factory_capture_idents_expr(&arg.value, bound, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_lazy_factory_capture_idents_expr(left, bound, captures);
            collect_lazy_factory_capture_idents_expr(right, bound, captures);
        }
        HirExpr::Field { base, .. } => {
            collect_lazy_factory_capture_idents_expr(base, bound, captures)
        }
        HirExpr::Index { base, index, .. } => {
            collect_lazy_factory_capture_idents_expr(base, bound, captures);
            collect_lazy_factory_capture_idents_expr(index, bound, captures);
        }
        HirExpr::Closure { params, body, .. } => {
            let mut scoped = bound.clone();
            scoped.extend(params.iter().cloned());
            collect_lazy_factory_capture_idents_block(body, &mut scoped, captures);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
            for arm in arms {
                let mut scoped = bound.clone();
                for binding in arm.pattern.binding_names() {
                    scoped.insert(binding.to_string());
                }
                collect_lazy_factory_capture_idents_block(&arm.body, &mut scoped, captures);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_lazy_factory_capture_idents_expr(&entry.key, bound, captures);
                collect_lazy_factory_capture_idents_expr(&entry.value, bound, captures);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn is_resource_pool_discard(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "discard")
}

/// `discard`'s argument must be a bare local binding that is an active lease — the
/// root identifier of `mut <name>` (peeling the effect wrapper).
pub(super) fn discard_lease_root(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Effect { value, .. } => discard_lease_root(value),
        HirExpr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

/// Validate that every `ResourcePool.discard` targets a binding introduced by an
/// enclosing `with ResourcePool.borrow/try_borrow(...) as name`. `lease_bindings`
/// is the stack of such names currently in scope.
pub(super) fn check_resource_pool_discards(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    lease_bindings: &mut Vec<String>,
) {
    for statement in &block.statements {
        check_resource_pool_discards_stmt(analyzer, statement, lease_bindings);
    }
}

pub(super) fn check_resource_pool_discards_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    lease_bindings: &mut Vec<String>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_resource_pool_discards_expr(analyzer, value, lease_bindings);
            }
        }
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings)
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            check_resource_pool_discards_expr(analyzer, resource, lease_bindings);
            // A `with` over a pool lease producer makes `binding` a lease.
            if resource_pool_borrow_pool_path(resource).is_some() {
                lease_bindings.push(binding.clone());
                check_resource_pool_discards(analyzer, body, lease_bindings);
                lease_bindings.pop();
            } else {
                check_resource_pool_discards(analyzer, body, lease_bindings);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_pool_discards_expr(analyzer, condition, lease_bindings);
            check_resource_pool_discards(analyzer, then_body, lease_bindings);
            if let Some(else_body) = else_body {
                check_resource_pool_discards(analyzer, else_body, lease_bindings);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_pool_discards_expr(analyzer, condition, lease_bindings);
            }
            check_resource_pool_discards(analyzer, body, lease_bindings);
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_pool_discards_expr(analyzer, iterable, lease_bindings);
            check_resource_pool_discards(analyzer, body, lease_bindings);
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings);
            for arm in arms {
                check_resource_pool_discards(analyzer, &arm.body, lease_bindings);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_pool_discards_expr(analyzer, &arm.operation, lease_bindings);
                check_resource_pool_discards(analyzer, &arm.body, lease_bindings);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_resource_pool_discards_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    lease_bindings: &mut Vec<String>,
) {
    match expr {
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if is_resource_pool_discard(callee) {
                let targets_lease = args
                    .iter()
                    .find(|arg| arg.name.as_deref() == Some("lease"))
                    .or_else(|| args.first())
                    .and_then(|arg| discard_lease_root(&arg.value))
                    .is_some_and(|root| lease_bindings.iter().any(|name| name == root));
                if !targets_lease {
                    resource_pool_discard_not_lease_diagnostic(analyzer, span.clone());
                }
            }
            for arg in args {
                check_resource_pool_discards_expr(analyzer, &arg.value, lease_bindings);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings);
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_pool_discards_expr(analyzer, left, lease_bindings);
            check_resource_pool_discards_expr(analyzer, right, lease_bindings);
        }
        HirExpr::Field { base, .. } => {
            check_resource_pool_discards_expr(analyzer, base, lease_bindings)
        }
        HirExpr::Index { base, index, .. } => {
            check_resource_pool_discards_expr(analyzer, base, lease_bindings);
            check_resource_pool_discards_expr(analyzer, index, lease_bindings);
        }
        HirExpr::Closure { body, .. } => {
            // A closure escapes the lease scope, so it carries no lease bindings.
            check_resource_pool_discards(analyzer, body, &mut Vec::new());
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    check_resource_pool_discards_expr(analyzer, guard, lease_bindings);
                }
                check_resource_pool_discards(analyzer, &arm.body, lease_bindings);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_pool_discards_expr(analyzer, &entry.key, lease_bindings);
                check_resource_pool_discards_expr(analyzer, &entry.value, lease_bindings);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                check_resource_pool_discards_expr(analyzer, &field.value, lease_bindings);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                check_resource_pool_discards_expr(analyzer, item, lease_bindings);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn resource_pool_borrow_pool_path(expr: &HirExpr) -> Option<PlacePath> {
    match expr {
        HirExpr::Call { callee, args, .. } if is_resource_pool_lease_producer(callee) => args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("pool"))
            .or_else(|| args.first())
            .and_then(|arg| effect_wrapped_place_path(&arg.value)),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            resource_pool_borrow_pool_path(value)
        }
        _ => None,
    }
}

pub(super) fn effect_wrapped_place_path(expr: &HirExpr) -> Option<PlacePath> {
    match expr {
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            effect_wrapped_place_path(value)
        }
        _ => place_path(expr),
    }
}

pub(super) fn resource_pool_fallible_factory_expr(expr: &HirExpr) -> Option<&HirExpr> {
    if hir_expr_type_name(expr).is_some_and(is_result_type) {
        return Some(expr);
    }
    match expr {
        HirExpr::Closure { body, .. } => resource_pool_fallible_factory_block(body),
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => resource_pool_fallible_factory_expr(value),
        HirExpr::Try { .. }
        | HirExpr::Match { .. }
        | HirExpr::Call { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Binary { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn resource_pool_fallible_factory_block(block: &HirBlock) -> Option<&HirExpr> {
    for statement in &block.statements {
        if let Some(expr) = resource_pool_fallible_factory_stmt(statement) {
            return Some(expr);
        }
    }
    None
}

pub(super) fn resource_pool_fallible_factory_stmt(statement: &HirStmt) -> Option<&HirExpr> {
    match statement {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => resource_pool_fallible_factory_expr(value),
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => resource_pool_fallible_factory_block(then_body).or_else(|| {
            else_body
                .as_ref()
                .and_then(resource_pool_fallible_factory_block)
        }),
        HirStmt::Loop { body, .. } | HirStmt::With { body, .. } => {
            resource_pool_fallible_factory_block(body)
        }
        HirStmt::For { body, .. } => resource_pool_fallible_factory_block(body),
        HirStmt::Match { arms, .. } => arms
            .iter()
            .find_map(|arm| resource_pool_fallible_factory_block(&arm.body)),
        HirStmt::Select { arms, .. } => arms
            .iter()
            .find_map(|arm| resource_pool_fallible_factory_block(&arm.body)),
        HirStmt::Let { .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

pub(super) fn check_resource_pool_factory_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => check_resource_producer_expr(analyzer, value, true),
        HirStmt::Let {
            value: Some(value), ..
        } => check_resource_producer_expr(analyzer, value, false),
        HirStmt::With { resource, body, .. } => {
            check_resource_producer_expr(analyzer, resource, true);
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
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
                check_resource_pool_factory_stmt(analyzer, statement);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    check_resource_pool_factory_stmt(analyzer, statement);
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
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_producer_expr(analyzer, iterable, false);
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_producer_expr(analyzer, value, false);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_pool_factory_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_producer_expr(analyzer, &arm.operation, false);
                for statement in &arm.body.statements {
                    check_resource_pool_factory_stmt(analyzer, statement);
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

pub(super) fn result_error_type_name(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).get(1).copied()
}

pub(super) fn list_element_type(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("List<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).into_iter().next()
}

pub(super) fn stream_item_type(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Stream<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).into_iter().next()
}

pub(super) fn check_resource_pool_lease_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => check_resource_pool_lease_expr(analyzer, value, false),
        HirStmt::With { resource, body, .. } => {
            check_resource_pool_lease_expr(analyzer, resource, true);
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_pool_lease_expr(analyzer, condition, false);
            for statement in &then_body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_pool_lease_expr(analyzer, condition, false);
            }
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_pool_lease_expr(analyzer, iterable, false);
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_pool_lease_expr(analyzer, value, false);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_pool_lease_expr(analyzer, &arm.operation, false);
                for statement in &arm.body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
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

pub(super) fn check_resource_pool_active_lease_block(
    analyzer: &mut Analyzer<'_>,
    active_pool: &PlacePath,
    block: &HirBlock,
) {
    for statement in &block.statements {
        check_resource_pool_active_lease_stmt(analyzer, active_pool, statement);
    }
}

pub(super) fn check_resource_pool_active_lease_stmt(
    analyzer: &mut Analyzer<'_>,
    active_pool: &PlacePath,
    statement: &HirStmt,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value)
        }
        HirStmt::With { resource, body, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, resource);
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, condition);
            check_resource_pool_active_lease_block(analyzer, active_pool, then_body);
            if let Some(else_body) = else_body {
                check_resource_pool_active_lease_block(analyzer, active_pool, else_body);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_pool_active_lease_expr(analyzer, active_pool, condition);
            }
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, iterable);
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
            for arm in arms {
                check_resource_pool_active_lease_block(analyzer, active_pool, &arm.body);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_pool_active_lease_expr(analyzer, active_pool, &arm.operation);
                check_resource_pool_active_lease_block(analyzer, active_pool, &arm.body);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_resource_pool_active_lease_expr(
    analyzer: &mut Analyzer<'_>,
    active_pool: &PlacePath,
    expr: &HirExpr,
) {
    match expr {
        HirExpr::Effect {
            effect,
            value,
            span,
            ..
        } => {
            if let Some(path) = place_path(value)
                && place_paths_overlap(active_pool, &path)
            {
                resource_pool_active_lease_conflict_diagnostic(analyzer, *effect, span.clone());
            }
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
        }
        HirExpr::Manage { value, span, .. } => {
            if let Some(path) = place_path(value)
                && place_paths_overlap(active_pool, &path)
            {
                resource_pool_active_lease_conflict_diagnostic(
                    analyzer,
                    ParamEffect::Take,
                    span.clone(),
                );
            }
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_resource_pool_active_lease_expr(analyzer, active_pool, &arg.value);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, left);
            check_resource_pool_active_lease_expr(analyzer, active_pool, right);
        }
        HirExpr::Field { base, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, base);
        }
        HirExpr::Index { base, index, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, base);
            check_resource_pool_active_lease_expr(analyzer, active_pool, index);
        }
        HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
        }
        HirExpr::Closure { body, .. } => {
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
            for arm in arms {
                check_resource_pool_active_lease_block(analyzer, active_pool, &arm.body);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_pool_active_lease_expr(analyzer, active_pool, &entry.key);
                check_resource_pool_active_lease_expr(analyzer, active_pool, &entry.value);
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

pub(super) fn place_paths_overlap(left: &PlacePath, right: &PlacePath) -> bool {
    if left.base != right.base {
        return false;
    }
    left.components
        .iter()
        .zip(&right.components)
        .all(|(left, right)| left == right)
}

pub(super) fn is_resource_pool_borrow(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "borrow")
}

pub(super) fn is_resource_pool_try_borrow(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_borrow")
}

/// Both `borrow` and `try_borrow` produce a scoped lease, so both are subject to
/// the same "must be consumed by `with`" and active-lease rules.
pub(super) fn is_resource_pool_lease_producer(callee: &Callee) -> bool {
    is_resource_pool_borrow(callee) || is_resource_pool_try_borrow(callee)
}

pub(super) fn is_resource_pool_new(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "new")
}

pub(super) fn is_resource_pool_try_new(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_new")
}

pub(super) fn is_resource_pool_lazy(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "lazy")
}

pub(super) fn is_resource_pool_try_lazy(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_lazy")
}

/// Infallible factory contract (`new`/`lazy`): the `create` closure returns a
/// bare resource. `try_new`/`try_lazy` are the fallible counterparts.
pub(super) fn is_resource_pool_infallible_constructor(callee: &Callee) -> bool {
    is_resource_pool_new(callee) || is_resource_pool_lazy(callee)
}

pub(super) fn is_resource_pool_fallible_constructor(callee: &Callee) -> bool {
    is_resource_pool_try_new(callee) || is_resource_pool_try_lazy(callee)
}

pub(super) fn is_resource_pool_constructor(callee: &Callee) -> bool {
    is_resource_pool_infallible_constructor(callee) || is_resource_pool_fallible_constructor(callee)
}

pub(super) fn is_resource_pool_type(type_name: &str) -> bool {
    type_root_name(type_name) == "ResourcePool"
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

pub(super) fn resource_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource `{binding}` cannot escape its `with` block."),
            span,
            "resource escapes",
        )
        .with_cause("A `with` resource must be dropped when the block exits."),
    );
}

pub(super) fn resource_producer_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    type_name: &str,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::RESOURCE_ESCAPE,
        format!("resource-producing expression of type `{type_name}` must be consumed by `with`."),
        span,
        "resource producer escapes",
        "Resource-producing calls create transient linear values that cannot be stored, returned, retained, managed, or passed as ordinary values.",
        "use_with",
        "Use `with producer(...)? as resource { ... }`, or an approved resource container API.",
    ));
}

pub(super) fn resource_pool_lease_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::RESOURCE_ESCAPE,
        "resource lease from `ResourcePool.borrow` must be scoped by `with`.",
        span,
        "resource lease escapes",
        "Pool leases are resources and must be returned to the pool when the `with` block exits.",
        "wrap_with",
        "Use `with ResourcePool.borrow(pool: mut pool) as lease { ... }`.",
    ));
}

pub(super) fn resource_pool_not_local_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_fix(
        code::RESOURCE_POOL_NOT_LOCAL,
        format!("ResourcePool binding `{binding}` must be local."),
        span,
        "ResourcePool must be local",
        "ResourcePool owns long-lived resources and must not be hidden behind an ordinary managed binding.",
        "make_resource_pool_local",
        format!("Declare `{binding}` with `local`, or pass it as a `mut` local-capability parameter."),
        "machine-applicable",
    ));
}

pub(super) fn resource_pool_fallible_factory_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    type_name: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_FALLIBLE_FACTORY,
            "`ResourcePool.new` requires an infallible factory.",
            span,
            "fallible ResourcePool factory",
        )
        .with_cause(format!(
            "The factory expression has type `{type_name}`, but `ResourcePool.new` expects a resource value."
        ))
        .with_cause("v0.6 `ResourcePool.new` eagerly constructs the pool and does not hide `Result` handling inside the pool constructor.")
        .with_fix(
            "use_matching_pool_constructor",
            "Handle failure before constructing the pool, or use `ResourcePool.try_new` with a fallible factory.",
            "manual",
        ),
    );
}

pub(super) fn resource_pool_infallible_try_factory_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    type_name: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_FALLIBLE_FACTORY,
            "`ResourcePool.try_new` requires a fallible factory.",
            span,
            "infallible ResourcePool try factory",
        )
        .with_cause(format!(
            "The factory expression has type `{type_name}`, but `ResourcePool.try_new` expects `Result<T, E>`."
        ))
        .with_cause("Use `ResourcePool.new` for infallible eager construction, or return a `Result` from the factory closure.")
        .with_fix(
            "use_matching_pool_constructor",
            "Use `ResourcePool.new`, or call a fallible resource constructor from the factory.",
            "manual",
        ),
    );
}

pub(super) fn resource_pool_lazy_factory_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    kind: HirBindingKind,
    span: crate::diagnostic::Span,
) {
    let (noun, why) = match kind {
        HirBindingKind::Param => (
            "parameter",
            "a parameter is borrowed and would not outlive the pool",
        ),
        HirBindingKind::ManagedLet => (
            "managed binding",
            "a managed binding is shared/refcounted, not a clean owned value",
        ),
        HirBindingKind::LocalLet => ("binding", "it is not a clean owned local"),
    };
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::RESOURCE_POOL_LAZY_FACTORY_CAPTURE,
        format!("lazy pool factory captures {noun} `{name}`."),
        span,
        "non-owned binding captured by stored factory",
        format!(
            "A lazy (`owned Fn`) factory is stored in the pool and called on demand, so it must own its captures, but {why}."
        ),
        "capture_owned_local",
        format!("Bind an owned `local` from `{name}` before constructing the pool and capture that local instead."),
    ));
}

pub(super) fn resource_pool_discard_not_lease_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::RESOURCE_POOL_DISCARD_NOT_LEASE,
        "`ResourcePool.discard` requires a pool lease binding.",
        span,
        "not a pool lease",
        "`discard` evicts a checked-out lease, so its argument must be the binding of an enclosing `with ResourcePool.borrow(...) as name` or `with ResourcePool.try_borrow(...)? as name`.",
        "discard_lease_binding",
        "Call `discard` on the `with`-lease binding, e.g. `ResourcePool.discard(lease: mut conn)` inside `with ... as conn`.",
    ));
}

pub(super) fn resource_pool_active_lease_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    effect: ParamEffect,
    span: crate::diagnostic::Span,
) {
    let effect = match effect {
        ParamEffect::Read => "read",
        ParamEffect::Mut => "mut",
        ParamEffect::Take => "take/manage",
    };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_ACTIVE_LEASE_CONFLICT,
            "ResourcePool cannot be used while one of its leases is active.",
            span,
            "ResourcePool active lease conflict",
        )
        .with_cause(format!(
            "This `{effect}` use targets the same pool root as an active `ResourcePool.borrow` lease."
        ))
        .with_cause("As a conservative policy the language permits one active lease per pool: borrow, stats, reset, take, and manage of the same pool root are rejected until the `with` scope exits. (The runtime checkout model can track several leases at once; surfacing concurrent leases in the language is a deliberate future step.)")
        .with_fix(
            "end_pool_lease_scope",
            "Move this pool use after the `with ResourcePool.borrow(...)` block, or use a different pool.",
            "manual",
        ),
    );
}

pub(super) fn resource_pool_invalid_max_size_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_INVALID_MAX_SIZE,
            "`ResourcePool.new` requires a positive literal `max_size`.",
            span,
            "invalid ResourcePool max_size",
        )
        .with_cause("v0.6 `ResourcePool.new` eagerly constructs exactly `max_size` resources and returns no `Result`.")
        .with_cause("Use a positive `Int` literal so empty or dynamically sized pools cannot hide construction failure or exhaustion behavior.")
        .with_fix(
            "use_positive_literal_max_size",
            "Pass a positive integer literal such as `max_size: 1`.",
            "manual",
        ),
    );
}

pub(super) fn resource_pool_factory_resource_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::RESOURCE_ESCAPE,
        format!("ResourcePool factory cannot capture resource `{binding}`."),
        span,
        "resource captured by ResourcePool factory",
        "ResourcePool factories are eager and noescape in v0.6, but they still must not close over with-bound resources.",
        "avoid_resource_capture",
        "Create the pooled resource directly inside the factory, or pass ordinary managed configuration into the factory.",
    ));
}

pub(super) fn local_class_binding_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_fix(
        code::LOCAL_CLASS_BINDING,
        format!("class binding `{binding}` cannot be local."),
        span,
        "class bound as local",
        "Classes are managed identity objects; their constructors produce managed handles.",
        "use_managed_class_binding",
        format!("Declare `{binding}` with `let` instead of `local`."),
        "machine-applicable",
    ));
}

pub(super) fn invalid_manage_operand_diagnostic(
    analyzer: &mut Analyzer<'_>,
    cause: impl Into<String>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::INVALID_MANAGE_OPERAND,
        "`manage` requires a local binding.",
        span,
        "not a local binding",
        cause,
        "remove_manage_or_create_local",
        "Remove `manage`, or create the value as `local` at its origin.",
    ));
}

pub(super) fn invalid_take_operand_diagnostic(
    analyzer: &mut Analyzer<'_>,
    cause: impl Into<String>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::INVALID_TAKE_OPERAND,
        "`take` requires a local value.",
        span,
        "not a local value",
        cause,
        "use_local_or_read",
        "Pass a local value with `take`, or use `read`/`mut` for managed values.",
    ));
}

pub(super) fn resource_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource `{binding}` cannot be captured by a managed closure."),
            span,
            "resource captured",
        )
        .with_cause("Managed closures may outlive the `with` block that owns the resource."),
    );
}

pub(super) fn fresh_return_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function_name: &str,
    name: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::FRESH_RETURN_NOT_CLEAN,
        format!(
            "fresh function `{}` returns non-fresh value `{name}`.",
            function_name
        ),
        span,
        "non-fresh value returned",
        "A `fresh` return must be newly created or a clean local binding created inside the function.",
        "return_fresh_value",
        "Return a struct constructor, fresh call, or clean local binding created inside the function.",
    ));
}

pub(super) fn freshness_unknown_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function_name: &str,
    issue: FreshReturnIssue,
) {
    analyzer.diagnostics.push(
        Diagnostic::warning(
            code::FRESHNESS_UNKNOWN,
            format!("freshness of return value in `{function_name}` could not be proven."),
            issue.span,
            "freshness unknown",
        )
        .with_cause(
            "This MVP checker trusts clean locals, clean inline fields of locals, struct constructors, known fresh functions, and literals.",
        ),
    );
}

pub(super) fn invalid_fresh_return_type_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    target: &TypeRef,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::INVALID_FRESH_RETURN_TYPE,
        format!(
            "function `{}` declares `fresh {}` but `{}` is not a struct.",
            function.name, target.name, target.name
        ),
        target.span.clone(),
        "invalid fresh type",
        "RSScript `fresh` is a shallow guarantee for newly created struct shells.",
        "use_struct_fresh_type",
        "Return a struct type as fresh, or remove `fresh` from this return contract.",
    ));
}

pub(super) fn trusted_fresh_ident(analyzer: &Analyzer<'_>, name: &str) -> bool {
    analyzer.hir.type_kind(name) == Some(HirTypeKind::Struct)
        || analyzer
            .hir
            .resolve_function(None, name)
            .is_some_and(|signature| signature.returns_fresh)
}
