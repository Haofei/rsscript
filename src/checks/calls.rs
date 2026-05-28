use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{CallResolution, HirBlock, HirCallArg, HirExpr, HirStmt};
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
                        .then_some(binding.name.clone())
                })
                .collect::<HashSet<_>>();
            if let Some(block) = &body.block {
                check_block(analyzer, block, &noescape_bindings);
            }
        }
    }
}

fn check_block(analyzer: &mut Analyzer<'_>, block: &HirBlock, noescape_bindings: &HashSet<String>) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value) => check_expr(analyzer, value, noescape_bindings),
            HirStmt::With { resource, body, .. } => {
                check_expr(analyzer, resource, noescape_bindings);
                check_block(analyzer, body, noescape_bindings);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_expr(analyzer, condition, noescape_bindings);
                check_block(analyzer, then_body, noescape_bindings);
                if let Some(else_body) = else_body {
                    check_block(analyzer, else_body, noescape_bindings);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_expr(analyzer, condition, noescape_bindings);
                }
                check_block(analyzer, body, noescape_bindings);
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn check_expr(analyzer: &mut Analyzer<'_>, expr: &HirExpr, noescape_bindings: &HashSet<String>) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            span,
            resolution,
            ..
        } => {
            check_call_args(analyzer, callee, args, span, resolution, noescape_bindings);
            for arg in args {
                check_expr(analyzer, &arg.value, noescape_bindings);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Try { value, .. } => {
            check_expr(analyzer, value, noescape_bindings);
        }
        HirExpr::Binary { left, right, .. } => {
            check_expr(analyzer, left, noescape_bindings);
            check_expr(analyzer, right, noescape_bindings);
        }
        HirExpr::Field { base, .. } => check_expr(analyzer, base, noescape_bindings),
        HirExpr::Index { base, index, .. } => {
            check_expr(analyzer, base, noescape_bindings);
            check_expr(analyzer, index, noescape_bindings);
        }
        HirExpr::Closure { body, .. } => check_block(analyzer, body, noescape_bindings),
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
    noescape_bindings: &HashSet<String>,
) {
    let call_name = callee_name(callee);
    if is_noescape_callback_call(callee, args, resolution, noescape_bindings) {
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
}

fn is_noescape_callback_call(
    callee: &Callee,
    args: &[HirCallArg],
    resolution: &CallResolution,
    noescape_bindings: &HashSet<String>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && args.is_empty()
        && matches!(callee, Callee::Name(name) if noescape_bindings.contains(name))
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
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}
