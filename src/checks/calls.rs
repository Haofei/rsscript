use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{CallResolution, HirBindingKind, HirBlock, HirCallArg, HirExpr, HirStmt};
use crate::syntax::ast::{Callee, Item};

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let items = analyzer.syntax_program.items.clone();
    for item in &items {
        if let Item::Function(function) = item {
            let Some(body) = analyzer.hir.function_body(&function.name).cloned() else {
                continue;
            };
            let noescape_bindings = body
                .bindings
                .iter()
                .filter_map(|binding| {
                    (binding.type_name.as_deref() == Some("noescape Fn()"))
                        .then_some((binding.name.clone(), binding.span.clone()))
                })
                .collect::<HashMap<_, _>>();
            if let Some(block) = &body.block {
                let mut local_closure_bindings = HashMap::new();
                collect_local_closure_bindings(block, &mut local_closure_bindings);
                check_block(analyzer, block, &noescape_bindings, &local_closure_bindings);
            }
        }
    }
}

fn collect_local_closure_bindings(block: &HirBlock, bindings: &mut HashMap<String, Span>) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                kind: HirBindingKind::LocalLet,
                name,
                value: Some(HirExpr::Closure { body, .. }),
                span,
                ..
            } => {
                bindings.insert(name.clone(), span.clone());
                collect_local_closure_bindings(body, bindings);
            }
            HirStmt::Let {
                value: Some(HirExpr::Closure { body, .. }),
                ..
            } => collect_local_closure_bindings(body, bindings),
            HirStmt::Let { .. } | HirStmt::Return { .. } | HirStmt::Expr(_) => {}
            HirStmt::With { body, .. } => collect_local_closure_bindings(body, bindings),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_closure_bindings(then_body, bindings);
                if let Some(else_body) = else_body {
                    collect_local_closure_bindings(else_body, bindings);
                }
            }
            HirStmt::Loop { body, .. } => collect_local_closure_bindings(body, bindings),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_local_closure_bindings(&arm.body, bindings);
                }
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }
}

