//! Source-body syntax rules that do not require HIR or a backend.

use crate::{task_group_async_let_diagnostics, unsupported_syntax_diagnostic};
use rsscript_diagnostics::Diagnostic;
use rsscript_syntax::ast::{Block, Expr, Item, Stmt};

/// Derive all source-level statement and expression diagnostics for one item.
///
/// This query intentionally accepts only syntax. Type-alias canonicalization
/// remains a compiler fact-extraction concern and is supplied to separate
/// `TypeRef` queries.
pub fn item_body_surface_diagnostics(item: &Item) -> Vec<Diagnostic> {
    let Item::Function(function) = item else {
        return Vec::new();
    };
    block_surface_diagnostics(&function.body, false)
}

/// Derive source-body diagnostics for a block. `in_task_group` is explicit so
/// callers cannot accidentally encode structured-concurrency state in a
/// compiler-side mutable field.
pub fn block_surface_diagnostics(block: &Block, in_task_group: bool) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_block(block, in_task_group, &mut diagnostics);
    diagnostics
}

fn collect_block(block: &Block, in_task_group: bool, diagnostics: &mut Vec<Diagnostic>) {
    for statement in &block.statements {
        collect_statement(statement, in_task_group, diagnostics);
    }
}

fn collect_statement(statement: &Stmt, in_task_group: bool, diagnostics: &mut Vec<Diagnostic>) {
    match statement {
        Stmt::Let(statement) => {
            if statement.malformed {
                diagnostics.push(unsupported_syntax_diagnostic(
                    statement.span.clone(),
                    "malformed statement",
                    "`let` and `local` bindings need a binding name, and an `=` must be followed by an expression.",
                ));
            }
            if statement.is_async && !in_task_group {
                diagnostics.push(unsupported_syntax_diagnostic(
                    statement.span.clone(),
                    "`async let` outside task_group",
                    "`async let` can only be used inside a `task_group { ... }` block.",
                ));
            }
            if let Some(value) = &statement.value {
                collect_expr(value, in_task_group, diagnostics);
            }
        }
        Stmt::Return(statement) => {
            if let Some(value) = &statement.value {
                collect_expr(value, in_task_group, diagnostics);
            }
        }
        Stmt::With(statement) => {
            collect_expr(&statement.resource, in_task_group, diagnostics);
            collect_block(&statement.body, in_task_group, diagnostics);
        }
        Stmt::MalformedWith(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed with statement",
            "`with` statements must use `with resource as name { ... }`.",
        )),
        Stmt::If(statement) => {
            collect_expr(&statement.condition, in_task_group, diagnostics);
            collect_block(&statement.then_body, in_task_group, diagnostics);
            if let Some(else_body) = &statement.else_body {
                collect_block(else_body, in_task_group, diagnostics);
            }
        }
        Stmt::MalformedIf(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed if statement",
            "`if` statements must use `if condition { ... }` with optional `else { ... }` or `else if ...`.",
        )),
        Stmt::Loop(statement) => {
            if let Some(condition) = &statement.condition {
                collect_expr(condition, in_task_group, diagnostics);
            }
            collect_block(&statement.body, in_task_group, diagnostics);
        }
        Stmt::MalformedLoop(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed loop statement",
            "`loop` statements must use `loop { ... }`; `while` statements must use `while condition { ... }`.",
        )),
        Stmt::For(statement) => {
            collect_expr(&statement.iterable, in_task_group, diagnostics);
            collect_block(&statement.body, in_task_group, diagnostics);
        }
        Stmt::TaskGroup(statement) => {
            diagnostics.extend(task_group_async_let_diagnostics(&statement.body));
            collect_block(&statement.body, true, diagnostics);
        }
        Stmt::Select(statement) => {
            for arm in &statement.arms {
                if await_inner(&arm.operation).is_none() {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        arm.span.clone(),
                        "malformed select arm",
                        "Select arms must use `name = await operation => { ... }`.",
                    ));
                }
                collect_expr(&arm.operation, in_task_group, diagnostics);
                collect_block(&arm.body, in_task_group, diagnostics);
            }
        }
        Stmt::MalformedFor(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed for statement",
            "`for` statements must use `for name in iterable { ... }`.",
        )),
        Stmt::Match(statement) => {
            collect_expr(&statement.value, in_task_group, diagnostics);
            for span in &statement.malformed_arm_spans {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "malformed match arm",
                    "Match arms must use `pattern => statement` or `pattern => { ... }`.",
                ));
            }
            for arm in &statement.arms {
                collect_block(&arm.body, in_task_group, diagnostics);
            }
        }
        Stmt::LetElse(statement) => {
            collect_expr(&statement.value, in_task_group, diagnostics);
            collect_block(&statement.else_body, in_task_group, diagnostics);
        }
        Stmt::MalformedMatch(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed match statement",
            "`match` statements must use `match value { pattern => ... }`.",
        )),
        Stmt::Assign(statement) => {
            collect_expr(&statement.target, in_task_group, diagnostics);
            collect_expr(&statement.value, in_task_group, diagnostics);
        }
        Stmt::Expr(expr) => collect_expr(expr, in_task_group, diagnostics),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Unknown(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "unsupported statement",
            "This statement is outside the current RSScript parser surface.",
        )),
    }
}

