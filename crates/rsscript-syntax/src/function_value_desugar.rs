//! Syntax-level named-function values via forwarding-closure desugaring.
//!
//! RSScript has no first-class function-pointer value, but a *named function*
//! passed where a `Fn(...)` is expected is exactly what the tinygrad port needs
//! (named matcher/rewrite functions). This pass rewrites a bare argument that
//! names a top-level free function into an equivalent forwarding closure:
//!
//! ```text
//! List.filter(list: read xs, predicate: is_pos)
//! // becomes
//! List.filter(list: read xs, predicate: (x) { return is_pos(x: read x) })
//! ```
//!
//! Doing it at the syntax level (before module isolation and HIR/back-end
//! lowering) means the checker, register VM, and Rust backend all see the same
//! closure, so behaviour is identical across backends with no new value kind.
//! It runs before module isolation, which then mangles the synthesized call like
//! any other reference.

use std::collections::{HashMap, HashSet};

use super::ast::{Block, CallArg, Callee, DataEffect, Expr, Item, MatchArm, Program, Stmt};

/// A free function's parameters: `(name, effect)` in declared order, used to
/// build the forwarding closure's parameter list and forwarding call arguments.
type FunctionParams = Vec<(String, Option<DataEffect>)>;

/// Rewrite bare free-function-name arguments into forwarding closures.
pub fn desugar_function_values(program: &mut Program) {
    let functions = collect_free_functions(program);
    if functions.is_empty() {
        return;
    }
    for item in &mut program.items {
        if let Item::Function(function) = item {
            let mut scope: HashSet<String> =
                function.params.iter().map(|p| p.name.clone()).collect();
            walk_block(&mut function.body, &functions, &mut scope);
        }
    }
}

/// Map each top-level free function (not a `Type.method`, not `main`) to its
/// parameters.
fn collect_free_functions(program: &Program) -> HashMap<String, FunctionParams> {
    let mut functions = HashMap::new();
    for item in &program.items {
        if let Item::Function(function) = item
            && !function.name.contains('.')
            && function.name != "main"
        {
            functions.insert(
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.effective_effect()))
                    .collect(),
            );
        }
    }
    functions
}

fn walk_block(
    block: &mut Block,
    functions: &HashMap<String, FunctionParams>,
    scope: &mut HashSet<String>,
) {
    let saved: Vec<String> = scope.iter().cloned().collect();
    for statement in &mut block.statements {
        walk_stmt(statement, functions, scope);
    }
    scope.clear();
    scope.extend(saved);
}