fn check_block(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    noescape_bindings: &HashMap<String, Span>,
    local_closure_bindings: &HashMap<String, Span>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value),
                span,
                ..
            } => {
                check_noescape_escape(
                    analyzer,
                    value,
                    span,
                    noescape_bindings,
                    NoescapeEscapeContext::Store,
                );
                check_local_closure_escape(
                    analyzer,
                    value,
                    span,
                    local_closure_bindings,
                    LocalClosureEscapeContext::Store,
                );
                check_expr(analyzer, value, noescape_bindings, local_closure_bindings);
            }
            HirStmt::Return {
                value: Some(value), ..
            } => {
                check_noescape_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    noescape_bindings,
                    NoescapeEscapeContext::Return,
                );
                check_local_closure_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    local_closure_bindings,
                    LocalClosureEscapeContext::Return,
                );
                check_expr(analyzer, value, noescape_bindings, local_closure_bindings);
            }
            HirStmt::Expr(value) => {
                check_noescape_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    noescape_bindings,
                    NoescapeEscapeContext::UseAsValue,
                );
                check_local_closure_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    local_closure_bindings,
                    LocalClosureEscapeContext::UseAsValue,
                );
                check_expr(analyzer, value, noescape_bindings, local_closure_bindings);
            }
            HirStmt::With { resource, body, .. } => {
                check_expr(
                    analyzer,
                    resource,
                    noescape_bindings,
                    local_closure_bindings,
                );
                check_block(analyzer, body, noescape_bindings, local_closure_bindings);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_expr(
                    analyzer,
                    condition,
                    noescape_bindings,
                    local_closure_bindings,
                );
                check_block(
                    analyzer,
                    then_body,
                    noescape_bindings,
                    local_closure_bindings,
                );
                if let Some(else_body) = else_body {
                    check_block(
                        analyzer,
                        else_body,
                        noescape_bindings,
                        local_closure_bindings,
                    );
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_expr(
                        analyzer,
                        condition,
                        noescape_bindings,
                        local_closure_bindings,
                    );
                }
                check_block(analyzer, body, noescape_bindings, local_closure_bindings);
            }
            HirStmt::Match { value, arms, .. } => {
                check_expr(analyzer, value, noescape_bindings, local_closure_bindings);
                for arm in arms {
                    check_block(
                        analyzer,
                        &arm.body,
                        noescape_bindings,
                        local_closure_bindings,
                    );
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn check_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    noescape_bindings: &HashMap<String, Span>,
    local_closure_bindings: &HashMap<String, Span>,
) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            span,
            resolution,
            ..
        } => {
            check_call_args(
                analyzer,
                callee,
                args,
                span,
                resolution,
                noescape_bindings,
                local_closure_bindings,
            );
            for arg in args {
                check_expr(
                    analyzer,
                    &arg.value,
                    noescape_bindings,
                    local_closure_bindings,
                );
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_expr(analyzer, value, noescape_bindings, local_closure_bindings);
        }
        HirExpr::Binary { left, right, .. } => {
            check_expr(analyzer, left, noescape_bindings, local_closure_bindings);
            check_expr(analyzer, right, noescape_bindings, local_closure_bindings);
        }
        HirExpr::Field { base, .. } => {
            check_expr(analyzer, base, noescape_bindings, local_closure_bindings)
        }
        HirExpr::Index { base, index, .. } => {
            check_expr(analyzer, base, noescape_bindings, local_closure_bindings);
            check_expr(analyzer, index, noescape_bindings, local_closure_bindings);
        }
        HirExpr::Closure { body, .. } => {
            check_block(analyzer, body, noescape_bindings, local_closure_bindings)
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_call_args(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
    resolution: &CallResolution,
    noescape_bindings: &HashMap<String, Span>,
    local_closure_bindings: &HashMap<String, Span>,
) {
    let call_name = callee_name(callee);
    if is_closure_binding_call(
        callee,
        args,
        resolution,
        noescape_bindings,
        local_closure_bindings,
    ) {
        return;
    }
    if matches!(resolution, CallResolution::EnumVariant) {
        return;
    }

    for arg in args {
        if arg.name.is_none() {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNNAMED_ARGUMENT,
                    format!("call to `{call_name}` uses an unnamed argument."),
                    arg.span.clone(),
                    "argument must be named",
                )
                .with_cause("RSScript v0.5 requires all non-receiver call arguments to be named.")
                .with_fix(
                    "add_argument_name",
                    "Write the argument as `name: value`.",
                    "manual",
                ),
            );
        }
    }

    let signature = match resolution {
        CallResolution::Resolved { signature, .. } => signature,
        CallResolution::Unknown => {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_CALLEE,
                    format!("call to `{}` does not resolve.", callee_display(callee)),
                    call_span.clone(),
                    "unknown callee",
                )
                .with_cause(
                    "The callee is not a user function, known type constructor, enum variant, or builtin signature.",
                )
                .with_fix(
                    "declare_or_import_callee",
                    "Declare the function or add a builtin signature for this API.",
                    "manual",
                ),
            );
            return;
        }
        CallResolution::EnumVariant => return,
    };
    let signature_params = signature.params.clone();
    let param_effects: HashMap<String, &'static str> = signature
        .params
        .iter()
        .filter_map(|param| {
            param
                .effect
                .map(|effect| (param.name.clone(), effect.as_str()))
        })
        .collect();
    let param_names: HashSet<String> = signature_params
        .iter()
        .map(|param| param.name.clone())
        .collect();

    let mut seen_names = HashSet::new();
    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        if !seen_names.insert(name.as_str()) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_ARGUMENT,
                    format!("call to `{call_name}` repeats argument `{name}`."),
                    arg.span.clone(),
                    "duplicate argument",
                )
                .with_cause("Each named parameter can be provided at most once.")
                .with_fix(
                    "remove_duplicate_argument",
                    format!("Remove the extra `{name}: ...` argument."),
                    "manual",
                ),
            );
        }
        if !param_names.contains(name) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_ARGUMENT,
                    format!("call to `{call_name}` has no argument named `{name}`."),
                    arg.span.clone(),
                    "unknown argument",
                )
                .with_cause(format!(
                    "`{call_name}` does not declare a parameter named `{name}`."
                ))
                .with_fix(
                    "rename_argument",
                    format!("Use one of: {}.", join_param_names(&signature_params)),
                    "manual",
                ),
            );
        }
    }

    if args.iter().all(|arg| arg.name.is_some()) {
        let provided_names: HashSet<&str> =
            args.iter().filter_map(|arg| arg.name.as_deref()).collect();
        for param in &signature_params {
            if !provided_names.contains(param.name.as_str()) {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::MISSING_ARGUMENT,
                        format!(
                            "call to `{call_name}` is missing required argument `{}`.",
                            param.name
                        ),
                        call_span.clone(),
                        "missing argument",
                    )
                    .with_cause(format!(
                        "`{call_name}` requires a named argument `{}`.",
                        param.name
                    ))
                    .with_fix(
                        "add_argument",
                        format!("Add `{}: ...` to the call.", param.name),
                        "manual",
                    ),
                );
            }
        }
    }

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        let Some(expected) = param_effects.get(name) else {
            continue;
        };
        if expr_data_effect(&arg.value) != Some(*expected) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::MISSING_DATA_EFFECT,
                    format!("argument `{name}` for `{call_name}` is missing `{expected}`."),
                    hir_expr_span(&arg.value).clone(),
                    "missing data effect",
                )
                .with_cause("Non-Copy parameters require an explicit `read`, `mut`, or `take` call-site effect.")
                .with_fix(
                    "add_data_effect",
                    format!("Write `{name}: {expected} ...` at the call site."),
                    "machine-applicable",
                ),
            );
        }
    }

    for arg in args {
        let expected_param = arg
            .name
            .as_ref()
            .and_then(|name| signature.params.iter().find(|param| param.name == *name));
        if expected_param.is_some_and(|param| param.type_name == "noescape Fn()") {
            continue;
        }
        check_noescape_escape(
            analyzer,
            &arg.value,
            &arg.span,
            noescape_bindings,
            NoescapeEscapeContext::Pass { callee: &call_name },
        );
        check_local_closure_escape(
            analyzer,
            &arg.value,
            &arg.span,
            local_closure_bindings,
            LocalClosureEscapeContext::Pass { callee: &call_name },
        );
    }
}

