//! Semantic validation of where source `await` expressions may occur.

use crate::hir::{HirBlock, HirExpr, HirStmt, assign_target_reads};
use rsscript_diagnostics::{Diagnostic, code};

/// Diagnose `await` expressions outside an async function or structured task
/// group. Operand type and lifetime validation are separate semantic passes.
pub fn await_placement_diagnostics(block: &HirBlock, function_is_async: bool) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_block(block, function_is_async, &mut diagnostics);
    diagnostics
}

fn collect_block(block: &HirBlock, function_is_async: bool, diagnostics: &mut Vec<Diagnostic>) {
    // A task-group body is flattened into its parent block. Its async `let`
    // bindings identify the structured-concurrency boundary where awaits are
    // valid even within a synchronous enclosing function.
    let in_task_group = block
        .statements
        .iter()
        .any(|statement| matches!(statement, HirStmt::Let { is_async: true, .. }));
    let async_context = function_is_async || in_task_group;
    for statement in &block.statements {
        collect_statement(statement, async_context, diagnostics);
    }
}

fn collect_statement(
    statement: &HirStmt,
    function_is_async: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_expression(value, function_is_async, diagnostics);
            }
        }
        HirStmt::With { resource, body, .. } => {
            collect_expression(resource, function_is_async, diagnostics);
            collect_block(body, function_is_async, diagnostics);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expression(condition, function_is_async, diagnostics);
            collect_block(then_body, function_is_async, diagnostics);
            if let Some(else_body) = else_body {
                collect_block(else_body, function_is_async, diagnostics);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expression(condition, function_is_async, diagnostics);
            }
            collect_block(body, function_is_async, diagnostics);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expression(iterable, function_is_async, diagnostics);
            collect_block(body, function_is_async, diagnostics);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expression(value, function_is_async, diagnostics);
            for arm in arms {
                collect_block(&arm.body, function_is_async, diagnostics);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                // Select operations run at the structured await boundary, while
                // their bodies remain ordinary code in the enclosing context.
                collect_expression(&arm.operation, true, diagnostics);
                collect_block(&arm.body, function_is_async, diagnostics);
            }
        }
        HirStmt::Expr(value) => collect_expression(value, function_is_async, diagnostics),
        HirStmt::Assign { target, value, .. } => {
            collect_expression(value, function_is_async, diagnostics);
            for read in assign_target_reads(target) {
                collect_expression(read, function_is_async, diagnostics);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_expression(expr: &HirExpr, function_is_async: bool, diagnostics: &mut Vec<Diagnostic>) {
    match expr {
        HirExpr::Await { value, span, .. } => {
            if !function_is_async {
                diagnostics.push(
                    Diagnostic::error(
                        code::AWAIT_OUTSIDE_ASYNC,
                        "`await` is only valid inside an async function.",
                        span.clone(),
                        "await outside async fn",
                    )
                    .with_cause("Suspension points are part of the async function frame and cannot appear in ordinary synchronous functions.")
                    .with_fix("move_to_async_fn", "Move this await into an `async fn`, or call a synchronous API.", "manual"),
                );
            }
            collect_expression(value, function_is_async, diagnostics);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expression(left, function_is_async, diagnostics);
            collect_expression(right, function_is_async, diagnostics);
        }
        HirExpr::Field { base, .. } => collect_expression(base, function_is_async, diagnostics),
        HirExpr::Index { base, index, .. } => {
            collect_expression(base, function_is_async, diagnostics);
            collect_expression(index, function_is_async, diagnostics);
        }
        HirExpr::Call { args, .. } => {
            for argument in args {
                collect_expression(&argument.value, function_is_async, diagnostics);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Try { value, .. } => collect_expression(value, function_is_async, diagnostics),
        HirExpr::Closure { body, .. } => collect_block(body, false, diagnostics),
        HirExpr::Match { value, arms, .. } => {
            collect_expression(value, function_is_async, diagnostics);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expression(guard, function_is_async, diagnostics);
                }
                collect_block(&arm.body, function_is_async, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expression(&entry.key, function_is_async, diagnostics);
                collect_expression(&entry.value, function_is_async, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expression(&field.value, function_is_async, diagnostics);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expression(item, function_is_async, diagnostics);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_diagnostics::Span;

    fn span() -> Span {
        Span {
            file: "async.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    fn await_expression() -> HirExpr {
        HirExpr::Await {
            value: Box::new(HirExpr::Unknown(span())),
            type_name: None,
            span: span(),
        }
    }

    #[test]
    fn rejects_await_in_synchronous_hir_but_allows_it_in_async_hir() {
        let block = HirBlock {
            statements: vec![HirStmt::Expr(await_expression())],
            span: span(),
        };

        let diagnostics = await_placement_diagnostics(&block, false);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::AWAIT_OUTSIDE_ASYNC);
        assert!(await_placement_diagnostics(&block, true).is_empty());
    }
}
