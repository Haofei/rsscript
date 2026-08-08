use std::collections::HashSet;

use crate::syntax::ast::{Block, Expr, Stmt};

pub(super) fn collect_task_group_async_lets(
    block: &Block,
    async_lets: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.is_async {
                    async_lets.push((stmt.name.clone(), stmt.span.clone()));
                }
                if let Some(value) = &stmt.value {
                    collect_task_group_async_lets_expr(value, async_lets);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_async_lets_expr(value, async_lets);
                }
            }
            Stmt::With(stmt) => {
                collect_task_group_async_lets_expr(&stmt.resource, async_lets);
                collect_task_group_async_lets(&stmt.body, async_lets);
            }
            Stmt::If(stmt) => {
                collect_task_group_async_lets_expr(&stmt.condition, async_lets);
                collect_task_group_async_lets(&stmt.then_body, async_lets);
                if let Some(else_body) = &stmt.else_body {
                    collect_task_group_async_lets(else_body, async_lets);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_task_group_async_lets_expr(condition, async_lets);
                }
                collect_task_group_async_lets(&stmt.body, async_lets);
            }
            Stmt::For(stmt) => {
                collect_task_group_async_lets_expr(&stmt.iterable, async_lets);
                collect_task_group_async_lets(&stmt.body, async_lets);
            }
            Stmt::Match(stmt) => {
                collect_task_group_async_lets_expr(&stmt.value, async_lets);
                for arm in &stmt.arms {
                    collect_task_group_async_lets(&arm.body, async_lets);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_task_group_async_lets_expr(&stmt.value, async_lets);
                collect_task_group_async_lets(&stmt.else_body, async_lets);
            }
            Stmt::Assign(stmt) => {
                collect_task_group_async_lets_expr(&stmt.target, async_lets);
                collect_task_group_async_lets_expr(&stmt.value, async_lets);
            }
            Stmt::Expr(expr) => collect_task_group_async_lets_expr(expr, async_lets),
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
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
}

