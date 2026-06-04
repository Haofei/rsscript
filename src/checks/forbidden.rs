use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{HirBlock, HirExpr, HirStmt};
use crate::syntax::ast::{BinaryOp, FunctionDecl, Item};

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    check_operator_overload_attempts(analyzer);
    check_implicit_conversion_attempts(analyzer);
    check_own_struct_attempts(analyzer);
    check_surface_reference_attempts(analyzer);
}

fn check_own_struct_attempts(analyzer: &mut Analyzer<'_>) {
    for index in 0..analyzer.tokens.len().saturating_sub(1) {
        if analyzer.tokens[index].is_ident_text("own")
            && analyzer.tokens[index + 1].is_ident_text("struct")
        {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::OWN_STRUCT_ATTEMPT,
                    "`own struct` is not part of RSScript v0.6.",
                    analyzer.tokens[index].span.clone(),
                    "own struct attempt",
                )
                .with_cause("v0.6 has only `class`, `struct`, and `resource` type declarations.")
                .with_fix(
                    "choose_type_kind",
                    "Use `struct` for inline values, `class` for managed identity, or `resource` for deterministic cleanup.",
                    "manual",
                ),
            );
        }
    }
}

fn check_surface_reference_attempts(analyzer: &mut Analyzer<'_>) {
    for index in 0..analyzer.tokens.len() {
        if !analyzer.tokens[index].symbol("&") || is_boolean_and(analyzer, index) {
            continue;
        }
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::SURFACE_REFERENCE_ATTEMPT,
                "surface reference syntax is not part of RSScript.",
                analyzer.tokens[index].span.clone(),
                "surface reference attempt",
            )
            .with_cause("RSScript uses explicit data effects instead of `&T` or `&mut T` syntax.")
            .with_fix(
                "use_data_effect",
                "Use a parameter effect such as `value: read T` or `value: mut T`.",
                "manual",
            ),
        );
    }
}

fn is_boolean_and(analyzer: &Analyzer<'_>, index: usize) -> bool {
    analyzer
        .tokens
        .get(index.wrapping_sub(1))
        .is_some_and(|token| token.symbol("&"))
        || analyzer
            .tokens
            .get(index + 1)
            .is_some_and(|token| token.symbol("&"))
}

fn check_implicit_conversion_attempts(analyzer: &mut Analyzer<'_>) {
    for index in 0..analyzer.tokens.len() {
        if !analyzer.tokens[index].is_ident_text("as") || as_belongs_to_with(analyzer, index) {
            continue;
        }
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::IMPLICIT_CONVERSION_ATTEMPT,
                "cast-style conversions are not part of RSScript.",
                analyzer.tokens[index].span.clone(),
                "implicit conversion attempt",
            )
            .with_cause("RSScript requires conversions to be explicit named APIs so review tools can see them.")
            .with_fix(
                "use_named_conversion",
                "Use a named conversion such as `Target.from(value: read source)`.",
                "manual",
            ),
        );
    }
}

fn as_belongs_to_with(analyzer: &Analyzer<'_>, as_index: usize) -> bool {
    for token in analyzer.tokens[..as_index].iter().rev() {
        if token.is_ident_text("with") {
            return true;
        }
        if token.symbol("{")
            || token.symbol("}")
            || token.is_ident_text("let")
            || token.is_ident_text("local")
            || token.is_ident_text("return")
            || token.is_ident_text("fn")
            || token.is_ident_text("class")
            || token.is_ident_text("struct")
            || token.is_ident_text("resource")
            || token.is_ident_text("if")
            || token.is_ident_text("else")
            || token.is_ident_text("loop")
            || token.is_ident_text("while")
            || token.is_ident_text("break")
            || token.is_ident_text("continue")
        {
            return false;
        }
    }
    false
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
        | HirExpr::Unknown(_) => {}
    }
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
            let (Some(left_type), Some(right_type)) = (
                inferred_operand_type(analyzer, left).map(str::to_string),
                inferred_operand_type(analyzer, right).map(str::to_string),
            ) else {
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
            let (Some(left_type), Some(right_type)) = (
                inferred_operand_type(analyzer, left).map(str::to_string),
                inferred_operand_type(analyzer, right).map(str::to_string),
            ) else {
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
            }
        }
        BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
            let (Some(left_type), Some(right_type)) = (
                inferred_operand_type(analyzer, left).map(str::to_string),
                inferred_operand_type(analyzer, right).map(str::to_string),
            ) else {
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
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide => {}
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
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide
    )
}

fn non_numeric_operand(analyzer: &Analyzer<'_>, expr: &HirExpr) -> bool {
    inferred_operand_type(analyzer, expr).is_some_and(|type_name| !is_numeric_type(type_name))
}

fn inferred_operand_type<'a>(analyzer: &'a Analyzer<'_>, expr: &'a HirExpr) -> Option<&'a str> {
    match expr {
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
        HirExpr::Number { .. } => Some("Int"),
        HirExpr::String { .. } => Some("String"),
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
    }
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

fn type_root_name(type_name: &str) -> &str {
    type_name.split('<').next().unwrap_or(type_name)
}
