//! Diagnostics for compiler-resolved type compatibility facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

pub fn binding_type_mismatch_diagnostic(
    name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("binding `{name}` has initializer type `{actual}`, expected `{expected}`."),
        span,
        "binding type mismatch",
    )
    .with_cause(
        "Explicit `let` and `local` type annotations are source-level contracts and must match the initializer before Rust lowering.",
    )
    .with_fix(
        "match_binding_type",
        format!("Initialize `{name}` with a `{expected}` value, or change the binding annotation."),
        "manual",
    )
}

pub fn binding_payload_type_mismatch_diagnostic(
    name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("binding `{name}` has initializer payload type `{actual}`, expected `{expected}`."),
        span,
        "binding type mismatch",
    )
    .with_cause(
        "Result and Option binding initializers are checked against explicit binding payload types before Rust lowering.",
    )
    .with_fix(
        "match_binding_payload_type",
        format!("Initialize `{name}` with a `{expected}` payload, or change the binding annotation."),
        "manual",
    )
}

pub fn argument_payload_type_mismatch_diagnostic(
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("argument `{arg_name}` for `{call_name}` has payload type `{actual}`, expected `{expected}`."),
        span,
        "argument type mismatch",
    )
    .with_cause(
        "Result and Option argument constructors are checked against the resolved parameter payload before Rust lowering.",
    )
    .with_fix(
        "match_argument_payload_type",
        format!("Pass a `{expected}` payload for `{arg_name}`."),
        "manual",
    )
}

pub fn argument_type_mismatch_diagnostic(
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
    .with_cause("RSScript call argument types must match the resolved callee signature before Rust lowering.")
    .with_fix(
        "match_argument_type",
        format!("Pass a value of type `{expected}` for `{arg_name}`."),
        "manual",
    )
}

pub fn map_literal_entry_type_mismatch_diagnostic(
    role: &str,
    actual: &str,
    expected: &str,
    context: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("map literal {role} has type `{actual}`, expected `{expected}`."),
        span,
        "map literal entry type mismatch",
    )
    .with_cause(format!(
        "The {context} is typed as a `Map`, so every map literal {role} must match the corresponding `Map` type argument before Rust lowering."
    ))
    .with_fix(
        "match_map_literal_entry_type",
        format!("Use a {role} expression of type `{expected}`."),
        "manual",
    )
}

pub fn list_literal_item_type_mismatch_diagnostic(
    actual: &str,
    expected: &str,
    context: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("list literal item has type `{actual}`, expected `{expected}`."),
        span,
        "list literal item type mismatch",
    )
    .with_cause(format!(
        "The {context} is typed as a `List`, so every array literal item must match the `List` item type before Rust lowering."
    ))
    .with_fix(
        "match_list_literal_item_type",
        format!("Use a `{expected}` value for this list literal item."),
        "manual",
    )
}

pub fn unknown_callee_diagnostic(call_name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_CALLEE,
        format!("call to `{call_name}` does not resolve."),
        span,
        "unknown callee",
    )
    .with_cause(
        "The callee is not a user function, known type constructor, enum variant, or builtin signature.",
    )
    .with_fix(
        "declare_or_import_callee",
        "Declare the function or add a builtin signature for this API.",
        "manual",
    )
}

pub fn ambiguous_receiver_call_diagnostic(
    call_name: &str,
    candidates: &[String],
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_CALLEE,
        format!("receiver-call `{call_name}` is ambiguous between {}.", candidates.join(", ")),
        span,
        "ambiguous receiver call",
    )
    .with_cause(
        "Receiver-call shorthand is only allowed when exactly one inherent or protocol method candidate is visible.",
    )
    .with_fix(
        "use_canonical_call",
        "Write the canonical qualified call explicitly.",
        "manual",
    )
}

pub fn message_payload_not_transferable_diagnostic(element: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::MESSAGE_PAYLOAD_NOT_TRANSFERABLE,
        format!("message channel payload `{element}` is not cross-isolate-transferable."),
        span,
        "non-transferable message payload",
    )
    .with_cause(
        "A message must be self-contained data with no managed handle, so it can cross an isolate boundary without sharing mutable state. v1 allows Copy scalars, `String`, and `Bytes`.",
    )
    .with_fix(
        "use_transferable_message_payload",
        format!(
            "Send a transferable value (a Copy scalar, `String`, or `Bytes`) instead of `{element}`, or use `Channel.bounded` for an in-isolate channel."
        ),
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "types.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn derives_type_compatibility_diagnostics_from_resolved_facts() {
        assert_eq!(
            binding_type_mismatch_diagnostic("value", "String", "Int", span()).code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            binding_payload_type_mismatch_diagnostic("value", "String", "Int", span()).code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            argument_payload_type_mismatch_diagnostic("call", "value", "String", "Int", span())
                .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            argument_type_mismatch_diagnostic("call", "value", "String", "Int", span()).code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            map_literal_entry_type_mismatch_diagnostic("key", "String", "Int", "argument", span())
                .code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            list_literal_item_type_mismatch_diagnostic("String", "Int", "argument", span()).code,
            code::ARGUMENT_TYPE_MISMATCH
        );
        assert_eq!(
            unknown_callee_diagnostic("missing", span()).code,
            code::UNKNOWN_CALLEE
        );
        assert_eq!(
            ambiguous_receiver_call_diagnostic("item.run", &["A.run".to_owned()], span()).code,
            code::UNKNOWN_CALLEE
        );
        assert_eq!(
            message_payload_not_transferable_diagnostic("Handle", span()).code,
            code::MESSAGE_PAYLOAD_NOT_TRANSFERABLE
        );
    }
}
