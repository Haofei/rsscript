//! Diagnostics for compiler-resolved type compatibility facts.

use std::collections::HashSet;

use rsscript_diagnostics::{Diagnostic, FixEdit, Span, code};

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
    contains_named_generic(
        type_name,
        &generic_names.iter().map(String::as_str).collect(),
    )
}

fn contains_named_generic(type_name: &str, generic_names: &HashSet<&str>) -> bool {
    let root = crate::type_root_name(type_name);
    generic_names.contains(root)
        || type_name
            .trim()
            .strip_prefix("fresh ")
            .is_some_and(|target| contains_named_generic(target.trim(), generic_names))
        || crate::type_arg_names(type_name).is_some_and(|arguments| {
            arguments
                .iter()
                .any(|argument| contains_named_generic(argument, generic_names))
        })
        || function_return_type(type_name)
            .is_some_and(|return_type| contains_named_generic(return_type, generic_names))
        || function_parameter_types(type_name)
            .iter()
            .any(|parameter| contains_named_generic(parameter, generic_names))
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

/// Split the text after `Fn(` at the parenthesis that actually closes the
/// parameter list, into `(parameters, everything after the `)`)`.
///
/// `str::split_once(')')` stops at the *first* `)`, which is the wrong one as
/// soon as a parameter is itself a function type: for
/// `Fn(read Fn(read Int) -> Int, read Int) -> Int` it cuts the list at
/// `read Fn(read Int`. Every consumer then counted, compared and — worst —
/// *rendered* that fragment, so `RS0207` reported an expected type of
/// `Fn(read Int` and a parameter count of one for a two-parameter callback.
///
/// The `>` of an arrow closes nothing, so it is skipped rather than treated as
/// the end of a generic argument list.
pub(crate) fn split_function_type_body(body: &str) -> Option<(&str, &str)> {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    for (index, character) in body.char_indices() {
        match character {
            '(' | '<' => depth += 1,
            '>' if index > 0 && bytes[index - 1] == b'-' => {}
            '>' => depth = depth.saturating_sub(1),
            ')' if depth == 0 => return Some((&body[..index], &body[index + 1..])),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn function_return_type(type_name: &str) -> Option<&str> {
    function_body(type_name)
        .and_then(split_function_type_body)
        .and_then(|(_, rest)| rest.trim_start().strip_prefix("->"))
        .map(str::trim)
}

fn function_parameter_types(type_name: &str) -> Vec<&str> {
    let Some(params) = function_body(type_name)
        .and_then(split_function_type_body)
        .map(|(params, _)| params.trim())
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
        "Explicit `let` and `local` type annotations are source-level contracts, and the checker requires the initializer to match them.",
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
        "Result and Option binding initializers are checked against explicit binding payload types before the program is built.",
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
        "Result and Option argument constructors are checked against the resolved parameter payload before the program is built.",
    )
    .with_fix(
        "match_argument_payload_type",
        format!("Pass a `{expected}` payload for `{arg_name}`."),
        "manual",
    )
}

/// A mechanical, safe repair for one `RS0207` argument.
///
/// Measured on 2026-09-19: `RS0207` is the one class haiku's repair loop
/// *introduces* more often than it clears — five against one — and the reason
/// is the shape this report named for every other class: a diagnostic that
/// names the expected type and no replacement is the shape that persists.
/// Every variant here is a case where the replacement is derivable from facts
/// the checker already has; anything else keeps the advisory fix it always had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentTypeRepair {
    /// The argument is a `Result<T, E>` or `Option<T>` where `T` is wanted, and
    /// the enclosing function can propagate the failure case under the `RS0013`
    /// rules. Appending `?` is the whole edit.
    PropagateWithTry {
        /// The argument expression, whose end is where `?` goes.
        operand: Span,
        /// `Result` or `Option`, for the help line.
        container: &'static str,
    },
    /// An `Int` literal where a `Float` is wanted. `7` becomes `7.0`.
    IntLiteralToFloat {
        /// The literal's own span, replaced wholesale.
        literal: Span,
        /// The literal exactly as written.
        text: String,
    },
    /// A `String` literal where an `Int` is wanted. There is deliberately no
    /// edit: `"12"` and `"twelve"` are the same shape here and only one of them
    /// has a value, so the repair is a parse whose failure case the caller has
    /// to handle. Naming the function is as far as this can safely go.
    ParseStringLiteral,
}

pub fn argument_type_mismatch_diagnostic(
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    argument_type_mismatch_diagnostic_with_repair(call_name, arg_name, actual, expected, span, None)
}

/// `RS0207` with the replacement attached where one is mechanical and safe.
///
/// The advisory `match_argument_type` fix is always present and unchanged, so a
/// consumer that reads only it sees exactly what it saw before. A repair adds a
/// second, more specific fix in front of it, and only the first two variants of
/// [`ArgumentTypeRepair`] carry an edit.
pub fn argument_type_mismatch_diagnostic_with_repair(
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: Span,
    repair: Option<ArgumentTypeRepair>,
) -> Diagnostic {
    let diagnostic = Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("argument `{arg_name}` for `{call_name}` has type `{actual}`, expected `{expected}`."),
        span,
        "argument type mismatch",
    )
    .with_cause("RSScript call argument types must match the resolved callee signature before backend lowering.");

    let diagnostic = match repair {
        Some(ArgumentTypeRepair::PropagateWithTry { operand, container }) => diagnostic
            .with_cause(format!(
                "`{arg_name}` is a `{container}` that has not been unwrapped; `?` propagates its failure case out of this function."
            ))
            .with_fix_edit(
                "propagate_with_try",
                format!("Append `?` to hand the failure case back to the caller and pass the `{expected}`."),
                FixEdit::insert_after(&operand, "?"),
            ),
        Some(ArgumentTypeRepair::IntLiteralToFloat { literal, text }) => diagnostic.with_fix_edit(
            "widen_int_literal_to_float",
            format!("Write `{text}.0`: a `Float` parameter takes a float literal."),
            FixEdit::replace(&literal, format!("{text}.0")),
        ),
        Some(ArgumentTypeRepair::ParseStringLiteral) => diagnostic.with_fix(
            "parse_string_literal",
            format!(
                "`String.parse_int(value: ...)` returns `Option<Int>`, not `{expected}`; bind it with `let Some(...) = ... else` or `Option.unwrap_or` before passing `{arg_name}`."
            ),
            "manual",
        ),
        None => diagnostic,
    };

    diagnostic.with_fix(
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
        "The {context} is typed as a `Map`, so every map literal {role} must match the corresponding `Map` type argument; the checker enforces this before the program is built."
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
        "The {context} is typed as a `List`, so every array literal item must match the `List` item type; the checker enforces this before the program is built."
    ))
    .with_fix(
        "match_list_literal_item_type",
        format!("Use a `{expected}` value for this list literal item."),
        "manual",
    )
}

