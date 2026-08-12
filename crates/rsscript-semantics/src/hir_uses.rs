//! Backend-neutral HIR identifier-use queries.

use crate::hir::{HirBlock, HirExpr, HirStmt, assign_target_reads};
use rsscript_syntax::Span;

/// Return the identifier reads reachable from a HIR block in source order.
/// This deliberately follows nested statement bodies but leaves a `match`
/// expression's arms to its enclosing statement traversal, matching the HIR
/// control-flow boundary.
pub fn hir_block_identifier_uses(block: &HirBlock) -> Vec<(String, Span)> {
    let mut uses = Vec::new();
    collect_block_identifier_uses(block, &mut uses);
    uses
}

/// Return the identifier reads directly represented by one statement.
pub fn hir_stmt_identifier_uses(statement: &HirStmt) -> Vec<(String, Span)> {
    let mut uses = Vec::new();
    collect_stmt_identifier_uses(statement, &mut uses);
    uses
}

fn collect_block_identifier_uses(block: &HirBlock, uses: &mut Vec<(String, Span)>) {
    for statement in &block.statements {
        collect_stmt_identifier_uses(statement, uses);
        match statement {
            HirStmt::With { body, .. } => collect_block_identifier_uses(body, uses),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_block_identifier_uses(then_body, uses);
                if let Some(else_body) = else_body {
                    collect_block_identifier_uses(else_body, uses);
                }
            }
            HirStmt::Loop { body, .. } => collect_block_identifier_uses(body, uses),
            HirStmt::For { body, .. } => collect_block_identifier_uses(body, uses),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_block_identifier_uses(&arm.body, uses);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_block_identifier_uses(&arm.body, uses);
                }
            }
            HirStmt::Let { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Assign { .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_stmt_identifier_uses(statement: &HirStmt, uses: &mut Vec<(String, Span)>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_identifier_uses(value, uses),
        HirStmt::Assign { target, value, .. } => {
            for read in assign_target_reads(target) {
                collect_expr_identifier_uses(read, uses);
            }
            collect_expr_identifier_uses(value, uses);
        }
        HirStmt::With { resource, .. } => collect_expr_identifier_uses(resource, uses),
        HirStmt::If { condition, .. } => collect_expr_identifier_uses(condition, uses),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_expr_identifier_uses(condition, uses),
        HirStmt::For { iterable, .. } => collect_expr_identifier_uses(iterable, uses),
        HirStmt::Match { value, .. } => collect_expr_identifier_uses(value, uses),
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_identifier_uses(&arm.operation, uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Loop {
            condition: None, ..
        }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_identifier_uses(expr: &HirExpr, uses: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => uses.push((name.clone(), span.clone())),
        HirExpr::Field { base, .. } => collect_expr_identifier_uses(base, uses),
        HirExpr::Index { base, index, .. } => {
            collect_expr_identifier_uses(base, uses);
            collect_expr_identifier_uses(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_identifier_uses(&arg.value, uses);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_identifier_uses(&field.value, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_identifier_uses(&entry.key, uses);
                collect_expr_identifier_uses(&entry.value, uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_identifier_uses(item, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_identifier_uses(value, uses),
        HirExpr::Binary { left, right, .. } => {
            collect_expr_identifier_uses(left, uses);
            collect_expr_identifier_uses(right, uses);
        }
        HirExpr::Closure { body, .. } => collect_block_identifier_uses(body, uses),
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Match { .. }
        | HirExpr::Unknown(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::Hir;
    use rsscript_syntax::parse_source;

    #[test]
    fn collects_nested_statement_and_assignment_reads_in_source_order() {
        let program = parse_source(
            "uses.rss",
            r#"
fn main(a: Int, b: Int, c: Int) -> Unit {
    let x = a
    if b { x = c }
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let block = hir
            .function_body("main")
            .and_then(|body| body.block.as_ref())
            .unwrap();

        assert_eq!(
            hir_block_identifier_uses(block)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }
}