fn walk_stmt(
    stmt: &mut Stmt,
    functions: &HashMap<String, FunctionParams>,
    scope: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Let(let_stmt) => {
            if let Some(value) = &mut let_stmt.value {
                walk_expr(value, functions, scope);
            }
            scope.insert(let_stmt.name.clone());
        }
        Stmt::LetElse(let_else) => {
            walk_expr(&mut let_else.value, functions, scope);
            let mut else_scope = scope.clone();
            walk_block(&mut let_else.else_body, functions, &mut else_scope);
            scope.insert(let_else.binding_name.clone());
        }
        Stmt::Return(ret) => {
            if let Some(value) = &mut ret.value {
                walk_expr(value, functions, scope);
            }
        }
        Stmt::With(with_stmt) => {
            walk_expr(&mut with_stmt.resource, functions, scope);
            let mut inner = scope.clone();
            inner.insert(with_stmt.binding.clone());
            walk_block(&mut with_stmt.body, functions, &mut inner);
        }
        Stmt::If(if_stmt) => {
            walk_expr(&mut if_stmt.condition, functions, scope);
            let mut then_scope = scope.clone();
            walk_block(&mut if_stmt.then_body, functions, &mut then_scope);
            if let Some(else_body) = &mut if_stmt.else_body {
                let mut else_scope = scope.clone();
                walk_block(else_body, functions, &mut else_scope);
            }
        }
        Stmt::Loop(loop_stmt) => {
            if let Some(condition) = &mut loop_stmt.condition {
                walk_expr(condition, functions, scope);
            }
            let mut inner = scope.clone();
            walk_block(&mut loop_stmt.body, functions, &mut inner);
        }
        Stmt::For(for_stmt) => {
            walk_expr(&mut for_stmt.iterable, functions, scope);
            let mut inner = scope.clone();
            inner.insert(for_stmt.binding.clone());
            walk_block(&mut for_stmt.body, functions, &mut inner);
        }
        Stmt::Match(match_stmt) => {
            walk_expr(&mut match_stmt.value, functions, scope);
            for arm in &mut match_stmt.arms {
                walk_match_arm(arm, functions, scope);
            }
        }
        Stmt::TaskGroup(task_group) => {
            let mut inner = scope.clone();
            walk_block(&mut task_group.body, functions, &mut inner);
        }
        Stmt::Select(select) => {
            for arm in &mut select.arms {
                walk_expr(&mut arm.operation, functions, scope);
                let mut inner = scope.clone();
                inner.insert(arm.binding.clone());
                walk_block(&mut arm.body, functions, &mut inner);
            }
        }
        Stmt::Assign(assign) => {
            walk_expr(&mut assign.target, functions, scope);
            walk_expr(&mut assign.value, functions, scope);
        }
        Stmt::Expr(expr) => walk_expr(expr, functions, scope),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn walk_match_arm(
    arm: &mut MatchArm,
    functions: &HashMap<String, FunctionParams>,
    scope: &mut HashSet<String>,
) {
    let mut inner = scope.clone();
    for name in arm.pattern.binding_names() {
        inner.insert(name.to_string());
    }
    if let Some(guard) = &mut arm.guard {
        walk_expr(guard, functions, &mut inner);
    }
    walk_block(&mut arm.body, functions, &mut inner);
}

fn walk_expr(
    expr: &mut Expr,
    functions: &HashMap<String, FunctionParams>,
    scope: &mut HashSet<String>,
) {
    match expr {
        Expr::Call { callee, args, .. } => {
            if let Callee::ReceiverCall { receiver, .. } = callee {
                walk_expr(receiver, functions, scope);
            }
            for arg in args.iter_mut() {
                // A bare function name in argument position becomes a forwarding
                // closure; anything else is walked normally.
                if let Some(closure) = forwarding_closure_for_arg(&arg.value, functions, scope) {
                    arg.value = closure;
                } else {
                    walk_expr(&mut arg.value, functions, scope);
                }
            }
        }
        Expr::Field { base, .. } => walk_expr(base, functions, scope),
        Expr::Index { base, index, .. } => {
            walk_expr(base, functions, scope);
            walk_expr(index, functions, scope);
        }
        Expr::Binary { left, right, .. } => {
            walk_expr(left, functions, scope);
            walk_expr(right, functions, scope);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => walk_expr(value, functions, scope),
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                walk_expr(&mut field.value, functions, scope);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                walk_expr(&mut entry.key, functions, scope);
                walk_expr(&mut entry.value, functions, scope);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                walk_expr(item, functions, scope);
            }
        }
        Expr::Closure { params, body, .. } => {
            let mut inner = scope.clone();
            inner.extend(params.iter().cloned());
            walk_block(body, functions, &mut inner);
        }
        Expr::Match { value, arms, .. } => {
            walk_expr(value, functions, scope);
            for arm in arms {
                walk_match_arm(arm, functions, scope);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

/// If `value` is a bare identifier naming a top-level free function not shadowed
/// by a local, return a forwarding closure `(p0, ..) { return f(p0: <eff> p0, ..) }`.
fn forwarding_closure_for_arg(
    value: &Expr,
    functions: &HashMap<String, FunctionParams>,
    scope: &HashSet<String>,
) -> Option<Expr> {
    let Expr::Ident(name, span) = value else {
        return None;
    };
    if scope.contains(name) {
        return None;
    }
    let params = functions.get(name)?;
    let call_args = params
        .iter()
        .map(|(param_name, effect)| {
            let ident = Expr::Ident(param_name.clone(), span.clone());
            let value = match effect {
                Some(effect) => Expr::Effect {
                    effect: *effect,
                    value: Box::new(ident),
                    span: span.clone(),
                },
                None => ident,
            };
            CallArg {
                name: Some(param_name.clone()),
                value,
                malformed: false,
                span: span.clone(),
            }
        })
        .collect();
    let call = Expr::Call {
        callee: Callee::Name(name.clone()),
        args: call_args,
        span: span.clone(),
    };
    let body = Block {
        statements: vec![Stmt::Return(super::ast::ReturnStmt {
            value: Some(call),
            span: span.clone(),
        })],
        span: span.clone(),
    };
    Some(Expr::Closure {
        params: params.iter().map(|(name, _)| name.clone()).collect(),
        captures: Vec::new(),
        explicit: false,
        body,
        span: span.clone(),
    })
}