fn is_closure_binding_call(
    callee: &Callee,
    args: &[HirCallArg],
    resolution: &CallResolution,
    noescape_bindings: &HashMap<String, Span>,
    local_closure_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && args.is_empty()
        && matches!(callee, Callee::Name(name) if noescape_bindings.contains_key(name) || local_closure_bindings.contains_key(name))
}

fn is_noescape_callback_call(
    callee: &Callee,
    args: &[HirCallArg],
    resolution: &CallResolution,
    noescape_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && args.is_empty()
        && matches!(callee, Callee::Name(name) if noescape_bindings.contains_key(name))
}

#[derive(Debug, Clone, Copy)]
enum NoescapeEscapeContext<'a> {
    Store,
    Return,
    UseAsValue,
    Pass { callee: &'a str },
}

#[derive(Debug, Clone, Copy)]
enum LocalClosureEscapeContext<'a> {
    Store,
    Return,
    UseAsValue,
    Pass { callee: &'a str },
}

fn check_local_closure_escape(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    context_span: &Span,
    local_closure_bindings: &HashMap<String, Span>,
    context: LocalClosureEscapeContext<'_>,
) {
    let Some((name, use_span)) = local_closure_escape_use(expr, local_closure_bindings) else {
        return;
    };
    analyzer.diagnostics.push(local_closure_escape_diagnostic(
        name,
        use_span,
        context_span.clone(),
        context,
    ));
}