pub fn unknown_callee_diagnostic(call_name: &str, span: Span) -> Diagnostic {
    unknown_callee_diagnostic_with_suggestions(call_name, span, &[], None)
}

/// `RS0206` with the replacement named, not just the failure.
///
/// The measured repair data is unambiguous: diagnostics whose message names the
/// edit get fixed and never persist, while `RS0206` — which said only that a
/// call "does not resolve" — persisted through three repair turns nine times,
/// because a model told twice that `print` does not exist simply re-invents it.
/// `suggestions` are in-scope names computed by
/// [`crate::unresolved_call_suggestions`].
///
/// `rename_span` is the exact source range covering the written callee. When it
/// is present and the best suggestion is a pure rename, the fix carries a
/// machine-applicable edit so `rss fix --json` can apply it; otherwise the
/// suggestion is advisory help.
pub fn unknown_callee_diagnostic_with_suggestions(
    call_name: &str,
    span: Span,
    suggestions: &[crate::NameSuggestion],
    rename_span: Option<Span>,
) -> Diagnostic {
    let diagnostic = Diagnostic::error(
        code::UNKNOWN_CALLEE,
        format!("call to `{call_name}` does not resolve."),
        span,
        "unknown callee",
    )
    .with_cause(
        "The callee is not a user function, known type constructor, enum variant, or builtin signature.",
    );

    let Some(best) = suggestions.first() else {
        return diagnostic.with_fix(
            "declare_or_import_callee",
            "Declare the function or add a builtin signature for this API.",
            "manual",
        );
    };

    // The best candidate is shown as a whole signature, not as a bare name.
    // Measured on 2026-09-19: both models take the rename they are offered
    // (sonnet 87%, haiku 74%) and then fail on the *arguments* of the call the
    // compiler just named, because the name was all it named. The remaining
    // candidates stay bare so the one answer stays legible.
    let best_display = best.signature.clone().unwrap_or_else(|| best.name.clone());
    let alternatives = suggestions
        .iter()
        .skip(1)
        .map(|suggestion| format!("`{}`", suggestion.name))
        .collect::<Vec<_>>();
    let title = if alternatives.is_empty() {
        format!("Did you mean `{best_display}`?")
    } else {
        format!(
            "Did you mean `{best_display}`? Also in scope: {}.",
            alternatives.join(", ")
        )
    };

    let diagnostic = match rename_span.filter(|_| best.pure_rename) {
        Some(rename_span) => diagnostic.with_fix_edit(
            "rename_callee",
            title,
            FixEdit::replace(&rename_span, best.name.clone()),
        ),
        None => diagnostic.with_fix("rename_callee", title, "maybe-incorrect"),
    };
    diagnostic.with_fix(
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

    /// `RS0206` names the call, not just the callee.
    ///
    /// Measured on 2026-09-19: sonnet applied 55 of 63 offered renames and
    /// haiku 53 of 72, and the turn *after* the rename routinely failed on the
    /// arguments of the call the compiler had just named. A name answers "what
    /// exists"; the signature answers "what do I write".
    #[test]
    fn unknown_callee_help_carries_the_best_candidates_whole_signature() {
        let best = crate::NameSuggestion {
            name: "String.parse_int".to_owned(),
            source: crate::SuggestionSource::KnownAlias,
            pure_rename: true,
            signature: Some("String.parse_int(value: String) -> Option<Int>".to_owned()),
        };
        let other = crate::NameSuggestion {
            name: "Json.parse".to_owned(),
            source: crate::SuggestionSource::EditDistance,
            pure_rename: true,
            signature: Some("Json.parse(text: String) -> Result<fresh Json, JsonError>".to_owned()),
        };

        let diagnostic = unknown_callee_diagnostic_with_suggestions(
            "Int.parse",
            span(),
            std::slice::from_ref(&best),
            Some(span()),
        );
        let fix = diagnostic
            .fixes
            .iter()
            .find(|fix| fix.kind == "rename_callee")
            .expect("a rename fix");
        assert_eq!(
            fix.title,
            "Did you mean `String.parse_int(value: String) -> Option<Int>`?"
        );
        // The fix contract is unchanged: the edit still replaces the written
        // callee with the bare name, so `rss fix` produces the same source it
        // produced before the title grew.
        assert_eq!(fix.applicability, "machine-applicable");
        assert_eq!(
            fix.edit.as_ref().map(|edit| edit.replacement.as_str()),
            Some("String.parse_int")
        );

        // Runners-up stay bare so the one answer stays legible.
        let diagnostic =
            unknown_callee_diagnostic_with_suggestions("Int.parse", span(), &[best, other], None);
        let fix = diagnostic
            .fixes
            .iter()
            .find(|fix| fix.kind == "rename_callee")
            .expect("a rename fix");
        assert_eq!(
            fix.title,
            "Did you mean `String.parse_int(value: String) -> Option<Int>`? Also in scope: `Json.parse`."
        );
        assert_eq!(fix.applicability, "maybe-incorrect");
        assert!(fix.edit.is_none());
    }

    /// A suggestion the caller could not resolve a signature for still names
    /// the replacement, rather than rendering an empty call.
    #[test]
    fn unknown_callee_help_falls_back_to_the_bare_name_without_a_signature() {
        let diagnostic = unknown_callee_diagnostic_with_suggestions(
            "Reprt",
            span(),
            &[crate::NameSuggestion {
                name: "Report".to_owned(),
                source: crate::SuggestionSource::EditDistance,
                pure_rename: true,
                signature: None,
            }],
            None,
        );
        assert!(
            diagnostic
                .fixes
                .iter()
                .any(|fix| fix.title == "Did you mean `Report`?"),
            "{:#?}",
            diagnostic.fixes
        );
    }

    /// `RS0207` carries the replacement where one is mechanical, and never
    /// where one is not.
    ///
    /// Measured on 2026-09-19: haiku's repair loop introduces `RS0207` five
    /// times for every one it clears, and `RS0207` was the only large class
    /// whose fix was `manual` and named no replacement.
    #[test]
    fn argument_type_mismatch_carries_a_repair_where_one_is_mechanical() {
        let advisory = argument_type_mismatch_diagnostic("call", "value", "String", "Int", span());
        assert_eq!(advisory.fixes.len(), 1);
        assert_eq!(advisory.fixes[0].kind, "match_argument_type");
        assert_eq!(advisory.fixes[0].applicability, "manual");

        let propagate = argument_type_mismatch_diagnostic_with_repair(
            "use_json",
            "value",
            "Result<JsonValue, JsonError>",
            "JsonValue",
            span(),
            Some(ArgumentTypeRepair::PropagateWithTry {
                operand: Span {
                    file: "types.rss".to_owned(),
                    line: 3,
                    column: 20,
                    length: 7,
                },
                container: "Result",
            }),
        );
        let fix = &propagate.fixes[0];
        assert_eq!(fix.kind, "propagate_with_try");
        assert_eq!(fix.applicability, "machine-applicable");
        let edit = fix.edit.as_ref().expect("a machine-applicable edit");
        assert_eq!(edit.replacement, "?");
        // A pure insertion immediately after the operand: nothing is removed,
        // so the argument text itself is untouched.
        assert_eq!(edit.span.length, 0);
        assert_eq!(edit.span.column, 27);
        assert_eq!(edit.span.line, 3);
        // The advisory fix is still there, unchanged, behind the specific one.
        assert_eq!(propagate.fixes[1].kind, "match_argument_type");

        let widen = argument_type_mismatch_diagnostic_with_repair(
            "scale",
            "ratio",
            "Int",
            "Float",
            span(),
            Some(ArgumentTypeRepair::IntLiteralToFloat {
                literal: Span {
                    file: "types.rss".to_owned(),
                    line: 1,
                    column: 9,
                    length: 1,
                },
                text: "7".to_owned(),
            }),
        );
        assert_eq!(widen.fixes[0].kind, "widen_int_literal_to_float");
        let edit = widen.fixes[0].edit.as_ref().expect("an edit");
        assert_eq!(edit.replacement, "7.0");
        assert_eq!(edit.span.length, 1);

        // A `String` literal where an `Int` is wanted is advice on purpose:
        // `"twelve"` has the same shape as `"12"` and no value, so there is no
        // edit that is safe to apply unseen.
        let parse = argument_type_mismatch_diagnostic_with_repair(
            "count",
            "value",
            "String",
            "Int",
            span(),
            Some(ArgumentTypeRepair::ParseStringLiteral),
        );
        assert_eq!(parse.fixes[0].kind, "parse_string_literal");
        assert_eq!(parse.fixes[0].applicability, "manual");
        assert!(parse.fixes[0].edit.is_none());
        assert!(parse.fixes[0].title.contains("String.parse_int"));
    }

    /// A `Fn(...)` parameter list ends at the parenthesis that closes it.
    ///
    /// `split_once(')')` cut the list at the first `)`, so a callback whose own
    /// parameter is a function type was rendered — in the `RS0207` message the
    /// caller reads — as `Fn(read Int`, and counted as one parameter short.
    #[test]
    fn a_function_types_parameter_list_is_split_at_its_own_closing_paren() {
        assert_eq!(
            split_function_type_body("read Fn(read Int) -> Int, read Int) -> Int"),
            Some(("read Fn(read Int) -> Int, read Int", " -> Int"))
        );
        // The `>` of an arrow closes nothing, even inside a generic argument.
        assert_eq!(
            split_function_type_body("read List<Fn(read Int) -> Int>) -> Bool"),
            Some(("read List<Fn(read Int) -> Int>", " -> Bool"))
        );
        assert_eq!(
            split_function_type_body("read Map<String, Int>) -> Int"),
            Some(("read Map<String, Int>", " -> Int"))
        );
        assert_eq!(split_function_type_body(") -> Int"), Some(("", " -> Int")));
        assert_eq!(split_function_type_body("read Int -> Int"), None);

        // The whole point: the rendered types stay whole.
        assert_eq!(
            function_parameter_types("noescape Fn(read Fn(read Int) -> Int, read Int) -> Int"),
            vec!["Fn(read Int) -> Int", "Int"]
        );
        assert_eq!(
            function_return_type("noescape Fn(read Fn(read Int) -> Int, read Int) -> Int"),
            Some("Int")
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
        assert!(type_contains_unresolved_generic(
            "List<T>",
            &["T".to_owned()]
        ));
        assert!(!type_contains_unresolved_generic("T", &[]));
    }
}
