//! Ownership diagnostics derived from checked local-flow facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

/// Diagnose use of a value after it moved into managed storage.
pub fn moved_use_diagnostic(name: &str, use_span: Span, move_span: &Span) -> Diagnostic {
    Diagnostic::error(
        code::USE_AFTER_MANAGE,
        format!("`{name}` was moved into the managed runtime by `manage {name}`."),
        use_span,
        "used after manage",
    )
    .with_cause(format!(
        "The move happened at {}:{}.",
        move_span.line, move_span.column
    ))
    .with_fix(
        "move_use_before_manage",
        format!("Move this use before `manage {name}`."),
        "manual",
    )
}

/// Diagnose binding an already-managed value as a local value.
pub fn managed_to_local_diagnostic(local_name: &str, managed_name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::MANAGED_TO_LOCAL,
        format!("managed value cannot be converted to local binding `{local_name}`."),
        span,
        "managed value used as local",
    )
    .with_cause(format!(
        "`{managed_name}` is already managed; RSScript has no managed -> local conversion."
    ))
    .with_fix(
        "create_local",
        "Create the value as `local` at its creation point.",
        "manual",
    )
}

/// Diagnose a retaining call that receives a local value.
pub fn retained_local_diagnostic(name: &str, callee: &str, param: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::LOCAL_VALUE_RETAINED,
        format!("retaining API `{callee}` cannot retain local value `{name}`."),
        span,
        "local value retained",
    )
    .with_cause(format!("`{callee}` declares `retains({param})`."))
    .with_fix(
        "manage_local",
        format!("Pass `{param}` through `manage {name}` before retaining it."),
        "manual",
    )
}

/// Diagnose a managed closure that retains a captured local value.
pub fn retained_closure_capture_diagnostic(
    name: &str,
    callee: &str,
    param: &str,
    capture_span: Span,
    closure_span: &Span,
) -> Diagnostic {
    Diagnostic::error(
        code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
        format!("retained closure passed to `{callee}` captures local value `{name}`."),
        capture_span,
        "local captured here",
    )
    .with_cause(format!(
        "`{callee}` declares `retains({param})`; the closure may outlive local values."
    ))
    .with_cause(format!(
        "The retained closure starts at {}:{}.",
        closure_span.line, closure_span.column
    ))
    .with_fix(
        "avoid_retained_capture",
        "Do not capture local values in closures passed to retaining APIs.",
        "manual",
    )
}

/// Diagnose consuming a managed handle field as an inline local value.
pub fn take_handle_field_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::TAKE_HANDLE_FIELD,
        format!("cannot `take` handle field `{name}`."),
        span,
        "take of handle field",
    )
    .with_cause(
        "Handle fields are managed references and cannot be consumed as local inline values.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(column: usize) -> Span {
        Span {
            file: "ownership.rss".to_owned(),
            line: 1,
            column,
            length: 1,
        }
    }

    #[test]
    fn derives_diagnostics_from_local_flow_facts() {
        assert_eq!(
            moved_use_diagnostic("value", span(9), &span(1)).code,
            code::USE_AFTER_MANAGE
        );
        assert_eq!(
            managed_to_local_diagnostic("copy", "value", span(1)).code,
            code::MANAGED_TO_LOCAL
        );
        assert_eq!(
            retained_local_diagnostic("value", "store", "item", span(1)).code,
            code::LOCAL_VALUE_RETAINED
        );
        assert_eq!(
            retained_closure_capture_diagnostic("value", "store", "callback", span(1), &span(4))
                .code,
            code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE
        );
        assert_eq!(
            take_handle_field_diagnostic("child", span(1)).code,
            code::TAKE_HANDLE_FIELD
        );
    }
}
