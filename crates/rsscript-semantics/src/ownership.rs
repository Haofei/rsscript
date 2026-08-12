//! Ownership diagnostics derived from checked local-flow facts.

use rsscript_diagnostics::{Diagnostic, FixEdit, Span, code};

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

/// Diagnose an exclusive use of an unbound fresh temporary.
pub fn fresh_requires_local_binding_diagnostic(expression_hint: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::FRESH_REQUIRES_LOCAL_BINDING,
        "`fresh` expression must be bound locally before `mut` or `take` use.",
        span,
        "fresh value requires local binding",
    )
    .with_cause(
        "Direct fresh expressions can materialize as managed temporaries for `read`; `mut` and `take` require an explicit local owner.",
    )
    .with_fix(
        "bind_fresh_local",
        format!("Bind the value first, for example `local value = {expression_hint}."),
        "manual",
    )
}

/// Diagnose a weak constructor field initialized without an explicit weak handle.
pub fn weak_field_requires_weak_handle_diagnostic(
    constructor_name: &str,
    field_name: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::WEAK_FIELD_REQUIRES_WEAK_HANDLE,
        format!(
            "weak field `{field_name}` for `{constructor_name}` must be initialized from an explicit weak handle."
        ),
        span,
        "weak field requires weak handle",
    )
    .with_cause("Weak fields are non-owning handles. Initializing them must be syntax-visible.")
    .with_fix(
        "wrap_with_weak_from",
        format!("Write `{field_name}: Weak.from(value: read target)` in the constructor."),
        "manual",
    )
}

/// Diagnose a constructor field which omits its ownership data effect.
pub fn constructor_field_effect_diagnostic(
    constructor_name: &str,
    field_name: &str,
    expected_effect: &str,
    span: &Span,
    cause: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::MISSING_DATA_EFFECT,
        format!(
            "field `{field_name}` for `{constructor_name}` must be initialized with `{expected_effect}`."
        ),
        span.clone(),
        "missing constructor field effect",
    )
    .with_cause(cause)
    .with_fix_edit(
        "add_constructor_field_effect",
        format!("Write `{field_name}: {expected_effect} ...` in the constructor."),
        FixEdit::insert_before(span, format!("{expected_effect} ")),
    )
}

/// Diagnose an inline constructor field initialized from a managed value.
pub fn managed_inline_constructor_field_diagnostic(
    constructor_name: &str,
    field_name: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::MISSING_DATA_EFFECT,
        format!(
            "field `{field_name}` for `{constructor_name}` cannot be initialized from a managed value."
        ),
        span,
        "managed value used for inline field",
    )
    .with_cause(
        "Inline non-Copy fields own their stored value. RSScript has no implicit clone from managed values into inline fields.",
    )
    .with_fix(
        "make_field_handle_or_bind_local",
        "Use a `handle` field, construct a fresh inline value, or bind the value as `local` and pass it with `take`.",
        "manual",
    )
}

/// Diagnose a spawned task that captures a local value.
pub fn spawn_local_capture_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::LOCAL_VALUE_RETAINED,
        format!("spawn cannot capture local value `{name}`."),
        span,
        "local captured by spawn",
    )
    .with_cause("`spawn` may retain captured values until task completion.")
    .with_fix(
        "manage_before_spawn",
        format!("Convert `{name}` through `manage` before spawning the task."),
        "manual",
    )
}

/// Diagnose an exclusive operation on a read-only loop view.
pub fn read_view_mutation_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::READ_VIEW_MUTATION,
        format!("`{name}` is a read view from a `for` loop and cannot be used as an exclusive value."),
        span,
        "read view mutation",
    )
    .with_cause(
        "RSScript `for` iterates `List<T>` by read view for non-Copy struct elements, so the loop variable does not own the element.",
    )
    .with_fix(
        "copy_before_mutating",
        "Create a fresh local copy before mutation, or use an explicit partitioning API that grants exclusive element ownership.",
        "manual",
    )
}

/// Diagnose consuming a local captured by a `noescape` callback.
pub fn noescape_consumes_capture_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::NOESCAPE_CONSUMES_CAPTURE,
        format!("noescape closure cannot consume captured local value `{name}`."),
        span,
        "captured local consumed here",
    )
    .with_cause(
        "`noescape Fn()` callbacks are non-consuming; the callee may call this closure more than once.",
    )
    .with_cause(
        "Read or mutate the captured local inside the noescape closure, or move/manage it before constructing the closure.",
    )
    .with_fix(
        "avoid_consuming_capture",
        "Do not use `take` or `manage` on captured local values inside noescape callbacks.",
        "manual",
    )
}

