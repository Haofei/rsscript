//! Source-level structured-concurrency rules for `task_group` async lets.

use std::collections::HashSet;

use crate::unsupported_syntax_diagnostic;
use rsscript_diagnostics::{Diagnostic, Span};
use rsscript_syntax::ast::{Block, Expr, Stmt};

/// Derive the complete source-level contract for `async let` declarations in
/// one `task_group` body.
///
/// Task groups own their pending handles lexically. A handle must be awaited
/// exactly once, from a direct statement after its declaration; nested source
/// blocks may not introduce a handle belonging to the enclosing group. Nested
/// `task_group` and `select` bodies are independent structured boundaries and
/// are deliberately not traversed here.
pub fn task_group_async_let_diagnostics(block: &Block) -> Vec<Diagnostic> {
    let top_level = block
        .statements
        .iter()
        .filter_map(|statement| match statement {
            Stmt::Let(let_statement) if let_statement.is_async && let_statement.name != "_" => {
                Some(let_statement.name.clone())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();

    let mut facts = TaskGroupFacts::default();
    collect_block(block, true, &mut facts);
    let mut diagnostics = Vec::new();

    for span in facts.nested_async_lets {
        diagnostics.push(unsupported_syntax_diagnostic(
            span,
            "nested async let",
            "`async let` is currently supported only as a direct child of `task_group { ... }` so checking and lowering share one structured-concurrency model.",
        ));
    }

    for (name, span) in &facts.async_lets {
        if name != "_" && !facts.direct_awaited.contains(name) {
            diagnostics.push(unsupported_syntax_diagnostic(
                span.clone(),
                "unawaited async let",
                "`async let` handles are lexical task_group handles and must be consumed by `await` inside the same `task_group { ... }` block.",
            ));
        }
    }

    for (name, span) in &facts.all_awaits {
        if top_level.contains(name) && !facts.direct_awaited.contains(name) {
            diagnostics.push(unsupported_syntax_diagnostic(
                span.clone(),
                "nested async let await",
                "`await` of a task_group async-let handle must be a direct task_group statement in the v0.7 executable MVP.",
            ));
        }
    }

    let mut declared = HashSet::new();
    let mut consumed = HashSet::new();
    for statement in &block.statements {
        if let Stmt::Let(let_statement) = statement
            && let_statement.is_async
        {
            if let_statement.name != "_" {
                declared.insert(let_statement.name.clone());
            }
            continue;
        }
        for (name, span) in direct_awaits_in_statement(statement) {
            if !top_level.contains(&name) {
                continue;
            }
            if !declared.contains(&name) {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span,
                    "async let await before declaration",
                    "`await` of a task_group async-let handle must appear after the matching `async let` declaration.",
                ));
            } else if !consumed.insert(name) {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span,
                    "async let awaited more than once",
                    "`async let` handles are bounded task_group handles and can be consumed by `await` only once.",
                ));
            }
        }
    }

    diagnostics
}

#[derive(Default)]
struct TaskGroupFacts {
    async_lets: Vec<(String, Span)>,
    nested_async_lets: Vec<Span>,
    all_awaits: Vec<(String, Span)>,
    direct_awaited: HashSet<String>,
}

fn collect_block(block: &Block, is_root: bool, facts: &mut TaskGroupFacts) {
    for statement in &block.statements {
        let first_await = facts.all_awaits.len();
        collect_statement(statement, is_root, facts);
        // A direct task-group statement may contain structured subexpressions.
        // Preserve the v0.7 rule's historic meaning of “direct” here: awaits
        // reached from such a statement count as consumption, while the stricter
        // syntactic sequence check below still sees only a top-level await.
        if is_root && matches!(statement, Stmt::Let(_) | Stmt::Assign(_) | Stmt::Expr(_)) {
            facts.direct_awaited.extend(
                facts.all_awaits[first_await..]
                    .iter()
                    .map(|(name, _)| name.clone()),
            );
        }
    }
}

