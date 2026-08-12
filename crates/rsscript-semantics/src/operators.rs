//! Canonical diagnostics for resolved builtin operator facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

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