/// Diagnose a closure use omitted from an explicit capture declaration.
pub fn explicit_closure_missing_capture_diagnostic(
    name: &str,
    actual_effect: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::CLOSURE_CAPTURE_CONTRACT,
        format!("closure uses `{name}` without declaring it in captures"),
        span,
        "missing closure capture",
    )
    .with_cause(
        "Escaping function values must make every external input explicit in `captures(...)`.",
    )
    .with_cause(format!(
        "Add `captures({actual_effect} {name})` or remove the external use."
    ))
}

/// Diagnose an explicit closure capture that has no corresponding use.
pub fn explicit_closure_unused_capture_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::CLOSURE_CAPTURE_CONTRACT,
        format!("closure declares capture `{name}` but does not use it"),
        span,
        "unused closure capture",
    )
    .with_cause(
        "A closure capture list is review evidence and must describe the function value's real inputs.",
    )
    .with_cause("Remove the capture entry or use the value inside the closure body.")
}

/// Diagnose a mismatch between a declared closure capture effect and its use.
pub fn explicit_closure_capture_contract_diagnostic(
    name: &str,
    declared_effect: &str,
    actual_effect: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::CLOSURE_CAPTURE_CONTRACT,
        format!(
            "closure capture `{name}` is declared as `{declared_effect}` but used as `{actual_effect}`"
        ),
        span,
        "closure capture effect mismatch",
    )
    .with_cause("Closure captures use the same read/mut/take ownership vocabulary as parameters.")
    .with_cause(format!(
        "Change the capture to `{actual_effect} {name}` or change the closure body to match the declared access."
    ))
}

/// Diagnose an unused binding whose open variant parameter cannot be inferred.
pub fn uninferable_binding_type_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::UNINFERABLE_BINDING_TYPE,
        format!("the type of `{name}` cannot be inferred."),
        span,
        "uninferable binding type",
    )
    .with_cause(
        "A bare `Ok(...)`, `Err(...)`, or `None` leaves a type parameter open, and this binding is never used, so nothing can constrain it — the type is ambiguous and would not lower to valid Rust.",
    )
    .with_fix(
        "annotate_binding_type",
        "Add a type annotation (e.g. `let v: Result<Int, String> = ...`) or remove the unused binding.",
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
        assert_eq!(
            fresh_requires_local_binding_diagnostic("make()", span(1)).code,
            code::FRESH_REQUIRES_LOCAL_BINDING
        );
        assert_eq!(
            weak_field_requires_weak_handle_diagnostic("Node", "parent", span(1)).code,
            code::WEAK_FIELD_REQUIRES_WEAK_HANDLE
        );
        assert_eq!(
            constructor_field_effect_diagnostic("Node", "child", "take", &span(1), "owns").code,
            code::MISSING_DATA_EFFECT
        );
        assert_eq!(
            managed_inline_constructor_field_diagnostic("Node", "child", span(1)).code,
            code::MISSING_DATA_EFFECT
        );
        assert_eq!(
            spawn_local_capture_diagnostic("value", span(1)).code,
            code::LOCAL_VALUE_RETAINED
        );
        assert_eq!(
            read_view_mutation_diagnostic("item", span(1)).code,
            code::READ_VIEW_MUTATION
        );
        assert_eq!(
            noescape_consumes_capture_diagnostic("item", span(1)).code,
            code::NOESCAPE_CONSUMES_CAPTURE
        );
        assert_eq!(
            explicit_closure_missing_capture_diagnostic("item", "read", span(1)).code,
            code::CLOSURE_CAPTURE_CONTRACT
        );
        assert_eq!(
            explicit_closure_unused_capture_diagnostic("item", span(1)).code,
            code::CLOSURE_CAPTURE_CONTRACT
        );
        assert_eq!(
            explicit_closure_capture_contract_diagnostic("item", "read", "take", span(1)).code,
            code::CLOSURE_CAPTURE_CONTRACT
        );
        assert_eq!(
            uninferable_binding_type_diagnostic("value", span(1)).code,
            code::UNINFERABLE_BINDING_TYPE
        );
    }
}
