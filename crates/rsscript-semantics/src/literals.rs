//! Literal validity diagnostics over checked HIR.

use crate::hir::HirExpr;
use rsscript_diagnostics::{Diagnostic, code};

/// Diagnose a decimal integer literal outside RSScript's `Int` range.
pub fn integer_literal_range_diagnostic(expr: &HirExpr) -> Option<Diagnostic> {
    let HirExpr::Number { value, span, .. } = expr else {
        return None;
    };
    let is_decimal_integer = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    if !is_decimal_integer || value.parse::<i64>().is_ok() {
        return None;
    }
    Some(
        Diagnostic::error(
            code::INTEGER_LITERAL_OUT_OF_RANGE,
            format!("integer literal `{value}` does not fit in `Int` (i64)."),
            span.clone(),
            "integer literal out of range",
        )
        .with_cause("RSScript `Int` is a 64-bit signed integer; literals must fit in i64.")
        .with_fix(
            "use_in_range_literal",
            "Use a value within i64 range.",
            "manual",
        ),
    )
}

/// Diagnose a `Char` literal that does not contain exactly one Unicode scalar.
pub fn char_literal_scalar_diagnostic(expr: &HirExpr) -> Option<Diagnostic> {
    let HirExpr::Char { value, span } = expr else {
        return None;
    };
    let count = value.chars().count();
    if count == 1 {
        return None;
    }
    Some(
        Diagnostic::error(
            code::CHAR_LITERAL_NOT_SINGLE_SCALAR,
            format!("character literal must contain exactly one character, found {count}."),
            span.clone(),
            "invalid character literal",
        )
        .with_cause("A `Char` is a single Unicode scalar value; `''` is empty and `'ab'` holds more than one.")
        .with_fix(
            "use_single_char_literal",
            "Put exactly one character between the single quotes, or use a `String` literal (double quotes) for text.",
            "manual",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_diagnostics::Span;

    fn span() -> Span {
        Span {
            file: "literals.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn rejects_out_of_range_integer_literals() {
        let literal = HirExpr::Number {
            value: "9223372036854775808".to_owned(),
            span: span(),
        };
        let diagnostic = integer_literal_range_diagnostic(&literal).expect("must exceed i64");
        assert_eq!(diagnostic.code, code::INTEGER_LITERAL_OUT_OF_RANGE);
    }

    #[test]
    fn requires_one_scalar_in_char_literals() {
        let literal = HirExpr::Char {
            value: "ab".to_owned(),
            span: span(),
        };
        let diagnostic = char_literal_scalar_diagnostic(&literal).expect("must reject two chars");
        assert_eq!(diagnostic.code, code::CHAR_LITERAL_NOT_SINGLE_SCALAR);
        let scalar = HirExpr::Char {
            value: "🦀".to_owned(),
            span: span(),
        };
        assert!(char_literal_scalar_diagnostic(&scalar).is_none());
    }
}