fn local_closure_escape_use<'a>(
    expr: &'a HirExpr,
    local_closure_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if local_closure_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } if is_local_closure_call(callee, args, resolution, local_closure_bindings) => None,
        HirExpr::Call {
            args, resolution, ..
        } => {
            for arg in args {
                if call_arg_targets_noescape_param(arg, resolution) {
                    continue;
                }
                if let Some(found) = local_closure_escape_use(&arg.value, local_closure_bindings) {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => local_closure_escape_use(value, local_closure_bindings),
        HirExpr::Binary { left, right, .. } => {
            local_closure_escape_use(left, local_closure_bindings)
                .or_else(|| local_closure_escape_use(right, local_closure_bindings))
        }
        HirExpr::Field { base, .. } => local_closure_escape_use(base, local_closure_bindings),
        HirExpr::Index { base, index, .. } => {
            local_closure_escape_use(base, local_closure_bindings)
                .or_else(|| local_closure_escape_use(index, local_closure_bindings))
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                if let Some(found) =
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn is_local_closure_call(
    callee: &Callee,
    args: &[HirCallArg],
    resolution: &CallResolution,
    local_closure_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && args.is_empty()
        && matches!(callee, Callee::Name(name) if local_closure_bindings.contains_key(name))
}

fn check_noescape_escape(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    context_span: &Span,
    noescape_bindings: &HashMap<String, Span>,
    context: NoescapeEscapeContext<'_>,
) {
    let Some((name, use_span)) = noescape_escape_use(expr, noescape_bindings) else {
        return;
    };
    analyzer.diagnostics.push(noescape_escape_diagnostic(
        name,
        use_span,
        context_span.clone(),
        context,
    ));
}

fn noescape_escape_use<'a>(
    expr: &'a HirExpr,
    noescape_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if noescape_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } if is_noescape_callback_call(callee, args, resolution, noescape_bindings) => None,
        HirExpr::Call {
            args, resolution, ..
        } => {
            for arg in args {
                if call_arg_targets_noescape_param(arg, resolution) {
                    continue;
                }
                if let Some(found) = noescape_escape_use(&arg.value, noescape_bindings) {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => noescape_escape_use(value, noescape_bindings),
        HirExpr::Binary { left, right, .. } => noescape_escape_use(left, noescape_bindings)
            .or_else(|| noescape_escape_use(right, noescape_bindings)),
        HirExpr::Field { base, .. } => noescape_escape_use(base, noescape_bindings),
        HirExpr::Index { base, index, .. } => noescape_escape_use(base, noescape_bindings)
            .or_else(|| noescape_escape_use(index, noescape_bindings)),
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                if let Some(found) = noescape_any_use_in_stmt(statement, noescape_bindings) {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn noescape_any_use_in_stmt<'a>(
    statement: &'a HirStmt,
    noescape_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => noescape_any_use(value, noescape_bindings),
        HirStmt::With { resource, body, .. } => noescape_any_use(resource, noescape_bindings)
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            }),
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => noescape_any_use(condition, noescape_bindings)
            .or_else(|| {
                then_body
                    .statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            })
            .or_else(|| {
                else_body.as_ref().and_then(|body| {
                    body.statements.iter().find_map(|statement| {
                        noescape_any_use_in_stmt(statement, noescape_bindings)
                    })
                })
            }),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(|condition| noescape_any_use(condition, noescape_bindings))
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            }),
        HirStmt::Match { value, arms, .. } => {
            noescape_any_use(value, noescape_bindings).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        noescape_any_use_in_stmt(statement, noescape_bindings)
                    })
                })
            })
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn noescape_any_use<'a>(
    expr: &'a HirExpr,
    noescape_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if noescape_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if let Callee::Name(name) = callee
                && noescape_bindings.contains_key(name)
            {
                return Some((name.as_str(), span.clone()));
            }
            args.iter()
                .find_map(|arg| noescape_any_use(&arg.value, noescape_bindings))
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => noescape_any_use(value, noescape_bindings),
        HirExpr::Binary { left, right, .. } => noescape_any_use(left, noescape_bindings)
            .or_else(|| noescape_any_use(right, noescape_bindings)),
        HirExpr::Field { base, .. } => noescape_any_use(base, noescape_bindings),
        HirExpr::Index { base, index, .. } => noescape_any_use(base, noescape_bindings)
            .or_else(|| noescape_any_use(index, noescape_bindings)),
        HirExpr::Closure { body, .. } => body
            .statements
            .iter()
            .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings)),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn local_closure_any_use_in_stmt<'a>(
    statement: &'a HirStmt,
    local_closure_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => local_closure_any_use(value, local_closure_bindings),
        HirStmt::With { resource, body, .. } => {
            local_closure_any_use(resource, local_closure_bindings).or_else(|| {
                body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            })
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => local_closure_any_use(condition, local_closure_bindings)
            .or_else(|| {
                then_body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            })
            .or_else(|| {
                else_body.as_ref().and_then(|body| {
                    body.statements.iter().find_map(|statement| {
                        local_closure_any_use_in_stmt(statement, local_closure_bindings)
                    })
                })
            }),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(|condition| local_closure_any_use(condition, local_closure_bindings))
            .or_else(|| {
                body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            }),
        HirStmt::Match { value, arms, .. } => local_closure_any_use(value, local_closure_bindings)
            .or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        local_closure_any_use_in_stmt(statement, local_closure_bindings)
                    })
                })
            }),
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn local_closure_any_use<'a>(
    expr: &'a HirExpr,
    local_closure_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if local_closure_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if let Callee::Name(name) = callee
                && local_closure_bindings.contains_key(name)
            {
                return Some((name.as_str(), span.clone()));
            }
            args.iter()
                .find_map(|arg| local_closure_any_use(&arg.value, local_closure_bindings))
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => local_closure_any_use(value, local_closure_bindings),
        HirExpr::Binary { left, right, .. } => local_closure_any_use(left, local_closure_bindings)
            .or_else(|| local_closure_any_use(right, local_closure_bindings)),
        HirExpr::Field { base, .. } => local_closure_any_use(base, local_closure_bindings),
        HirExpr::Index { base, index, .. } => local_closure_any_use(base, local_closure_bindings)
            .or_else(|| local_closure_any_use(index, local_closure_bindings)),
        HirExpr::Closure { body, .. } => body
            .statements
            .iter()
            .find_map(|statement| local_closure_any_use_in_stmt(statement, local_closure_bindings)),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn call_arg_targets_noescape_param(arg: &HirCallArg, resolution: &CallResolution) -> bool {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return false;
    };
    arg.name
        .as_ref()
        .and_then(|name| signature.params.iter().find(|param| param.name == *name))
        .is_some_and(|param| param.type_name == "noescape Fn()")
}

