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

/// Diagnose a managed closure that captures a local binding.
pub fn managed_closure_local_capture_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
        format!("managed closure captures local value `{name}`."),
        span,
        "local captured here",
    )
    .with_cause("Closures bound with `let` are managed closures.")
    .with_fix(
        "use_local_closure",
        "Bind the closure with `local` or use a noescape callback.",
        "manual",
    )
}

/// Diagnose a resource which would outlive its `with` scope.
pub fn resource_escape_diagnostic(binding: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_ESCAPE,
        format!("resource `{binding}` cannot escape its `with` block."),
        span,
        "resource escapes",
    )
    .with_cause("A `with` resource must be dropped when the block exits.")
}

/// Diagnose a managed closure capture of a scoped resource.
pub fn resource_capture_diagnostic(binding: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_ESCAPE,
        format!("resource `{binding}` cannot be captured by a managed closure."),
        span,
        "resource captured",
    )
    .with_cause("Managed closures may outlive the `with` block that owns the resource.")
}

/// Diagnose a transient resource producer outside a `with` boundary.
pub fn resource_producer_escape_diagnostic(type_name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_ESCAPE,
        format!("resource-producing expression of type `{type_name}` must be consumed by `with`."),
        span,
        "resource producer escapes",
    )
    .with_cause(
        "Resource-producing calls create transient linear values that cannot be stored, returned, retained, managed, or passed as ordinary values.",
    )
    .with_fix(
        "use_with",
        "Use `with producer(...)? as resource { ... }`.",
        "manual",
    )
}

/// Diagnose a result-wrapped resource producer that lacks an explicit `?`.
pub fn resource_producer_missing_try_diagnostic(resource_type: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_PRODUCER_MISSING_TRY,
        format!(
            "`with` over `Result<{resource_type}, E>` must explicitly unwrap the resource producer with `?`."
        ),
        span,
        "missing resource producer `?`",
    )
    .with_cause(
        "Resource-producing `Result` values are transient; the successful resource must enter the `with` scope explicitly.",
    )
    .with_fix(
        "add_try_to_resource_producer",
        "Write `with producer(...)? as resource { ... }`.",
        "machine-applicable",
    )
}

/// Diagnose binding a managed class handle as a local value.
pub fn local_class_binding_diagnostic(binding: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::LOCAL_CLASS_BINDING,
        format!("class binding `{binding}` cannot be local."),
        span,
        "class bound as local",
    )
    .with_cause("Classes are managed identity objects; their constructors produce managed handles.")
    .with_fix(
        "use_managed_class_binding",
        format!("Declare `{binding}` with `let` instead of `local`."),
        "machine-applicable",
    )
}

/// Diagnose `manage` applied to a value which is not a local binding or fresh shell.
pub fn invalid_manage_operand_diagnostic(cause: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::INVALID_MANAGE_OPERAND,
        "`manage` requires a local binding.",
        span,
        "not a local binding",
    )
    .with_cause(cause)
    .with_fix(
        "remove_manage_or_create_local",
        "Remove `manage`, or create the value as `local` at its origin.",
        "manual",
    )
}

/// Diagnose `take` applied to a value which is not local.
pub fn invalid_take_operand_diagnostic(cause: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::INVALID_TAKE_OPERAND,
        "`take` requires a local value.",
        span,
        "not a local value",
    )
    .with_cause(cause)
    .with_fix(
        "use_local_or_read",
        "Pass a local value with `take`, or use `read`/`mut` for managed values.",
        "manual",
    )
}

/// Diagnose a `fresh` function that returns a value which is not fresh.
pub fn fresh_return_not_clean_diagnostic(
    function_name: &str,
    name: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::FRESH_RETURN_NOT_CLEAN,
        format!("fresh function `{function_name}` returns non-fresh value `{name}`."),
        span,
        "non-fresh value returned",
    )
    .with_cause(
        "A `fresh` return must be newly created or a clean local binding created inside the function.",
    )
    .with_fix(
        "return_fresh_value",
        "Return a struct constructor, fresh call, or clean local binding created inside the function.",
        "manual",
    )
}

/// Warn when the checked facts cannot prove a `fresh` return.
pub fn freshness_unknown_diagnostic(function_name: &str, span: Span) -> Diagnostic {
    Diagnostic::warning(
        code::FRESHNESS_UNKNOWN,
        format!("freshness of return value in `{function_name}` could not be proven."),
        span,
        "freshness unknown",
    )
    .with_cause(
        "This MVP checker trusts clean locals, clean inline fields of locals, struct constructors, known fresh functions, and literals.",
    )
}

/// Diagnose a `fresh` return annotation on a non-struct type.
pub fn invalid_fresh_return_type_diagnostic(
    function_name: &str,
    target_name: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::INVALID_FRESH_RETURN_TYPE,
        format!(
            "function `{function_name}` declares `fresh {target_name}` but `{target_name}` is not a struct."
        ),
        span,
        "invalid fresh type",
    )
    .with_cause("RSScript `fresh` is a shallow guarantee for newly created struct shells.")
    .with_fix(
        "use_struct_fresh_type",
        "Return a struct type as fresh, or remove `fresh` from this return contract.",
        "manual",
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
        assert_eq!(
            managed_closure_local_capture_diagnostic("value", span(1)).code,
            code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE
        );
        assert_eq!(
            resource_escape_diagnostic("resource", span(1)).code,
            code::RESOURCE_ESCAPE
        );
        assert_eq!(
            resource_capture_diagnostic("resource", span(1)).code,
            code::RESOURCE_ESCAPE
        );
        assert_eq!(
            resource_producer_escape_diagnostic("File", span(1)).code,
            code::RESOURCE_ESCAPE
        );
        assert_eq!(
            resource_producer_missing_try_diagnostic("File", span(1)).code,
            code::RESOURCE_PRODUCER_MISSING_TRY
        );
        assert_eq!(
            local_class_binding_diagnostic("object", span(1)).code,
            code::LOCAL_CLASS_BINDING
        );
        assert_eq!(
            invalid_manage_operand_diagnostic("not local", span(1)).code,
            code::INVALID_MANAGE_OPERAND
        );
        assert_eq!(
            invalid_take_operand_diagnostic("not local", span(1)).code,
            code::INVALID_TAKE_OPERAND
        );
        assert_eq!(
            fresh_return_not_clean_diagnostic("build", "value", span(1)).code,
            code::FRESH_RETURN_NOT_CLEAN
        );
        assert_eq!(
            freshness_unknown_diagnostic("build", span(1)).code,
            code::FRESHNESS_UNKNOWN
        );
        assert_eq!(
            invalid_fresh_return_type_diagnostic("build", "Class", span(1)).code,
            code::INVALID_FRESH_RETURN_TYPE
        );
    }
}
