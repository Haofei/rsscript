//! Backend-neutral HIR facts for consuming handle fields.

use crate::TakeHandleField;
use crate::hir::{HirBlock, HirExpr, HirStmt, ParamEffect};
use rsscript_syntax::Span;

/// Return every distinct handle field consumed with `take` in a HIR block.
///
/// This is a language fact shared by diagnostics and local-flow analysis. The
/// traversal owns nested statement and expression shapes so compiler clients
/// do not need to rediscover them.
pub fn take_handle_fields(block: &HirBlock) -> Vec<TakeHandleField> {
    let mut fields = Vec::new();
    collect_block_take_handle_fields(block, &mut fields);
    fields
}

fn collect_block_take_handle_fields(block: &HirBlock, fields: &mut Vec<TakeHandleField>) {
    for statement in &block.statements {
        collect_stmt_take_handle_fields(statement, fields);
    }
}

fn collect_stmt_take_handle_fields(statement: &HirStmt, fields: &mut Vec<TakeHandleField>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => collect_expr_take_handle_fields(value, fields),
        HirStmt::With { resource, body, .. } => {
            collect_expr_take_handle_fields(resource, fields);
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_take_handle_fields(condition, fields);
            collect_block_take_handle_fields(then_body, fields);
            if let Some(else_body) = else_body {
                collect_block_take_handle_fields(else_body, fields);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_take_handle_fields(condition, fields);
            }
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_take_handle_fields(iterable, fields);
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_take_handle_fields(value, fields);
            for arm in arms {
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_take_handle_fields(&arm.operation, fields);
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_take_handle_fields(expr: &HirExpr, fields: &mut Vec<TakeHandleField>) {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Take,
            value,
            span,
            ..
        } => {
            if let HirExpr::Field { name, access, .. } = value.as_ref()
                && access.is_handle
            {
                push_take_handle_field(fields, name.clone(), span.clone());
            }
            collect_expr_take_handle_fields(value, fields);
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_take_handle_fields(value, fields),
        HirExpr::Binary { left, right, .. } => {
            collect_expr_take_handle_fields(left, fields);
            collect_expr_take_handle_fields(right, fields);
        }
        HirExpr::Field { base, .. } => collect_expr_take_handle_fields(base, fields),
        HirExpr::Index { base, index, .. } => {
            collect_expr_take_handle_fields(base, fields);
            collect_expr_take_handle_fields(index, fields);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_take_handle_fields(&arg.value, fields);
            }
        }
        HirExpr::Closure { body, .. } => collect_block_take_handle_fields(body, fields),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_take_handle_fields(value, fields);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_take_handle_fields(guard, fields);
                }
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_take_handle_fields(&entry.key, fields);
                collect_expr_take_handle_fields(&entry.value, fields);
            }
        }
        HirExpr::ObjectLiteral { fields: values, .. } => {
            for field in values {
                collect_expr_take_handle_fields(&field.value, fields);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_take_handle_fields(item, fields);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_take_handle_field(fields: &mut Vec<TakeHandleField>, name: String, span: Span) {
    let field = TakeHandleField { name, span };
    if !fields.contains(&field) {
        fields.push(field);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::{HirFieldAccess, HirStmt};

    fn span() -> Span {
        Span {
            file: "take.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn finds_taken_handle_fields_through_nested_expression_wrappers() {
        let handle = HirExpr::Field {
            base: Box::new(HirExpr::Ident {
                name: "owner".to_owned(),
                type_name: None,
                span: span(),
            }),
            name: "child".to_owned(),
            access: HirFieldAccess {
                function_name: "main".to_owned(),
                name: "child".to_owned(),
                span: span(),
                base_ty: None,
                ty: None,
                base_type: None,
                type_name: None,
                is_handle: true,
                is_weak: false,
            },
            span: span(),
        };
        let take = HirExpr::Effect {
            effect: ParamEffect::Take,
            value: Box::new(handle),
            events: Vec::new(),
            type_name: None,
            span: span(),
        };
        let block = HirBlock {
            statements: vec![HirStmt::Expr(HirExpr::Try {
                value: Box::new(take),
                type_name: None,
                span: span(),
            })],
            span: span(),
        };

        assert_eq!(
            take_handle_fields(&block),
            vec![TakeHandleField {
                name: "child".to_owned(),
                span: span(),
            }]
        );
    }
}
