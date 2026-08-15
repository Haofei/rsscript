//! Diagnostics for compiler-resolved type compatibility facts.

use std::collections::HashSet;

use rsscript_diagnostics::{Diagnostic, Span, code};

/// Facts that determine whether a rendered legacy HIR type still contains an
/// unresolved generic parameter.
#[derive(Debug, Clone, Default)]
pub struct UnresolvedGenericFacts {
    pub declared_type_names: HashSet<String>,
    pub active_generic_names: HashSet<String>,
}

/// Whether a rendered type references a generic parameter that has not been
/// substituted. This is a semantic rule shared by calls, closures, literals,
/// and assignment checking.
pub fn contains_unresolved_generic_type(type_name: &str, facts: &UnresolvedGenericFacts) -> bool {
    let root = crate::type_root_name(type_name);
    if facts.active_generic_names.contains(root) {
        return true;
    }
    // A declared generic container (for example `Channel<T>`) is not itself
    // an unresolved type, but its arguments can still be unresolved. Do not
    // return early for the declared root or an inferred `T` escapes type
    // checking through `fresh Channel<T>`.
    (!facts.declared_type_names.contains(root)
        && root.len() == 1
        && root.chars().all(|character| character.is_ascii_uppercase()))
        || type_name
            .trim()
            .strip_prefix("fresh ")
            .is_some_and(|target| contains_unresolved_generic_type(target.trim(), facts))
        || crate::type_arg_names(type_name).is_some_and(|arguments| {
            arguments
                .iter()
                .any(|argument| contains_unresolved_generic_type(argument, facts))
        })
        || function_return_type(type_name)
            .is_some_and(|return_type| contains_unresolved_generic_type(return_type, facts))
        || function_parameter_types(type_name)
            .iter()
            .any(|parameter| contains_unresolved_generic_type(parameter, facts))
}

/// Whether a rendered type includes one of the supplied unresolved generic
/// parameter names. Intended for unresolved callee signatures.
pub fn type_contains_unresolved_generic(type_name: &str, generic_names: &[String]) -> bool {
    contains_unresolved_generic_type(
        type_name,
        &UnresolvedGenericFacts {
            active_generic_names: generic_names.iter().cloned().collect(),
            ..UnresolvedGenericFacts::default()
        },
    )
}

/// Compare two rendered, alias-expanded source types using the language's
/// structural compatibility rule. Callers resolve aliases and generic
/// substitutions first; this function owns qualifier/function/container
/// comparison itself.
pub fn type_compatible(expected: &str, actual: &str) -> bool {
    if expected == actual || expected == "Self" {
        return true;
    }
    if strip_fresh_type(expected) == strip_fresh_type(actual) {
        return true;
    }
    if function_type_compatible(expected, actual) {
        return true;
    }
    if crate::type_root_name(expected) == crate::type_root_name(actual)
        && let (Some(expected_args), Some(actual_args)) = (
            crate::type_arg_names(expected),
            crate::type_arg_names(actual),
        )
        && expected_args.len() == actual_args.len()
        && expected_args
            .into_iter()
            .zip(actual_args)
            .all(|(expected, actual)| type_compatible(expected.trim(), actual.trim()))
    {
        return true;
    }
    matches!(
        (actual, crate::type_root_name(expected)),
        ("Option<?>", "Option") | ("Result<?>", "Result")
    )
}

fn function_type_compatible(expected: &str, actual: &str) -> bool {
    if !is_function_type(expected)
        || !is_function_type(actual)
        || function_type_prefix(expected) != function_type_prefix(actual)
    {
        return false;
    }
    let expected_params = function_parameter_types(expected);
    let actual_params = function_parameter_types(actual);
    if expected_params.len() != actual_params.len()
        || !expected_params
            .iter()
            .zip(actual_params.iter())
            .all(|(expected, actual)| type_compatible(expected, actual))
    {
        return false;
    }
    match (function_return_type(expected), function_return_type(actual)) {
        (Some(expected), Some(actual)) => type_compatible(expected, actual),
        (None, None) => true,
        _ => false,
    }
}

fn strip_fresh_type(type_name: &str) -> &str {
    type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name.trim())
}

fn is_function_type(type_name: &str) -> bool {
    function_body(type_name).is_some()
}

fn function_body(type_name: &str) -> Option<&str> {
    type_name
        .trim()
        .strip_prefix("noescape ")
        .or_else(|| type_name.trim().strip_prefix("owned "))
        .unwrap_or(type_name.trim())
        .strip_prefix("Fn(")
}

fn function_return_type(type_name: &str) -> Option<&str> {
    function_body(type_name)
        .and_then(|body| body.split_once(')'))
        .and_then(|(_, rest)| rest.trim_start().strip_prefix("->"))
        .map(str::trim)
}

fn function_parameter_types(type_name: &str) -> Vec<&str> {
    let Some(params) = function_body(type_name)
        .and_then(|body| body.split_once(')').map(|(params, _)| params.trim()))
    else {
        return Vec::new();
    };
    if params.is_empty() {
        return Vec::new();
    }
    split_top_level(params)
        .into_iter()
        .map(strip_parameter_effect)
        .collect()
}

fn split_top_level(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(value[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if start < value.len() {
        parts.push(value[start..].trim());
    }
    parts
}

fn strip_parameter_effect(parameter: &str) -> &str {
    ["read ", "mut ", "take "]
        .into_iter()
        .find_map(|prefix| parameter.trim().strip_prefix(prefix).map(str::trim))
        .unwrap_or_else(|| parameter.trim())
}

fn function_type_prefix(type_name: &str) -> &'static str {
    let type_name = type_name.trim();
    if type_name.starts_with("noescape ") {
        "noescape "
    } else if type_name.starts_with("owned ") {
        "owned "
    } else {
        ""
    }
}

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

    #[test]
    fn structurally_compares_function_and_container_types() {
        assert!(type_compatible("Fn(Int) -> Int", "Fn(read Int) -> Int"));
        assert!(type_compatible(
            "List<owned Fn(Int) -> Int>",
            "List<owned Fn(read Int) -> Int>"
        ));
        assert!(!type_compatible("Fn(Int) -> Int", "Fn(Int) -> String"));
        assert!(!type_compatible(
            "noescape Fn(Int) -> Int",
            "Fn(read Int) -> Int"
        ));
    }

    #[test]
    fn detects_unresolved_generics_from_neutral_type_facts() {
        let facts = UnresolvedGenericFacts {
            declared_type_names: HashSet::from(["Widget".to_owned()]),
            active_generic_names: HashSet::from(["T".to_owned()]),
        };
        assert!(contains_unresolved_generic_type("List<T>", &facts));
        assert!(contains_unresolved_generic_type(
            "Fn(read T) -> Int",
            &facts
        ));
        assert!(!contains_unresolved_generic_type("Widget", &facts));
        assert!(contains_unresolved_generic_type(
            "fresh Channel<T>",
            &UnresolvedGenericFacts {
                declared_type_names: HashSet::from(["Channel".to_owned()]),
                active_generic_names: HashSet::from(["T".to_owned()]),
            }
        ));
        assert!(contains_unresolved_generic_type(
            "List<U>",
            &UnresolvedGenericFacts::default()
        ));
    }
}
