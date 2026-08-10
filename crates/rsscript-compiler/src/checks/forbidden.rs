use crate::analyzer::Analyzer;
use crate::checks::shared::builtin_value_type_name;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{HirBlock, HirExpr, HirStmt};
use crate::syntax::ast::{BinaryOp, FunctionDecl, Item};
use crate::text_util::type_root_name;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::forbidden_surface_syntax_diagnostics(
            analyzer.tokens,
        ));
    check_operator_overload_attempts(analyzer);
}

fn check_operator_overload_attempts(analyzer: &mut Analyzer<'_>) {
    let functions = analyzer
        .syntax_program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function.clone()),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect::<Vec<FunctionDecl>>();

    for function in functions {
        if let Some(block) = analyzer
            .hir
            .function_body(&function.name)
            .and_then(|body| body.block.clone())
        {
            check_operator_overload_attempts_in_block(analyzer, &block);
        }
    }
}

fn check_operator_overload_attempts_in_block(analyzer: &mut Analyzer<'_>, block: &HirBlock) {
    for statement in &block.statements {
        check_operator_overload_attempts_in_stmt(analyzer, statement);
    }
}

fn check_operator_overload_attempts_in_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_operator_overload_attempts_in_expr(analyzer, value);
            }
        }
        HirStmt::With { resource, body, .. } => {
            check_operator_overload_attempts_in_expr(analyzer, resource);
            check_operator_overload_attempts_in_block(analyzer, body);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_operator_overload_attempts_in_expr(analyzer, condition);
            check_operator_overload_attempts_in_block(analyzer, then_body);
            if let Some(else_body) = else_body {
                check_operator_overload_attempts_in_block(analyzer, else_body);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_operator_overload_attempts_in_expr(analyzer, condition);
            }
            check_operator_overload_attempts_in_block(analyzer, body);
        }
        HirStmt::For { iterable, body, .. } => {
            check_operator_overload_attempts_in_expr(analyzer, iterable);
            check_operator_overload_attempts_in_block(analyzer, body);
        }
        HirStmt::Match { value, arms, .. } => {
            check_operator_overload_attempts_in_expr(analyzer, value);
            for arm in arms {
                check_operator_overload_attempts_in_block(analyzer, &arm.body);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_operator_overload_attempts_in_expr(analyzer, &arm.operation);
                check_operator_overload_attempts_in_block(analyzer, &arm.body);
            }
        }
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            check_operator_overload_attempts_in_expr(analyzer, value)
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn check_operator_overload_attempts_in_expr(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    match expr {
        HirExpr::Binary {
            op,
            left,
            right,
            span,
        } => {
            check_operator_overload_attempts_in_expr(analyzer, left);
            check_operator_overload_attempts_in_expr(analyzer, right);
            if arithmetic_operator(*op)
                && (non_numeric_operand(analyzer, left) || non_numeric_operand(analyzer, right))
            {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::OPERATOR_OVERLOAD_ATTEMPT,
                        "arithmetic operators are only built in for numeric values.",
                        span.clone(),
                        "operator on non-numeric value",
                    )
                    .with_cause("RSScript does not support user-defined operator overloads.")
                    .with_fix(
                        "use_named_function",
                        "Use a named function such as `Type.add(left: read a, right: read b)`.",
                        "manual",
                    ),
                );
            }
            check_builtin_operator_operand_types(analyzer, *op, left, right, span);
        }
        HirExpr::Field { base, .. } => check_operator_overload_attempts_in_expr(analyzer, base),
        HirExpr::Index { base, index, .. } => {
            check_operator_overload_attempts_in_expr(analyzer, base);
            check_operator_overload_attempts_in_expr(analyzer, index);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_operator_overload_attempts_in_expr(analyzer, &arg.value);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => check_operator_overload_attempts_in_expr(analyzer, value),
        HirExpr::Closure { body, .. } => check_operator_overload_attempts_in_block(analyzer, body),
        HirExpr::Match { value, arms, .. } => {
            check_operator_overload_attempts_in_expr(analyzer, value);
            for arm in arms {
                check_operator_overload_attempts_in_block(analyzer, &arm.body);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_operator_overload_attempts_in_expr(analyzer, &entry.key);
                check_operator_overload_attempts_in_expr(analyzer, &entry.value);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

/// Infers the type names of both operands, returning `None` if either operand's
/// type is unknown. The operand-type checks below only fire when both types are
/// known, so this consolidates the repeated "bail unless both are known" guard.
fn inferred_operand_types(
    analyzer: &Analyzer<'_>,
    left: &HirExpr,
    right: &HirExpr,
) -> Option<(String, String)> {
    let left_type = inferred_operand_type(analyzer, left)?;
    let right_type = inferred_operand_type(analyzer, right)?;
    Some((left_type, right_type))
}

fn check_builtin_operator_operand_types(
    analyzer: &mut Analyzer<'_>,
    op: BinaryOp,
    left: &HirExpr,
    right: &HirExpr,
    span: &crate::diagnostic::Span,
) {
    match op {
        BinaryOp::Equal | BinaryOp::NotEqual => {
            let Some((left_type, right_type)) = inferred_operand_types(analyzer, left, right)
            else {
                return;
            };
            if type_root_name(&left_type) != type_root_name(&right_type) {
                operator_type_mismatch_diagnostic(
                    analyzer,
                    span.clone(),
                    operator_label(op),
                    &left_type,
                    &right_type,
                    "matching operand types",
                );
            }
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
            let Some((left_type, right_type)) = inferred_operand_types(analyzer, left, right)
            else {
                return;
            };
            if !is_numeric_type(&left_type) || !is_numeric_type(&right_type) {
                operator_type_mismatch_diagnostic(
                    analyzer,
                    span.clone(),
                    operator_label(op),
                    &left_type,
                    &right_type,
                    "numeric operands",
                );
            } else if type_root_name(&left_type) != type_root_name(&right_type) {
                // Both numeric but different roots (e.g. `Float < Int`): the backend
                // rejects the mixed comparison, so reject it here too.
                operator_type_mismatch_diagnostic(
                    analyzer,
                    span.clone(),
                    operator_label(op),
                    &left_type,
                    &right_type,
                    "matching operand types",
                );
            }
        }
        BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
            let Some((left_type, right_type)) = inferred_operand_types(analyzer, left, right)
            else {
                return;
            };
            if type_root_name(&left_type) != "Bool" || type_root_name(&right_type) != "Bool" {
                operator_type_mismatch_diagnostic(
                    analyzer,
                    span.clone(),
                    operator_label(op),
                    &left_type,
                    &right_type,
                    "Bool operands",
                );
            }
        }
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo => {
            // Non-numeric operands are already rejected by the operator-overload
            // check; here catch mixed numeric roots (e.g. `Float + Int`), which
            // otherwise pass `check` and then fail the backend with E0277/E0308.
            let Some((left_type, right_type)) = inferred_operand_types(analyzer, left, right)
            else {
                return;
            };
            if is_numeric_type(&left_type)
                && is_numeric_type(&right_type)
                && type_root_name(&left_type) != type_root_name(&right_type)
            {
                operator_type_mismatch_diagnostic(
                    analyzer,
                    span.clone(),
                    operator_label(op),
                    &left_type,
                    &right_type,
                    "matching operand types",
                );
            }
        }
        BinaryOp::BitAnd
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight => {
            let Some((left_type, right_type)) = inferred_operand_types(analyzer, left, right)
            else {
                return;
            };
            if type_root_name(&left_type) != "Int" || type_root_name(&right_type) != "Int" {
                operator_type_mismatch_diagnostic(
                    analyzer,
                    span.clone(),
                    operator_label(op),
                    &left_type,
                    &right_type,
                    "Int operands",
                );
            }
        }
    }
}

fn operator_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    operator: &str,
    left_type: &str,
    right_type: &str,
    expected: &str,
) {
    analyzer.diagnostics.push(
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
        ),
    );
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

fn non_numeric_operand(analyzer: &Analyzer<'_>, expr: &HirExpr) -> bool {
    inferred_operand_type(analyzer, expr).is_some_and(|type_name| !is_numeric_type(&type_name))
}

fn inferred_operand_type(analyzer: &Analyzer<'_>, expr: &HirExpr) -> Option<String> {
    let type_name = match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| builtin_value_type_name(name))
            .or_else(|| {
                analyzer
                    .hir
                    .type_info(name)
                    .map(|type_info| type_info.name.as_str())
            }),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
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
    Some(analyzer.expand_type_alias(type_name))
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
