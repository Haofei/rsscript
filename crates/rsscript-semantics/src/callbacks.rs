//! Canonical diagnostics for resolved callback contract facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

pub fn callback_operator_type_mismatch_diagnostic(
    operator: &str,
    left_type: &str,
    right_type: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::OPERATOR_TYPE_MISMATCH,
        format!("operator `{operator}` has operands `{left_type}` and `{right_type}`, expected {expected}."),
        span,
        "operator type mismatch",
    )
    .with_cause(
        "`noescape Fn(...)` callback parameter types apply inside callback expressions before Rust lowering.",
    )
    .with_fix(
        "use_typed_operator_operands",
        "Use operands with matching RSScript types.",
        "manual",
    )
}

pub fn callback_return_type_mismatch_diagnostic(
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("callback argument `{arg_name}` for `{call_name}` returns `{actual}`, expected `{expected}`."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn() -> T` callback return types are part of the call signature and must be checked before Rust lowering.",
    )
    .with_fix(
        "match_callback_return_type",
        format!("Return a `{expected}` value from this callback."),
        "manual",
    )
}

pub fn callback_fresh_return_not_clean_diagnostic(
    call_name: &str,
    arg_name: &str,
    name: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("callback argument `{arg_name}` for `{call_name}` returns non-fresh value `{name}`, expected `{expected}`."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn() -> fresh T` callback returns are fresh contracts and cannot return captured or managed values.",
    )
    .with_fix(
        "return_fresh_callback_value",
        "Return a struct constructor, fresh call, or local value created inside the callback.",
        "manual",
    )
}

pub fn callback_fresh_return_unknown_diagnostic(
    call_name: &str,
    arg_name: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("callback argument `{arg_name}` for `{call_name}` returns value whose freshness cannot be proven, expected `{expected}`."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn() -> fresh T` callback returns must be proven fresh before Rust lowering.",
    )
    .with_fix(
        "return_fresh_callback_value",
        "Return a struct constructor, fresh call, or local value created inside the callback.",
        "manual",
    )
}

pub fn callback_arity_mismatch_diagnostic(
    call_name: &str,
    arg_name: &str,
    actual: usize,
    expected: usize,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("callback argument `{arg_name}` for `{call_name}` has {actual} parameter(s), expected {expected}."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn(...) -> T` callback parameter counts are part of the call signature and must be checked before Rust lowering.",
    )
    .with_fix(
        "match_callback_parameter_count",
        format!("Use a callback with {expected} parameter(s)."),
        "manual",
    )
}

pub fn callback_call_arity_mismatch_diagnostic(
    callback_name: &str,
    actual: usize,
    expected: usize,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("callback `{callback_name}` called with {actual} argument(s), expected {expected}."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn(...)` callback calls must match the callback parameter contract before Rust lowering.",
    )
    .with_fix(
        "match_callback_call_arity",
        format!("Call `{callback_name}` with {expected} argument(s)."),
        "manual",
    )
}

pub fn callback_call_argument_type_mismatch_diagnostic(
    callback_name: &str,
    index: usize,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("argument {} for callback `{callback_name}` has type `{actual}`, expected `{expected}`.", index + 1),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn(...)` callback argument types are part of the callback contract and must be checked before Rust lowering.",
    )
    .with_fix(
        "match_callback_call_argument_type",
        format!("Pass a `{expected}` value for argument {}.", index + 1),
        "manual",
    )
}

pub fn callback_call_site_argument_type_mismatch_diagnostic(
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("argument `{arg_name}` for `{call_name}` has type `{actual}`, expected `{expected}`."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "`noescape Fn(...)` callback parameter types apply to ordinary calls inside callback expressions before Rust lowering.",
    )
    .with_fix(
        "match_callback_body_call_argument_type",
        format!("Pass a `{expected}` value for `{arg_name}`."),
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "callback.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn derives_callback_contract_diagnostics_from_resolved_facts() {
        assert_eq!(
            callback_operator_type_mismatch_diagnostic(
                "+",
                "Int",
                "String",
                "matching operands",
                span()
            )
            .code,
            code::OPERATOR_TYPE_MISMATCH
        );
        assert_eq!(
            callback_return_type_mismatch_diagnostic("apply", "callback", "String", "Int", span())
                .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            callback_fresh_return_not_clean_diagnostic(
                "apply",
                "callback",
                "value",
                "fresh Node",
                span()
            )
            .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            callback_fresh_return_unknown_diagnostic("apply", "callback", "fresh Node", span())
                .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            callback_arity_mismatch_diagnostic("apply", "callback", 1, 2, span()).code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            callback_call_arity_mismatch_diagnostic("callback", 1, 2, span()).code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            callback_call_argument_type_mismatch_diagnostic("callback", 0, "String", "Int", span())
                .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            callback_call_site_argument_type_mismatch_diagnostic(
                "call",
                "value",
                "String",
                "Int",
                span()
            )
            .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
    }
}