pub(super) fn collect_nested_task_group_async_lets(
    block: &Block,
    async_lets: &mut Vec<crate::diagnostic::Span>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_all_async_let_spans_expr(value, async_lets);
                }
            }
            Stmt::With(stmt) => {
                collect_all_async_let_spans_expr(&stmt.resource, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::If(stmt) => {
                collect_all_async_let_spans_expr(&stmt.condition, async_lets);
                collect_all_async_let_spans(&stmt.then_body, async_lets);
                if let Some(else_body) = &stmt.else_body {
                    collect_all_async_let_spans(else_body, async_lets);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_all_async_let_spans_expr(condition, async_lets);
                }
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::For(stmt) => {
                collect_all_async_let_spans_expr(&stmt.iterable, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::Match(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                for arm in &stmt.arms {
                    collect_all_async_let_spans(&arm.body, async_lets);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                collect_all_async_let_spans(&stmt.else_body, async_lets);
            }
            Stmt::Assign(stmt) => {
                collect_all_async_let_spans_expr(&stmt.target, async_lets);
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
            }
            Stmt::Expr(expr) => collect_all_async_let_spans_expr(expr, async_lets),
            Stmt::Return(crate::syntax::ast::ReturnStmt {
                value: Some(expr), ..
            }) => {
                collect_all_async_let_spans_expr(expr, async_lets);
            }
            Stmt::Return(_)
            | Stmt::Select(_)
            | Stmt::TaskGroup(_)
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
}

fn collect_all_async_let_spans(block: &Block, async_lets: &mut Vec<crate::diagnostic::Span>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.is_async {
                    async_lets.push(stmt.span.clone());
                }
                if let Some(value) = &stmt.value {
                    collect_all_async_let_spans_expr(value, async_lets);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_all_async_let_spans_expr(value, async_lets);
                }
            }
            Stmt::With(stmt) => {
                collect_all_async_let_spans_expr(&stmt.resource, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::If(stmt) => {
                collect_all_async_let_spans_expr(&stmt.condition, async_lets);
                collect_all_async_let_spans(&stmt.then_body, async_lets);
                if let Some(else_body) = &stmt.else_body {
                    collect_all_async_let_spans(else_body, async_lets);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_all_async_let_spans_expr(condition, async_lets);
                }
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::For(stmt) => {
                collect_all_async_let_spans_expr(&stmt.iterable, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::Match(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                for arm in &stmt.arms {
                    collect_all_async_let_spans(&arm.body, async_lets);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                collect_all_async_let_spans(&stmt.else_body, async_lets);
            }
            Stmt::Assign(stmt) => {
                collect_all_async_let_spans_expr(&stmt.target, async_lets);
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
            }
            Stmt::Expr(expr) => collect_all_async_let_spans_expr(expr, async_lets),
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    collect_all_async_let_spans_expr(&arm.operation, async_lets);
                    collect_all_async_let_spans(&arm.body, async_lets);
                }
            }
            Stmt::TaskGroup(_)
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
}

/// A directly-nested child of an expression: either another expression
/// (including the operand of `Await`/`Effect`/`Spawn`/etc.) or a block (closure
/// bodies and match-arm bodies).
enum AsyncExprChild<'a> {
    Expr(&'a Expr),
    Block(&'a Block),
}

/// Shared structural descent over an expression's children, in the canonical
/// order used by every async-let collector. `visit` is invoked once per direct
/// child and is responsible for its own recursion.
fn walk_async_expr_children<F>(expr: &Expr, mut visit: F)
where
    F: FnMut(AsyncExprChild<'_>),
{
    match expr {
        Expr::Binary { left, right, .. } => {
            visit(AsyncExprChild::Expr(left));
            visit(AsyncExprChild::Expr(right));
        }
        Expr::Field { base, .. } => visit(AsyncExprChild::Expr(base)),
        Expr::Index { base, index, .. } => {
            visit(AsyncExprChild::Expr(base));
            visit(AsyncExprChild::Expr(index));
        }
        Expr::Call { args, .. } => {
            for arg in args {
                visit(AsyncExprChild::Expr(&arg.value));
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => visit(AsyncExprChild::Expr(value)),
        Expr::Closure { body, .. } => visit(AsyncExprChild::Block(body)),
        Expr::Match { value, arms, .. } => {
            visit(AsyncExprChild::Expr(value));
            for arm in arms {
                visit(AsyncExprChild::Block(&arm.body));
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                visit(AsyncExprChild::Expr(&field.value));
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                visit(AsyncExprChild::Expr(&entry.key));
                visit(AsyncExprChild::Expr(&entry.value));
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                visit(AsyncExprChild::Expr(item));
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

fn collect_all_async_let_spans_expr(expr: &Expr, async_lets: &mut Vec<crate::diagnostic::Span>) {
    walk_async_expr_children(expr, |child| match child {
        AsyncExprChild::Expr(child) => collect_all_async_let_spans_expr(child, async_lets),
        AsyncExprChild::Block(block) => collect_all_async_let_spans(block, async_lets),
    });
}

fn collect_task_group_async_lets_expr(
    expr: &Expr,
    async_lets: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    walk_async_expr_children(expr, |child| match child {
        AsyncExprChild::Expr(child) => collect_task_group_async_lets_expr(child, async_lets),
        AsyncExprChild::Block(block) => collect_task_group_async_lets(block, async_lets),
    });
}

pub(super) fn collect_task_group_awaited_handles(block: &Block, awaited: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_awaited_handles_expr(value, awaited);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_awaited_handles_expr(value, awaited);
                }
            }
            Stmt::With(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.resource, awaited);
                collect_task_group_awaited_handles(&stmt.body, awaited);
            }
            Stmt::If(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.condition, awaited);
                collect_task_group_awaited_handles(&stmt.then_body, awaited);
                if let Some(else_body) = &stmt.else_body {
                    collect_task_group_awaited_handles(else_body, awaited);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_task_group_awaited_handles_expr(condition, awaited);
                }
                collect_task_group_awaited_handles(&stmt.body, awaited);
            }
            Stmt::For(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.iterable, awaited);
                collect_task_group_awaited_handles(&stmt.body, awaited);
            }
            Stmt::Match(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
                for arm in &stmt.arms {
                    collect_task_group_awaited_handles(&arm.body, awaited);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
                collect_task_group_awaited_handles(&stmt.else_body, awaited);
            }
            Stmt::Assign(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.target, awaited);
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
            }
            Stmt::Expr(expr) => collect_task_group_awaited_handles_expr(expr, awaited),
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
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
}

pub(super) fn collect_direct_task_group_awaited_handles(
    block: &Block,
    awaited: &mut HashSet<String>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_awaited_handles_expr(value, awaited);
                }
            }
            Stmt::Assign(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.target, awaited);
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
            }
            Stmt::Expr(expr) => collect_task_group_awaited_handles_expr(expr, awaited),
            _ => {}
        }
    }
}

pub(super) fn direct_task_group_awaited_handles_in_stmt(
    statement: &Stmt,
) -> Vec<(String, crate::diagnostic::Span)> {
    let mut awaited = Vec::new();
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                direct_task_group_awaited_handles_in_expr(value, &mut awaited);
            }
        }
        Stmt::Assign(stmt) => {
            direct_task_group_awaited_handles_in_expr(&stmt.target, &mut awaited);
            direct_task_group_awaited_handles_in_expr(&stmt.value, &mut awaited);
        }
        Stmt::Expr(expr) => direct_task_group_awaited_handles_in_expr(expr, &mut awaited),
        _ => {}
    }
    awaited
}

fn direct_task_group_awaited_handles_in_expr(
    expr: &Expr,
    awaited: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    match expr {
        Expr::Await { value, span } => {
            if let Some(name) = await_handle_name(value) {
                awaited.push((name.to_string(), span.clone()));
            }
        }
        Expr::Try { value, .. } => direct_task_group_awaited_handles_in_expr(value, awaited),
        _ => {}
    }
}

pub(super) fn find_nested_task_group_await_span<'a>(
    block: &'a Block,
    name: &str,
) -> Option<&'a crate::diagnostic::Span> {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::With(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.resource, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::If(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.condition, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.then_body, name) {
                    return Some(span);
                }
                if let Some(else_body) = &stmt.else_body
                    && let Some(span) = find_task_group_await_span(else_body, name)
                {
                    return Some(span);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition
                    && let Some(span) = find_nested_task_group_await_span_expr(condition, name)
                {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::For(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.iterable, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::Match(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                for arm in &stmt.arms {
                    if let Some(span) = find_task_group_await_span(&arm.body, name) {
                        return Some(span);
                    }
                }
            }
            Stmt::LetElse(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.else_body, name) {
                    return Some(span);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::Assign(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.target, name)
                    .or_else(|| find_nested_task_group_await_span_expr(&stmt.value, name))
                {
                    return Some(span);
                }
            }
            Stmt::Expr(expr) => {
                if let Some(span) = find_nested_task_group_await_span_expr(expr, name) {
                    return Some(span);
                }
            }
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
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
    None
}

fn find_task_group_await_span<'a>(
    block: &'a Block,
    name: &str,
) -> Option<&'a crate::diagnostic::Span> {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::With(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.resource, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::If(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.condition, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.then_body, name) {
                    return Some(span);
                }
                if let Some(else_body) = &stmt.else_body
                    && let Some(span) = find_task_group_await_span(else_body, name)
                {
                    return Some(span);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition
                    && let Some(span) = find_nested_task_group_await_span_expr(condition, name)
                {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::For(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.iterable, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::Match(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                for arm in &stmt.arms {
                    if let Some(span) = find_task_group_await_span(&arm.body, name) {
                        return Some(span);
                    }
                }
            }
            Stmt::LetElse(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.else_body, name) {
                    return Some(span);
                }
            }
            Stmt::Assign(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.target, name)
                    .or_else(|| find_nested_task_group_await_span_expr(&stmt.value, name))
                {
                    return Some(span);
                }
            }
            Stmt::Expr(expr) => {
                if let Some(span) = find_nested_task_group_await_span_expr(expr, name) {
                    return Some(span);
                }
            }
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
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
    None
}

fn find_nested_task_group_await_span_expr<'a>(
    expr: &'a Expr,
    name: &str,
) -> Option<&'a crate::diagnostic::Span> {
    match expr {
        Expr::Await { value, span } => {
            if await_handle_name(value).is_some_and(|handle| handle == name) {
                return Some(span);
            }
            find_nested_task_group_await_span_expr(value, name)
        }
        Expr::Binary { left, right, .. } => find_nested_task_group_await_span_expr(left, name)
            .or_else(|| find_nested_task_group_await_span_expr(right, name)),
        Expr::Field { base, .. } => find_nested_task_group_await_span_expr(base, name),
        Expr::Index { base, index, .. } => find_nested_task_group_await_span_expr(base, name)
            .or_else(|| find_nested_task_group_await_span_expr(index, name)),
        Expr::Call { args, .. } => args
            .iter()
            .find_map(|arg| find_nested_task_group_await_span_expr(&arg.value, name)),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => find_nested_task_group_await_span_expr(value, name),
        Expr::Closure { body, .. } => find_task_group_await_span(body, name),
        Expr::Match { value, arms, .. } => find_nested_task_group_await_span_expr(value, name)
            .or_else(|| {
                arms.iter()
                    .find_map(|arm| find_task_group_await_span(&arm.body, name))
            }),
        Expr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| find_nested_task_group_await_span_expr(&field.value, name)),
        Expr::MapLiteral { entries, .. } => entries.iter().find_map(|entry| {
            find_nested_task_group_await_span_expr(&entry.key, name)
                .or_else(|| find_nested_task_group_await_span_expr(&entry.value, name))
        }),
        Expr::ArrayLiteral { items, .. } => items
            .iter()
            .find_map(|item| find_nested_task_group_await_span_expr(item, name)),
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}

fn collect_task_group_awaited_handles_expr(expr: &Expr, awaited: &mut HashSet<String>) {
    // Record the awaited handle name before descending into the operand; the
    // structural descent itself (including into the `Await` operand) is shared.
    if let Expr::Await { value, .. } = expr
        && let Some(name) = await_handle_name(value)
    {
        awaited.insert(name.to_string());
    }
    walk_async_expr_children(expr, |child| match child {
        AsyncExprChild::Expr(child) => collect_task_group_awaited_handles_expr(child, awaited),
        AsyncExprChild::Block(block) => collect_task_group_awaited_handles(block, awaited),
    });
}

fn await_handle_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => await_handle_name(value),
        _ => None,
    }
}
