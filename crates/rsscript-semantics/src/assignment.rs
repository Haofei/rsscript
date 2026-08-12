//! Canonical diagnostics for resolved assignment facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

/// Diagnose an invalid assignment target or mutability boundary.
pub fn invalid_assignment_diagnostic(span: Span, label: String, cause: String) -> Diagnostic {
    Diagnostic::error(code::INVALID_ASSIGNMENT, "invalid assignment.", span, label)
        .with_cause(cause)
        .with_fix(
            "declare_let_mut",
            "Declare the target as a `let mut` local, or remove the assignment.",
            "manual",
        )
}

/// Diagnose a type mismatch while reassigning a resolved local binding.
pub fn local_assignment_type_mismatch_diagnostic(
    name: &str,
    value_type: &str,
    target_type: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ASSIGNMENT_TYPE_MISMATCH,
        format!("cannot assign `{value_type}` to `{name}` of type `{target_type}`."),
        span,
        "assignment type mismatch",
    )
    .with_cause("The assigned value's type must match the place's type before Rust lowering.")
    .with_fix(
        "match_assignment_type",
        format!("Assign a `{target_type}` value to `{name}`."),
        "manual",
    )
}

/// Diagnose a type mismatch while assigning through a resolved field/index
/// place.
pub fn place_assignment_type_mismatch_diagnostic(
    value_type: &str,
    target_type: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ASSIGNMENT_TYPE_MISMATCH,
        format!("cannot assign `{value_type}` to `{target_type}` place."),
        span,
        "assignment type mismatch",
    )
    .with_cause(
        "The assigned value's type must match the field or indexed element type before Rust lowering.",
    )
    .with_fix(
        "match_assignment_type",
        format!("Assign a `{target_type}` value to this place."),
        "manual",
    )
}

/// Diagnose an index-assignment target whose resolved base is not a `List`.
pub fn deferred_index_assignment_diagnostic(base_type: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::ASSIGNMENT_TARGET_DEFERRED,
        "index assignment is only supported for List values.",
        span,
        format!("cannot assign through `{base_type}` index"),
    )
    .with_cause(
        "`list[i] = value` has clear in-place list update semantics. Other indexed types still require explicit APIs such as `Map.insert`.",
    )
    .with_fix(
        "use_explicit_update_api",
        "Use the collection's explicit mutating API for this indexed assignment.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "assignment.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn derives_assignment_diagnostics_from_resolved_facts() {
        assert_eq!(
            invalid_assignment_diagnostic(span(), "invalid target".to_owned(), "cause".to_owned())
                .code,
            code::INVALID_ASSIGNMENT
        );
        assert_eq!(
            local_assignment_type_mismatch_diagnostic("value", "String", "Int", span()).code,
            code::ASSIGNMENT_TYPE_MISMATCH
        );
        assert_eq!(
            place_assignment_type_mismatch_diagnostic("String", "Int", span()).code,
            code::ASSIGNMENT_TYPE_MISMATCH
        );
        assert_eq!(
            deferred_index_assignment_diagnostic("Map<String, Int>", span()).code,
            code::ASSIGNMENT_TARGET_DEFERRED
        );
    }
}