fn noescape_escape_diagnostic(
    name: &str,
    use_span: Span,
    context_span: Span,
    context: NoescapeEscapeContext<'_>,
) -> Diagnostic {
    let (summary, cause) = match context {
        NoescapeEscapeContext::Store => (
            format!("noescape callback `{name}` cannot be stored."),
            "`noescape Fn()` parameters are temporary callback capabilities and cannot be bound into stored values.".to_string(),
        ),
        NoescapeEscapeContext::Return => (
            format!("noescape callback `{name}` cannot be returned."),
            "`noescape Fn()` parameters cannot escape the current function through a return value.".to_string(),
        ),
        NoescapeEscapeContext::UseAsValue => (
            format!("noescape callback `{name}` cannot be used as an ordinary value."),
            "Call the noescape callback directly, or pass it to another resolved `noescape Fn()` parameter.".to_string(),
        ),
        NoescapeEscapeContext::Pass { callee } => (
            format!("noescape callback `{name}` cannot be passed to `{callee}` as an ordinary value."),
            "Forwarding a noescape callback is only allowed when the target parameter is also `noescape Fn()`.".to_string(),
        ),
    };
    Diagnostic::error(
        code::NOESCAPE_CALLBACK_ESCAPE,
        summary,
        use_span,
        "noescape callback escapes",
    )
    .with_cause(cause)
    .with_cause(format!(
        "The escaping context starts at {}:{}.",
        context_span.line, context_span.column
    ))
    .with_fix(
        "keep_noescape_local",
        "Call the callback directly, or change the API to an ordinary managed callback type.",
        "manual",
    )
}

fn local_closure_escape_diagnostic(
    name: &str,
    use_span: Span,
    context_span: Span,
    context: LocalClosureEscapeContext<'_>,
) -> Diagnostic {
    let (summary, cause) = match context {
        LocalClosureEscapeContext::Store => (
            format!("local closure `{name}` cannot be stored in a managed binding."),
            "A closure bound with `local` is an exclusive local capability and cannot become managed data.".to_string(),
        ),
        LocalClosureEscapeContext::Return => (
            format!("local closure `{name}` cannot be returned."),
            "A local closure cannot escape the function where its local captures are valid.".to_string(),
        ),
        LocalClosureEscapeContext::UseAsValue => (
            format!("local closure `{name}` cannot be used as an ordinary value."),
            "Call the local closure directly, or pass it to a resolved `noescape Fn()` parameter.".to_string(),
        ),
        LocalClosureEscapeContext::Pass { callee } => (
            format!("local closure `{name}` cannot be passed to `{callee}` as an ordinary value."),
            "Forwarding a local closure is only allowed when the target parameter is `noescape Fn()`.".to_string(),
        ),
    };
    Diagnostic::error(
        code::LOCAL_CLOSURE_ESCAPE,
        summary,
        use_span,
        "local closure escapes",
    )
    .with_cause(cause)
    .with_cause(format!(
        "The escaping context starts at {}:{}.",
        context_span.line, context_span.column
    ))
    .with_fix(
        "keep_local_closure_noescape",
        "Call the closure locally, or pass it to a noescape callback parameter.",
        "manual",
    )
}

fn join_param_names(params: &[crate::hir::ParamSig]) -> String {
    params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn callee_name(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { name, .. } => name.clone(),
    }
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
    }
}

fn expr_data_effect(expr: &HirExpr) -> Option<&'static str> {
    match expr {
        HirExpr::Effect { effect, .. } => Some(effect.as_str()),
        _ => None,
    }
}

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
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
        | HirExpr::Unknown(span) => span,
    }
}