fn collect_expr(expr: &Expr, in_task_group: bool, diagnostics: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_expr(left, in_task_group, diagnostics);
            collect_expr(right, in_task_group, diagnostics);
        }
        Expr::Field { base, .. } => collect_expr(base, in_task_group, diagnostics),
        Expr::Index { base, index, .. } => {
            collect_expr(base, in_task_group, diagnostics);
            collect_expr(index, in_task_group, diagnostics);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                if arg.malformed {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        arg.span.clone(),
                        "malformed call argument",
                        "Call arguments cannot contain empty argument slots.",
                    ));
                } else {
                    collect_expr(&arg.value, in_task_group, diagnostics);
                }
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            collect_expr(value, in_task_group, diagnostics);
        }
        Expr::Spawn { value, span } => {
            diagnostics.push(unsupported_syntax_diagnostic(
                span.clone(),
                "unsupported spawn expression",
                "`spawn` is not a v0.7 source-level task feature. Use `task_group { async let ... }` for structured isolate-local async work.",
            ));
            collect_expr(value, in_task_group, diagnostics);
        }
        Expr::Await { value, .. } => collect_expr(value, in_task_group, diagnostics),
        Expr::Closure { body, .. } => collect_block(body, in_task_group, diagnostics),
        Expr::Match {
            value,
            arms,
            malformed_arm_spans,
            ..
        } => {
            collect_expr(value, in_task_group, diagnostics);
            for span in malformed_arm_spans {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "malformed match arm",
                    "Match arms must use `pattern => statement` or `pattern => { ... }`.",
                ));
            }
            for arm in arms {
                collect_block(&arm.body, in_task_group, diagnostics);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr(&field.value, in_task_group, diagnostics);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr(&entry.key, in_task_group, diagnostics);
                collect_expr(&entry.value, in_task_group, diagnostics);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr(item, in_task_group, diagnostics);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _) => {}
        Expr::Unknown(span) => diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "unsupported expression",
            "This expression is outside the current RSScript parser surface.",
        )),
    }
}

fn await_inner(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Try { value, .. } => match value.as_ref() {
            Expr::Await { value, .. } => Some(value),
            _ => None,
        },
        Expr::Await { value, .. } => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_rules_reject_spawn_and_async_let_outside_task_group() {
        let program = rsscript_syntax::parse_source(
            "body.rss",
            "fn work() -> Unit { async let task = run(); spawn task }",
        );
        let diagnostics = item_body_surface_diagnostics(&program.items[0]);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].label, "`async let` outside task_group");
        assert_eq!(diagnostics[1].label, "unsupported spawn expression");
    }
}
