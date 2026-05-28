use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::syntax::ast::{Block, CallArg, Callee, DataEffect, Expr, Item, LetKind, Stmt};

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let items = analyzer.syntax_program.items.clone();
    for item in &items {
        if let Item::Function(function) = item {
            let locals = collect_local_bindings_from_block(&function.body);
            check_block(analyzer, &function.body, &locals);
        }
    }
}

fn check_block(analyzer: &mut Analyzer<'_>, block: &Block, locals: &HashSet<String>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    check_expr(analyzer, value, locals);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    check_expr(analyzer, value, locals);
                }
            }
            Stmt::With(stmt) => {
                check_expr(analyzer, &stmt.resource, locals);
                check_block(analyzer, &stmt.body, locals);
            }
            Stmt::If(stmt) => {
                check_expr(analyzer, &stmt.condition, locals);
                check_block(analyzer, &stmt.then_body, locals);
                if let Some(else_body) = &stmt.else_body {
                    check_block(analyzer, else_body, locals);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    check_expr(analyzer, condition, locals);
                }
                check_block(analyzer, &stmt.body, locals);
            }
            Stmt::Expr(expr) => check_expr(analyzer, expr, locals),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(_) => {}
        }
    }
}

fn check_expr(analyzer: &mut Analyzer<'_>, expr: &Expr, locals: &HashSet<String>) {
    match expr {
        Expr::Call {
            callee, args, span, ..
        } => {
            check_call_args(analyzer, callee, args, span, locals);
            for arg in args {
                check_expr(analyzer, &arg.value, locals);
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            check_expr(analyzer, value, locals);
        }
        Expr::Field { base, .. } => check_expr(analyzer, base, locals),
        Expr::Closure { body, .. } => check_block(analyzer, body, locals),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_call_args(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[CallArg],
    call_span: &Span,
    locals: &HashSet<String>,
) {
    let call_name = callee_name(callee);
    if is_enum_variant_call(&call_name) {
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
                .with_cause("RSScript v0.4.1 requires all non-receiver call arguments to be named.")
                .with_fix(
                    "add_argument_name",
                    "Write the argument as `name: value`.",
                    "manual",
                ),
            );
        }
    }

    let Some(signature) = analyzer.resolve_callee(callee) else {
        return;
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
    let retained_params = signature.retained_params.clone();
    let param_names: HashSet<String> = signature_params
        .iter()
        .map(|param| param.name.clone())
        .collect();

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
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
                    arg.value.span().clone(),
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
        let Some(name) = &arg.name else {
            continue;
        };
        if !retained_params.contains(name) {
            continue;
        }
        if let Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } = &arg.value
            && let Expr::Ident(var, span) = value.as_ref()
            && locals.contains(var)
        {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::LOCAL_VALUE_RETAINED,
                    format!("retaining API `{call_name}` cannot retain local value `{var}`."),
                    span.clone(),
                    "local value retained",
                )
                .with_cause(format!(
                    "`{call_name}` declares `effects(retains({name}))`."
                ))
                .with_fix(
                    "manage_local",
                    format!(
                        "Pass `{name}: read (manage {var})` if the value should become managed."
                    ),
                    "manual",
                ),
            );
        }
    }
}

fn is_enum_variant_call(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some" | "None" | "Result" | "Option")
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

fn expr_data_effect(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::Effect { effect, .. } => Some(match effect {
            DataEffect::Read => "read",
            DataEffect::Mut => "mut",
            DataEffect::Take => "take",
        }),
        _ => None,
    }
}

fn collect_local_bindings_from_block(block: &Block) -> HashSet<String> {
    let mut locals = HashSet::new();
    collect_local_bindings_from_statements(&block.statements, &mut locals);
    locals
}

fn collect_local_bindings_from_statements(statements: &[Stmt], locals: &mut HashSet<String>) {
    for statement in statements {
        match statement {
            Stmt::Let(stmt) if stmt.kind == LetKind::Local => {
                locals.insert(stmt.name.clone());
                if let Some(value) = &stmt.value {
                    collect_local_bindings_from_expr(value, locals);
                }
            }
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_local_bindings_from_expr(value, locals);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_local_bindings_from_expr(value, locals);
                }
            }
            Stmt::With(stmt) => {
                collect_local_bindings_from_expr(&stmt.resource, locals);
                collect_local_bindings_from_statements(&stmt.body.statements, locals);
            }
            Stmt::If(stmt) => {
                collect_local_bindings_from_expr(&stmt.condition, locals);
                collect_local_bindings_from_statements(&stmt.then_body.statements, locals);
                if let Some(else_body) = &stmt.else_body {
                    collect_local_bindings_from_statements(&else_body.statements, locals);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_local_bindings_from_expr(condition, locals);
                }
                collect_local_bindings_from_statements(&stmt.body.statements, locals);
            }
            Stmt::Expr(expr) => collect_local_bindings_from_expr(expr, locals),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(_) => {}
        }
    }
}

fn collect_local_bindings_from_expr(expr: &Expr, locals: &mut HashSet<String>) {
    match expr {
        Expr::Call { args, .. } => {
            for arg in args {
                collect_local_bindings_from_expr(&arg.value, locals);
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            collect_local_bindings_from_expr(value, locals);
        }
        Expr::Field { base, .. } => collect_local_bindings_from_expr(base, locals),
        Expr::Closure { body, .. } => {
            collect_local_bindings_from_statements(&body.statements, locals);
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}
