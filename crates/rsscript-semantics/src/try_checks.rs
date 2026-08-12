//! Checked-HIR diagnostics for the `?` operator.

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
}
