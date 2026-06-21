use super::*;

pub(super) fn check_try_value_is_result(analyzer: &mut Analyzer<'_>, value: &HirExpr, span: &Span) {
    let Some(type_name) = hir_expr_type_name(value) else {
        return;
    };
    // `?` applies to either failure-carrying type: `Result` (short-circuits `Err`)
    // or `Option` (short-circuits `None`).
    if is_result_type(type_name) || is_option_type(type_name) {
        return;
    }

    analyzer.diagnostics.push(
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
    );
}

pub(super) fn check_try_error_types(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    function_error_type: Option<&str>,
) {
    for statement in &block.statements {
        check_try_error_types_stmt(analyzer, statement, function_error_type);
    }
}

pub(super) fn check_try_error_types_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    function_error_type: Option<&str>,
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
            check_try_error_types_expr(analyzer, value, function_error_type)
        }
        HirStmt::With { resource, body, .. } => {
            check_try_error_types_expr(analyzer, resource, function_error_type);
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_try_error_types_expr(analyzer, condition, function_error_type);
            check_try_error_types(analyzer, then_body, function_error_type);
            if let Some(else_body) = else_body {
                check_try_error_types(analyzer, else_body, function_error_type);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_try_error_types_expr(analyzer, condition, function_error_type);
            }
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirStmt::For { iterable, body, .. } => {
            check_try_error_types_expr(analyzer, iterable, function_error_type);
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirStmt::Match { value, arms, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type);
            for arm in arms {
                check_try_error_types(analyzer, &arm.body, function_error_type);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_try_error_types_expr(analyzer, &arm.operation, function_error_type);
                check_try_error_types(analyzer, &arm.body, function_error_type);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_try_error_types_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    function_error_type: Option<&str>,
) {
    match expr {
        HirExpr::Try { value, span, .. } => {
            if let (Some(function_error_type), Some(operand_type)) =
                (function_error_type, hir_expr_type_name(value))
                && let Some(operand_error_type) = result_error_type_name(operand_type)
                && operand_error_type != function_error_type
            {
                try_error_type_mismatch_diagnostic(
                    analyzer,
                    span,
                    operand_error_type,
                    function_error_type,
                );
            }
            check_try_error_types_expr(analyzer, value, function_error_type);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_try_error_types_expr(analyzer, &arg.value, function_error_type);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type);
        }
        HirExpr::Binary { left, right, .. } => {
            check_try_error_types_expr(analyzer, left, function_error_type);
            check_try_error_types_expr(analyzer, right, function_error_type);
        }
        HirExpr::Field { base, .. } => {
            check_try_error_types_expr(analyzer, base, function_error_type);
        }
        HirExpr::Index { base, index, .. } => {
            check_try_error_types_expr(analyzer, base, function_error_type);
            check_try_error_types_expr(analyzer, index, function_error_type);
        }
        HirExpr::Closure { body, .. } => {
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirExpr::Match { value, arms, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    check_try_error_types_expr(analyzer, guard, function_error_type);
                }
                check_try_error_types(analyzer, &arm.body, function_error_type);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_try_error_types_expr(analyzer, &entry.key, function_error_type);
                check_try_error_types_expr(analyzer, &entry.value, function_error_type);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                check_try_error_types_expr(analyzer, &field.value, function_error_type);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                check_try_error_types_expr(analyzer, item, function_error_type);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| builtin_value_type_name(name)),
        HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn builtin_value_type_name(name: &str) -> Option<&'static str> {
    match name {
        "true" | "false" => Some("Bool"),
        "null" => Some("JsonLiteral"),
        "Unit" => Some("Unit"),
        "None" => Some("Option<?>"),
        _ => None,
    }
}

pub(super) fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

pub(super) fn is_result_type(type_name: &str) -> bool {
    type_name == "Result" || type_name.starts_with("Result<")
}

pub(super) fn is_option_type(type_name: &str) -> bool {
    type_name == "Option" || type_name.starts_with("Option<")
}

pub(super) fn result_error_type_ref_name(return_ty: &TypeRef) -> Option<String> {
    if return_ty.name != "Result" || return_ty.args.len() != 2 {
        return None;
    }
    return_ty.args.get(1).map(type_ref_name)
}

pub(super) fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}
