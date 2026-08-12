//! Canonical diagnostics and checked-HIR validation for builtin operators.

use crate::hir::{Hir, HirBlock, HirExpr, HirStmt, number_literal_type_name};
use crate::type_root_name;
use rsscript_diagnostics::{Diagnostic, Span, code};
use rsscript_syntax::ast::BinaryOp;

/// Derive every builtin-operator diagnostic from checked HIR.
///
/// The compiler supplies the resolved HIR only; this query owns recursive
/// traversal, alias normalization, numeric classification, and diagnostics.
pub fn builtin_operator_diagnostics(hir: &Hir) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (_, body) in hir.function_bodies() {
        if let Some(block) = &body.block {
            collect_block_operator_diagnostics(hir, block, &mut diagnostics);
        }
    }
    diagnostics
}

fn collect_block_operator_diagnostics(
    hir: &Hir,
    block: &HirBlock,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        collect_stmt_operator_diagnostics(hir, statement, diagnostics);
    }
}

fn collect_stmt_operator_diagnostics(
    hir: &Hir,
    statement: &HirStmt,
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
            collect_expr_operator_diagnostics(hir, value, diagnostics)
        }
        HirStmt::With { resource, body, .. } => {
            collect_expr_operator_diagnostics(hir, resource, diagnostics);
            collect_block_operator_diagnostics(hir, body, diagnostics);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_operator_diagnostics(hir, condition, diagnostics);
            collect_block_operator_diagnostics(hir, then_body, diagnostics);
            if let Some(else_body) = else_body {
                collect_block_operator_diagnostics(hir, else_body, diagnostics);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_operator_diagnostics(hir, condition, diagnostics);
            }
            collect_block_operator_diagnostics(hir, body, diagnostics);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_operator_diagnostics(hir, iterable, diagnostics);
            collect_block_operator_diagnostics(hir, body, diagnostics);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_operator_diagnostics(hir, value, diagnostics);
            for arm in arms {
                collect_block_operator_diagnostics(hir, &arm.body, diagnostics);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_operator_diagnostics(hir, &arm.operation, diagnostics);
                collect_block_operator_diagnostics(hir, &arm.body, diagnostics);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_operator_diagnostics(hir: &Hir, expr: &HirExpr, diagnostics: &mut Vec<Diagnostic>) {
    match expr {
        HirExpr::Binary {
            op,
            left,
            right,
            span,
        } => {
            collect_expr_operator_diagnostics(hir, left, diagnostics);
            collect_expr_operator_diagnostics(hir, right, diagnostics);
            let Some((left_type, right_type)) = operand_types(hir, left, right) else {
                return;
            };
            if arithmetic_operator(*op)
                && (!is_numeric_type(&left_type) || !is_numeric_type(&right_type))
            {
                diagnostics.push(operator_overload_attempt_diagnostic(span.clone()));
            }
            if let Some(expected) = incompatible_operator_operands(*op, &left_type, &right_type) {
                diagnostics.push(operator_type_mismatch_diagnostic(
                    operator_label(*op),
                    &left_type,
                    &right_type,
                    expected,
                    span.clone(),
                ));
            }
        }
        HirExpr::Field { base, .. } => collect_expr_operator_diagnostics(hir, base, diagnostics),
        HirExpr::Index { base, index, .. } => {
            collect_expr_operator_diagnostics(hir, base, diagnostics);
            collect_expr_operator_diagnostics(hir, index, diagnostics);
        }
        HirExpr::Call { args, .. } => {
            for argument in args {
                collect_expr_operator_diagnostics(hir, &argument.value, diagnostics);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_operator_diagnostics(hir, value, diagnostics),
        HirExpr::Closure { body, .. } => collect_block_operator_diagnostics(hir, body, diagnostics),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_operator_diagnostics(hir, value, diagnostics);
            for arm in arms {
                collect_block_operator_diagnostics(hir, &arm.body, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_operator_diagnostics(hir, &entry.key, diagnostics);
                collect_expr_operator_diagnostics(hir, &entry.value, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_operator_diagnostics(hir, &field.value, diagnostics);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_operator_diagnostics(hir, item, diagnostics);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn operand_types(hir: &Hir, left: &HirExpr, right: &HirExpr) -> Option<(String, String)> {
    Some((operand_type(hir, left)?, operand_type(hir, right)?))
}

fn operand_type(hir: &Hir, expr: &HirExpr) -> Option<String> {
    let type_name = match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| builtin_value_type_name(name))
            .or_else(|| hir.type_info(name).map(|info| info.name.as_str())),
        HirExpr::Number { value, .. } => Some(number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Char { .. } => Some("Char"),
        HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. }
        | HirExpr::Index { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }?;
    Some(hir.canonical_type_name(type_name))
}

fn incompatible_operator_operands(
    op: BinaryOp,
    left_type: &str,
    right_type: &str,
) -> Option<&'static str> {
    match op {
        BinaryOp::Equal | BinaryOp::NotEqual
            if type_root_name(left_type) != type_root_name(right_type) =>
        {
            Some("matching operand types")
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
            if !is_numeric_type(left_type) || !is_numeric_type(right_type) =>
        {
            Some("numeric operands")
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
            if type_root_name(left_type) != type_root_name(right_type) =>
        {
            Some("matching operand types")
        }
        BinaryOp::LogicalAnd | BinaryOp::LogicalOr
            if type_root_name(left_type) != "Bool" || type_root_name(right_type) != "Bool" =>
        {
            Some("Bool operands")
        }
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo
            if is_numeric_type(left_type)
                && is_numeric_type(right_type)
                && type_root_name(left_type) != type_root_name(right_type) =>
        {
            Some("matching operand types")
        }
        BinaryOp::BitAnd
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight
            if type_root_name(left_type) != "Int" || type_root_name(right_type) != "Int" =>
        {
            Some("Int operands")
        }
        _ => None,
    }
}

fn operator_label(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Modulo => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight => ">>",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::LogicalAnd => "&&",
        BinaryOp::LogicalOr => "||",
    }
}

fn arithmetic_operator(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::ShiftLeft
            | BinaryOp::ShiftRight
    )
}

fn is_numeric_type(type_name: &str) -> bool {
    matches!(
        type_root_name(type_name),
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float"
            | "Float32"
            | "Float64"
    )
}

fn builtin_value_type_name(name: &str) -> Option<&'static str> {
    match name {
        "true" | "false" => Some("Bool"),
        "null" => Some("JsonLiteral"),
        "Unit" => Some("Unit"),
        "None" => Some("Option<?>"),
        _ => None,
    }
}

/// Diagnose an arithmetic operation on a resolved non-numeric value.
pub fn operator_overload_attempt_diagnostic(span: Span) -> Diagnostic {
    Diagnostic::error(
        code::OPERATOR_OVERLOAD_ATTEMPT,
        "arithmetic operators are only built in for numeric values.",
        span,
        "operator on non-numeric value",
    )
    .with_cause("RSScript does not support user-defined operator overloads.")
    .with_fix(
        "use_named_function",
        "Use a named function such as `Type.add(left: read a, right: read b)`.",
        "manual",
    )
}

/// Diagnose incompatible resolved builtin operator operand types.
pub fn operator_type_mismatch_diagnostic(
    operator: &str,
    left_type: &str,
    right_type: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::OPERATOR_TYPE_MISMATCH,
        format!(
            "operator `{operator}` has operands `{left_type}` and `{right_type}`, expected {expected}."
        ),
        span,
        "operator type mismatch",
    )
    .with_cause("RSScript operators have fixed built-in operand types and do not use implicit conversion or overload resolution.")
    .with_fix(
        "use_typed_operator_operands",
        "Compare values of the same supported type, or call an explicit named conversion/function first.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "operators.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn preserves_builtin_operator_diagnostic_contracts() {
        assert_eq!(
            operator_overload_attempt_diagnostic(span()).code,
            code::OPERATOR_OVERLOAD_ATTEMPT
        );
        assert_eq!(
            operator_type_mismatch_diagnostic("+", "Int", "Float", "matching operands", span())
                .code,
            code::OPERATOR_TYPE_MISMATCH
        );
    }
}
