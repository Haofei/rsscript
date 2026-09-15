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

/// What the enclosing function lets `?` propagate.
///
/// `?` is an early return of the failure case, so it needs a return type that
/// can carry one. Anything the checker cannot classify with confidence — a
/// generic return type, an unexpanded alias, a closure body whose contract is
/// not known here — is [`TryContext::Unknown`] and is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryContext<'a> {
    /// `-> Result<T, E>`: `?` propagates `E`, which must match exactly.
    ResultError(&'a str),
    /// `-> Option<T>`: `?` propagates `None`.
    Option,
    /// A concrete return type that can carry neither a failure nor a `None`.
    Unsupported(&'a str),
    /// Not classifiable; no obligation is derived.
    Unknown,
}

impl<'a> TryContext<'a> {
    /// Classify an enclosing function's rendered return type. Callers are
    /// expected to have expanded type aliases first.
    pub fn from_return_type(return_type: Option<&'a str>) -> Self {
        let Some(return_type) = return_type.map(str::trim) else {
            return TryContext::Unknown;
        };
        if is_option_type(return_type) {
            return TryContext::Option;
        }
        if is_result_type(return_type) {
            return match result_error_type_name(return_type) {
                Some(error_type) => TryContext::ResultError(error_type),
                // `Result` without a spelled-out error type carries a failure
                // but names no error type to match against.
                None => TryContext::Unknown,
            };
        }
        if is_unclassifiable_return_type(return_type) {
            return TryContext::Unknown;
        }
        TryContext::Unsupported(return_type)
    }

    /// The context seen by a closure body nested in this one. A closure has its
    /// own return contract, which is not modelled here, so a `?` inside it is
    /// never reported as having no propagation target.
    fn inside_closure(self) -> Self {
        match self {
            TryContext::Unsupported(_) => TryContext::Unknown,
            other => other,
        }
    }
}

/// A return type the checker will not judge: a generic parameter or any type
/// still carrying an unresolved generic placeholder.
fn is_unclassifiable_return_type(return_type: &str) -> bool {
    return_type.is_empty()
        || return_type.contains('?')
        || (return_type.len() == 1
            && return_type
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase()))
}

/// Diagnose `?` used where the enclosing function cannot propagate a failure,
/// and `Result` error-type mismatches introduced by `?` in a function.
pub fn try_error_type_diagnostics(block: &HirBlock, context: TryContext<'_>) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_block(block, context, &mut diagnostics);
    diagnostics
}

fn try_without_propagation_target_diagnostic(span: &Span, return_type: &str) -> Diagnostic {
    Diagnostic::error(
        code::INVALID_TRY_OPERATOR,
        "`?` requires a function that returns `Result<T, E>` or `Option<T>`.",
        span.clone(),
        "no propagation target",
    )
    .with_cause(format!(
        "`?` returns the failure case out of the enclosing function, but this function returns `{return_type}`."
    ))
    .with_fix(
        "return_result_or_handle",
        "Return `Result<T, E>` or `Option<T>` from this function, or handle the failure with `match`.",
        "manual",
    )
}

fn collect_block(block: &HirBlock, context: TryContext<'_>, diagnostics: &mut Vec<Diagnostic>) {
    for statement in &block.statements {
        collect_statement(statement, context, diagnostics);
    }
}

fn collect_statement(
    statement: &HirStmt,
    function_error_type: TryContext<'_>,
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
    function_error_type: TryContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        HirExpr::Try { value, span, .. } => {
            match function_error_type {
                TryContext::Unsupported(return_type) => {
                    diagnostics.push(try_without_propagation_target_diagnostic(span, return_type));
                }
                TryContext::ResultError(function_error_type) => {
                    if let Some(operand_error_type) =
                        hir_expr_type_name(value).and_then(result_error_type_name)
                        && operand_error_type != function_error_type
                    {
                        diagnostics.push(try_error_type_mismatch_diagnostic(
                            span,
                            operand_error_type,
                            function_error_type,
                        ));
                    }
                }
                TryContext::Option | TryContext::Unknown => {}
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
        HirExpr::Closure { body, .. } => {
            collect_block(body, function_error_type.inside_closure(), diagnostics)
        }
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

        let diagnostics = try_error_type_diagnostics(&block, TryContext::ResultError("AppError"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::INVALID_TRY_OPERATOR);
        assert!(diagnostics[0].label.contains("mismatched try error type"));
    }

    fn try_block() -> HirBlock {
        HirBlock {
            statements: vec![HirStmt::Expr(HirExpr::Try {
                value: Box::new(HirExpr::Ident {
                    name: "operation".to_owned(),
                    type_name: Some("Result<Int, AppError>".to_owned()),
                    span: span(),
                }),
                type_name: Some("Int".to_owned()),
                span: span(),
            })],
            span: span(),
        }
    }

    #[test]
    fn classifies_a_function_return_type_into_a_try_context() {
        assert_eq!(
            TryContext::from_return_type(Some("Result<Int, AppError>")),
            TryContext::ResultError("AppError")
        );
        assert_eq!(
            TryContext::from_return_type(Some("Option<Int>")),
            TryContext::Option
        );
        assert_eq!(
            TryContext::from_return_type(Some("Unit")),
            TryContext::Unsupported("Unit")
        );
        // A bare type parameter, an unresolved placeholder, and a missing
        // return type are all left unjudged.
        assert_eq!(TryContext::from_return_type(Some("T")), TryContext::Unknown);
        assert_eq!(
            TryContext::from_return_type(Some("List<?>")),
            TryContext::Unknown
        );
        assert_eq!(TryContext::from_return_type(None), TryContext::Unknown);
    }

    #[test]
    fn rejects_try_in_a_function_that_cannot_propagate_a_failure() {
        let diagnostics = try_error_type_diagnostics(&try_block(), TryContext::Unsupported("Int"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::INVALID_TRY_OPERATOR);
        assert!(diagnostics[0].label.contains("no propagation target"));
    }

    #[test]
    fn accepts_try_in_option_and_matching_result_functions() {
        assert!(try_error_type_diagnostics(&try_block(), TryContext::Option).is_empty());
        assert!(
            try_error_type_diagnostics(&try_block(), TryContext::ResultError("AppError"))
                .is_empty()
        );
        assert!(try_error_type_diagnostics(&try_block(), TryContext::Unknown).is_empty());
    }

    #[test]
    fn a_closure_body_is_not_judged_against_the_outer_return_type() {
        let block = HirBlock {
            statements: vec![HirStmt::Expr(HirExpr::Closure {
                params: Vec::new(),
                captures: Vec::new(),
                explicit: false,
                ty: None,
                body: try_block(),
                span: span(),
            })],
            span: span(),
        };
        assert!(try_error_type_diagnostics(&block, TryContext::Unsupported("Unit")).is_empty());
    }
}
