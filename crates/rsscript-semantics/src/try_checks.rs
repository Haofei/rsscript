//! Checked-HIR diagnostics for the `?` operator.

use crate::hir::{HirBlock, HirExpr, HirStmt};
use rsscript_diagnostics::{Diagnostic, Span, code};

/// Diagnose applying `?` to a known non-`Result`/`Option` operand type.
pub fn try_operand_diagnostic(type_name: Option<&str>, span: &Span) -> Option<Diagnostic> {
    let type_name = type_name?;
    if is_result_type(type_name) || is_option_type(type_name) {
        return None;
    }
    Some(
        Diagnostic::error(
            code::INVALID_TRY_OPERATOR,
            "`?` can only be applied to a `Result` or `Option` value.",
            span.clone(),
            "invalid try operator",
        )
        .with_cause(format!(
            "The expression before `?` has type `{type_name}`, not `Result<T, E>` or `Option<T>`."
        ))
        .with_fix(
            "remove_try_or_return_result",
            "Remove `?`, or call an API that returns `Result<T, E>` or `Option<T>`.",
            "manual",
        ),
    )
}

fn is_result_type(type_name: &str) -> bool {
    type_name == "Result" || type_name.starts_with("Result<")
}

fn is_option_type(type_name: &str) -> bool {
    type_name == "Option" || type_name.starts_with("Option<")
}

/// Diagnose `Result` error-type mismatches introduced by `?` in a function.
pub fn try_error_type_diagnostics(
    block: &HirBlock,
    function_error_type: Option<&str>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_block(block, function_error_type, &mut diagnostics);
    diagnostics
}

fn collect_block(
    block: &HirBlock,
    function_error_type: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        collect_statement(statement, function_error_type, diagnostics);
    }
}

fn collect_statement(
    statement: &HirStmt,
    function_error_type: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_expression(value, function_error_type, diagnostics)
        }
        HirStmt::With { resource, body, .. } => {
            collect_expression(resource, function_error_type, diagnostics);
            collect_block(body, function_error_type, diagnostics);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expression(condition, function_error_type, diagnostics);
            collect_block(then_body, function_error_type, diagnostics);
            if let Some(else_body) = else_body {
                collect_block(else_body, function_error_type, diagnostics);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expression(condition, function_error_type, diagnostics);
            }
            collect_block(body, function_error_type, diagnostics);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expression(iterable, function_error_type, diagnostics);
            collect_block(body, function_error_type, diagnostics);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expression(value, function_error_type, diagnostics);
            for arm in arms {
                collect_block(&arm.body, function_error_type, diagnostics);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expression(&arm.operation, function_error_type, diagnostics);
                collect_block(&arm.body, function_error_type, diagnostics);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expression(
    expr: &HirExpr,
    function_error_type: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        HirExpr::Try { value, span, .. } => {
            if let (Some(function_error_type), Some(operand_type)) =
                (function_error_type, hir_expr_type_name(value))
                && let Some(operand_error_type) = result_error_type_name(operand_type)
                && operand_error_type != function_error_type
            {
                diagnostics.push(try_error_type_mismatch_diagnostic(
                    span,
                    operand_error_type,
                    function_error_type,
                ));
            }
            collect_expression(value, function_error_type, diagnostics);
        }
        HirExpr::Call { args, .. } => {
            for argument in args {
                collect_expression(&argument.value, function_error_type, diagnostics);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => {
            collect_expression(value, function_error_type, diagnostics)
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expression(left, function_error_type, diagnostics);
            collect_expression(right, function_error_type, diagnostics);
        }
        HirExpr::Field { base, .. } => collect_expression(base, function_error_type, diagnostics),
        HirExpr::Index { base, index, .. } => {
            collect_expression(base, function_error_type, diagnostics);
            collect_expression(index, function_error_type, diagnostics);
        }
        HirExpr::Closure { body, .. } => collect_block(body, function_error_type, diagnostics),
        HirExpr::Match { value, arms, .. } => {
            collect_expression(value, function_error_type, diagnostics);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expression(guard, function_error_type, diagnostics);
                }
                collect_block(&arm.body, function_error_type, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expression(&entry.key, function_error_type, diagnostics);
                collect_expression(&entry.value, function_error_type, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expression(&field.value, function_error_type, diagnostics);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expression(item, function_error_type, diagnostics);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { type_name, .. }
        | HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(if value.contains('.') { "Float" } else { "Int" }),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Char { .. } => Some("Char"),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn result_error_type_name(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).get(1).copied()
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth: usize = 0;
    for (index, character) in args.char_indices() {
        match character {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if start < args.len() {
        parts.push(args[start..].trim());
    }
    parts
}

fn try_error_type_mismatch_diagnostic(
    span: &Span,
    operand_error_type: &str,
    function_error_type: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::INVALID_TRY_OPERATOR,
        "`?` error type must exactly match the function error type.",
        span.clone(),
        "mismatched try error type",
    )
    .with_cause(format!(
        "The operand returns `Result<_, {operand_error_type}>`, but the function returns `Result<_, {function_error_type}>`."
    ))
    .with_cause("RSScript does not perform implicit error conversion for `?`.")
    .with_fix(
        "map_error_explicitly",
        "Handle the error explicitly and return the function's error type.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "try.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn accepts_failure_carrying_operands_and_rejects_known_scalars() {
        assert!(try_operand_diagnostic(Some("Result<String, Error>"), &span()).is_none());
        assert!(try_operand_diagnostic(Some("Option<Int>"), &span()).is_none());
        let diagnostic = try_operand_diagnostic(Some("Int"), &span())
            .expect("a scalar cannot be unwrapped with ?");
        assert_eq!(diagnostic.code, code::INVALID_TRY_OPERATOR);
    }

    #[test]
    fn reports_result_error_type_mismatches_in_checked_hir() {
        let operand = HirExpr::Ident {
            name: "operation".to_owned(),
            type_name: Some("Result<Int, OtherError>".to_owned()),
            span: span(),
        };
        let block = HirBlock {
            statements: vec![HirStmt::Expr(HirExpr::Try {
                value: Box::new(operand),
                type_name: Some("Int".to_owned()),
                span: span(),
            })],
            span: span(),
        };

        let diagnostics = try_error_type_diagnostics(&block, Some("AppError"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::INVALID_TRY_OPERATOR);
        assert!(diagnostics[0].label.contains("mismatched try error type"));
    }
}