fn collect_statement(statement: &Stmt, is_root: bool, facts: &mut TaskGroupFacts) {
    match statement {
        Stmt::Let(let_statement) => {
            if let_statement.is_async {
                facts
                    .async_lets
                    .push((let_statement.name.clone(), let_statement.span.clone()));
                if !is_root {
                    facts.nested_async_lets.push(let_statement.span.clone());
                }
            }
            if let Some(value) = &let_statement.value {
                collect_expr(value, facts);
            }
        }
        Stmt::Return(return_statement) => {
            if let Some(value) = &return_statement.value {
                collect_expr(value, facts);
            }
        }
        Stmt::With(with_statement) => {
            collect_expr(&with_statement.resource, facts);
            collect_block(&with_statement.body, false, facts);
        }
        Stmt::If(if_statement) => {
            collect_expr(&if_statement.condition, facts);
            collect_block(&if_statement.then_body, false, facts);
            if let Some(else_body) = &if_statement.else_body {
                collect_block(else_body, false, facts);
            }
        }
        Stmt::Loop(loop_statement) => {
            if let Some(condition) = &loop_statement.condition {
                collect_expr(condition, facts);
            }
            collect_block(&loop_statement.body, false, facts);
        }
        Stmt::For(for_statement) => {
            collect_expr(&for_statement.iterable, facts);
            collect_block(&for_statement.body, false, facts);
        }
        Stmt::Match(match_statement) => {
            collect_expr(&match_statement.value, facts);
            for arm in &match_statement.arms {
                if let Some(guard) = &arm.guard {
                    collect_expr(guard, facts);
                }
                collect_block(&arm.body, false, facts);
            }
        }
        Stmt::LetElse(let_else) => {
            collect_expr(&let_else.value, facts);
            collect_block(&let_else.else_body, false, facts);
        }
        Stmt::Assign(assign) => {
            collect_expr(&assign.target, facts);
            collect_expr(&assign.value, facts);
        }
        Stmt::Expr(expr) => collect_expr(expr, facts),
        Stmt::TaskGroup(_)
        | Stmt::Select(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::Unknown(_) => {}
    }
}

fn collect_expr(expr: &Expr, facts: &mut TaskGroupFacts) {
    match expr {
        Expr::Await { value, span } => {
            if let Some(name) = await_handle_name(value) {
                facts.all_awaits.push((name.to_owned(), span.clone()));
            }
            collect_expr(value, facts);
        }
        Expr::Binary { left, right, .. } => {
            collect_expr(left, facts);
            collect_expr(right, facts);
        }
        Expr::Field { base, .. } => collect_expr(base, facts),
        Expr::Index { base, index, .. } => {
            collect_expr(base, facts);
            collect_expr(index, facts);
        }
        Expr::Call { args, .. } => {
            for argument in args {
                collect_expr(&argument.value, facts);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => collect_expr(value, facts),
        Expr::Closure { body, .. } => collect_block(body, false, facts),
        Expr::Match { value, arms, .. } => {
            collect_expr(value, facts);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr(guard, facts);
                }
                collect_block(&arm.body, false, facts);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr(&field.value, facts);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr(&entry.key, facts);
                collect_expr(&entry.value, facts);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr(item, facts);
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

fn direct_awaits_in_statement(statement: &Stmt) -> Vec<(String, Span)> {
    match statement {
        Stmt::Let(let_statement) => let_statement
            .value
            .as_ref()
            .and_then(direct_await)
            .into_iter()
            .collect(),
        Stmt::Assign(assign) => [direct_await(&assign.target), direct_await(&assign.value)]
            .into_iter()
            .flatten()
            .collect(),
        Stmt::Expr(expr) => direct_await(expr).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn direct_await(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Await { value, span } => {
            await_handle_name(value).map(|name| (name.to_owned(), span.clone()))
        }
        Expr::Try { value, .. } => direct_await(value),
        _ => None,
    }
}

fn await_handle_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => await_handle_name(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_group_body(source: &str) -> Block {
        let program = rsscript_syntax::parse_source("task_group.rss", source);
        let rsscript_syntax::ast::Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };
        let Stmt::TaskGroup(group) = &function.body.statements[0] else {
            panic!("expected task group");
        };
        group.body.clone()
    }

    #[test]
    fn requires_direct_single_consumption_after_declaration() {
        let body = task_group_body(
            "fn run() { task_group { async let pending = work(); let value = await pending; await pending } }",
        );
        let labels = task_group_async_let_diagnostics(&body)
            .into_iter()
            .map(|diagnostic| diagnostic.label)
            .collect::<Vec<_>>();
        assert!(labels.contains(&"async let awaited more than once".to_owned()));
    }

    #[test]
    fn rejects_nested_async_let_and_await() {
        let body = task_group_body(
            "fn run() { task_group { async let pending = work(); if true { async let nested = work(); await pending } } }",
        );
        let labels = task_group_async_let_diagnostics(&body)
            .into_iter()
            .map(|diagnostic| diagnostic.label)
            .collect::<Vec<_>>();
        assert!(labels.contains(&"nested async let".to_owned()));
        assert!(labels.contains(&"nested async let await".to_owned()));
    }

    #[test]
    fn direct_statement_subexpressions_still_consume_a_handle() {
        let body = task_group_body(
            "fn run() { task_group { async let pending = work(); let value = Result.wrap(await pending) } }",
        );
        let labels = task_group_async_let_diagnostics(&body)
            .into_iter()
            .map(|diagnostic| diagnostic.label)
            .collect::<Vec<_>>();
        assert!(!labels.contains(&"unawaited async let".to_owned()));
        assert!(!labels.contains(&"nested async let await".to_owned()));
    }
}
