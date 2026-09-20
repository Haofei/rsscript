//! Canonical call argument-shape and effect diagnostics.

use std::collections::HashSet;

use rsscript_diagnostics::{Diagnostic, FixEdit, Span, code};

/// Resolved, syntax-independent information about one source call argument.
#[derive(Debug, Clone)]
pub struct CallArgumentFact {
    pub explicit_name: bool,
    pub resolved_name: Option<String>,
    pub span: Span,
    /// The argument's value, with any leading effect keyword excluded.
    pub value_span: Span,
    pub constructor_shorthand: bool,
    /// `None` is the source language's implicit `read` effect.
    pub effect: Option<&'static str>,
    /// The `mut`/`take`/`read` keyword actually written in front of the value.
    ///
    /// `None` means no keyword is present to rewrite: either the argument is
    /// bare, or its effect was inferred from a form that spells no keyword at
    /// all (receiver-call shorthand). Keeping the keyword separate from
    /// [`Self::value_span`] is what lets a fix *replace* or *remove* a wrong
    /// effect instead of only inserting a new one in front of it.
    pub effect_span: Option<Span>,
}

/// The call-relevant subset of a resolved function parameter.
#[derive(Debug, Clone)]
pub struct CallParameterFact {
    /// Whether this parameter may be supplied by an explicit source argument.
    /// Receiver slots are supplied by receiver-call syntax instead.
    pub accepts_argument: bool,
    /// Whether this parameter must be supplied by an explicit source argument.
    pub required: bool,
    pub name: String,
    pub effect: Option<&'static str>,
}

/// Resolved facts for receiver-call shorthand's implicit `self` argument.
#[derive(Debug, Clone)]
pub struct ReceiverCallEffectFact {
    pub callee_display: String,
    pub method: String,
    pub receiver_label: String,
    pub supplied_effect: &'static str,
    pub receiver_parameter_declared: bool,
    pub expected_effect: Option<&'static str>,
    pub span: Span,
}

/// Diagnose the implicit receiver's data-effect contract.
pub fn receiver_call_effect_diagnostics(fact: &ReceiverCallEffectFact) -> Vec<Diagnostic> {
    if !fact.receiver_parameter_declared {
        return vec![Diagnostic::error(
            code::UNKNOWN_CALLEE,
            format!(
                "receiver-call `{}` does not resolve to a method with a receiver parameter.",
                fact.callee_display
            ),
            fact.span.clone(),
            "receiver method required",
        )
        .with_cause(
            "Receiver-call shorthand expands to a qualified call and requires the resolved function to declare the receiver as its first parameter.",
        )
        .with_fix(
            "use_qualified_call",
            format!(
                "Call `{}` in qualified form, or put the receiver parameter first in its signature.",
                fact.method
            ),
            "manual",
        )];
    }
    let Some(expected) = fact.expected_effect else {
        return vec![Diagnostic::error(
            code::MISSING_DATA_EFFECT,
            format!(
                "receiver `{}` for `{}` has no declared `self` effect to match.",
                fact.receiver_label, fact.method
            ),
            fact.span.clone(),
            "missing receiver effect",
        )
        .with_cause(
            "Receiver-call shorthand requires the resolved method to declare `self: read|mut|take ...` so the call-site effect can be checked.",
        )
        .with_fix(
            "add_self_effect",
            "Declare the method receiver as `self: read ...`, `self: mut ...`, or `self: take ...`.",
            "manual",
        )];
    };
    if expected == fact.supplied_effect {
        return Vec::new();
    }
    vec![Diagnostic::error(
        code::MISSING_DATA_EFFECT,
        format!(
            "receiver `{}` for `{}` uses `{}` but the method requires `{}`.",
            fact.receiver_label, fact.method, fact.supplied_effect, expected
        ),
        fact.span.clone(),
        "receiver effect mismatch",
    )
    .with_cause(
        "Receiver-call shorthand is only valid when the visible receiver effect exactly matches the resolved method's `self` effect.",
    )
    // The span names the receiver alone, so an inserted effect keyword would
    // be correct only when the source spells no effect there — and `read` is
    // spellable, which this fact cannot distinguish from the implicit default.
    // The repair is described rather than claimed to be applicable unseen.
    .with_fix(
        "match_receiver_effect",
        format!(
            "Write `{} {}.{}(...)`.",
            expected, fact.receiver_label, fact.method
        ),
        "manual",
    )]
}

/// Diagnose call argument naming, completeness, and data-effect mismatches.
///
/// Call resolution remains the compiler's responsibility. This function consumes
/// its resolved facts so every downstream consumer can share the same semantic
/// rules without depending on compiler HIR internals.
pub fn call_argument_diagnostics(
    call_name: &str,
    call_span: &Span,
    allow_positional: bool,
    params: &[CallParameterFact],
    args: &[CallArgumentFact],
) -> Vec<Diagnostic> {
    let names = params
        .iter()
        .filter(|param| param.accepts_argument)
        .map(|param| param.name.as_str())
        .collect::<HashSet<_>>();
    let mut diagnostics = Vec::new();

    for arg in args {
        if !arg.explicit_name && !allow_positional && !arg.constructor_shorthand {
            diagnostics.push(
                Diagnostic::error(
                    code::UNNAMED_ARGUMENT,
                    format!("call to `{call_name}` uses an unnamed argument."),
                    arg.span.clone(),
                    "argument must be named",
                )
                .with_cause("Public, core, native, constructor, and protocol calls require named arguments. Constructor shorthand is only allowed for a bare identifier that matches a field name; positional arguments are only allowed for private helper calls and receiver-call shorthand.")
                .with_fix(
                    "add_argument_name",
                    "Write the argument as `name: value`.",
                    "manual",
                ),
            );
        }
    }

    let provided_names = args
        .iter()
        .filter_map(|arg| arg.resolved_name.as_deref())
        .collect::<HashSet<_>>();

    let mut seen_names = HashSet::new();
    let mut seen_positional_params = HashSet::new();
    for (argument_index, arg) in args.iter().enumerate() {
        let Some(name) = arg.resolved_name.as_deref() else {
            continue;
        };
        if !arg.explicit_name && !seen_positional_params.insert(name) {
            continue;
        }
        if !seen_names.insert(name) {
            diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_ARGUMENT,
                    format!("call to `{call_name}` repeats argument `{name}`."),
                    arg.span.clone(),
                    "duplicate argument",
                )
                .with_cause("Each named parameter can be provided at most once.")
                .with_fix(
                    "remove_duplicate_argument",
                    format!("Remove the extra `{name}: ...` argument."),
                    "manual",
                ),
            );
        }
        if !names.contains(name) {
            let declared = params
                .iter()
                .filter(|param| param.accepts_argument)
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let diagnostic = Diagnostic::error(
                code::UNKNOWN_ARGUMENT,
                format!("call to `{call_name}` has no argument named `{name}`."),
                arg.span.clone(),
                "unknown argument",
            )
            .with_cause(format!(
                "`{call_name}` does not declare a parameter named `{name}`."
            ));
            diagnostics.push(
                match nearest_parameter(name, argument_index, params, args, &provided_names) {
                    Some(target) => diagnostic.with_fix_edit(
                        "rename_argument",
                        format!("Did you mean `{target}`? Declared: {declared}."),
                        FixEdit::replace(&arg.span, target),
                    ),
                    None => diagnostic.with_fix(
                        "rename_argument",
                        format!("Use one of: {declared}."),
                        "manual",
                    ),
                },
            );
        }
    }

    for param in params.iter().filter(|param| param.required) {
        if !provided_names.contains(param.name.as_str()) {
            diagnostics.push(
                Diagnostic::error(
                    code::MISSING_ARGUMENT,
                    format!(
                        "call to `{call_name}` is missing required argument `{}`.",
                        param.name
                    ),
                    call_span.clone(),
                    "missing argument",
                )
                .with_cause(format!(
                    "`{call_name}` requires a named argument `{}`.",
                    param.name
                ))
                .with_fix(
                    "add_argument",
                    format!("Add `{}: ...` to the call.", param.name),
                    "manual",
                ),
            );
        }
    }

    for arg in args {
        let Some(name) = arg.resolved_name.as_deref() else {
            continue;
        };
        let Some(expected) = params
            .iter()
            .find(|param| param.name == name)
            .and_then(|param| param.effect)
        else {
            continue;
        };
        if expected == "read" && arg.effect.is_none() {
            continue;
        }
        if arg.effect != Some(expected) {
            let written = arg.effect.unwrap_or("read");
            let summary = if expected == "read" {
                format!(
                    "argument `{name}` for `{call_name}` uses `{written}` but the parameter is `read`."
                )
            } else {
                format!("argument `{name}` for `{call_name}` must use `{expected}`.")
            };
            let title = if expected == "read" {
                format!(
                    "Remove `{written}` from `{name}`: a `read` parameter takes the bare value."
                )
            } else {
                format!("Write `{name}: {expected} ...` at the call site.")
            };
            let diagnostic = Diagnostic::error(
                code::MISSING_DATA_EFFECT,
                summary,
                arg.effect_span.clone().unwrap_or_else(|| arg.value_span.clone()),
                "data effect mismatch",
            )
            .with_cause("A bare argument is `read`; `mut` and `take` must be written explicitly and match the parameter.");
            diagnostics.push(match data_effect_edit(arg, expected) {
                Some(edit) => diagnostic.with_fix_edit("match_data_effect", title, edit),
                None => diagnostic.with_fix("match_data_effect", title, "manual"),
            });
        }
    }

    diagnostics
}

/// The declared parameter that a wrong argument label most plausibly meant.
///
/// `RS0206` gained a did-you-mean because the measured repair data separates
/// diagnostics that name the edit from diagnostics that only name the failure:
/// the first get applied, the second get re-invented. `RS0203` was in the
/// second group — it listed every declared parameter and left the choice open.
/// It is also charged twice, because a label the callee does not declare is
/// both an unknown argument and a required parameter left unfilled, so one
/// rename clears two errors.
///
/// Measured wrong labels are almost never typos. They are other languages'
/// names for the same slot — `text` for `value`, `separator` for `delimiter`,
/// `a`/`b` for `left`/`right`, `end` for `len` — which edit distance cannot
/// reach and position gets right every time, because the model orders the
/// arguments correctly and only misnames them. So when the call has the shape
/// of a complete named call — every argument labelled, exactly as many
/// arguments as the callee accepts — the parameter standing in this argument's
/// own position is the answer. Otherwise the older evidence still applies: a
/// truncation or near-miss (`init` for `initial`, `lst` for `list`) is offered
/// when it is unambiguous, and nothing is offered when it is not.
fn nearest_parameter<'a>(
    written: &str,
    argument_index: usize,
    params: &'a [CallParameterFact],
    args: &[CallArgumentFact],
    provided: &HashSet<&str>,
) -> Option<&'a str> {
    let accepting: Vec<&'a str> = params
        .iter()
        .filter(|param| param.accepts_argument)
        .map(|param| param.name.as_str())
        .collect();

    // Right shape, wrong names: every slot is filled and labelled, so the
    // parameter standing in this argument's position is unambiguous.
    let complete_named_call =
        args.len() == accepting.len() && args.iter().all(|arg| arg.explicit_name);
    if complete_named_call
        && let Some(positional) = accepting.get(argument_index)
        && !provided.contains(positional)
    {
        return Some(positional);
    }

    let budget = match written.chars().count() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    let mut scored: Vec<(usize, usize, &str)> = accepting
        .iter()
        .filter(|name| !provided.contains(*name))
        .filter_map(|name| {
            let prefix = name.starts_with(written) || written.starts_with(*name);
            let distance = edit_distance(written, name);
            (prefix || distance <= budget).then_some((usize::from(!prefix), distance, *name))
        })
        .collect();
    scored.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(right.2))
    });
    // A tie between two equally plausible parameters is not a suggestion; the
    // declared list is the honest answer there.
    match scored.as_slice() {
        [(_, _, name)] => Some(name),
        [(rank, distance, name), (next_rank, next_distance, _), ..]
            if (rank, distance) != (next_rank, next_distance) =>
        {
            Some(name)
        }
        _ => None,
    }
}

/// Levenshtein distance, two rows at a time.
///
/// Deliberately local: this module takes its inputs as resolved facts and
/// depends on nothing but the diagnostic types, which is what lets other
/// consumers share these rules without linking the compiler's symbol tables.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// The concrete edit that turns an argument's written effect into `expected`.
///
/// There are three shapes and only the first used to be handled: a bare
/// argument *gains* the keyword, a wrongly spelled keyword is *replaced*, and a
/// keyword in front of a `read` parameter is *removed*, because `read` is
/// canonical by omission. Inserting in front of an argument that already
/// carries an effect produced `take mut value`, which does not parse — a
/// machine-applicable fix that makes the file worse is worse than no fix, and
/// the measured repair data shows models do apply what this fix names.
fn data_effect_edit(arg: &CallArgumentFact, expected: &str) -> Option<FixEdit> {
    let Some(written) = arg.effect_span.as_ref() else {
        // Nothing in the source spells an effect keyword here. A bare argument
        // simply gains one; an effect read off a form with no keyword at all
        // (receiver-call shorthand) has nothing to rewrite, so it stays advice.
        return (arg.effect.is_none() && expected != "read")
            .then(|| FixEdit::insert_before(&arg.value_span, format!("{expected} ")));
    };
    if expected != "read" {
        return Some(FixEdit::replace(written, expected));
    }
    // Deleting the keyword must also delete the whitespace that followed it, so
    // the span runs from the keyword up to the value. Only a value on the
    // keyword's own line has a length this column arithmetic can express.
    (written.line == arg.value_span.line && arg.value_span.column > written.column).then(|| {
        FixEdit::replace(
            &Span {
                length: arg.value_span.column - written.column,
                ..written.clone()
            },
            "",
        )
    })
}

/// Diagnose a return value that does not match its resolved declared type.
pub fn return_type_mismatch_diagnostic(
    function_name: &str,
    actual: &str,
    expected: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::RETURN_TYPE_MISMATCH,
        format!("return in `{function_name}` has type `{actual}`, expected `{expected}`."),
        span,
        "return type mismatch",
    )
    .with_cause(
        "RSScript return types are part of the review contract and must be checked before Rust lowering.",
    )
    .with_fix(
        "match_return_type",
        format!("Return a value of type `{expected}` here."),
        "manual",
    )
}

/// Diagnose a `Result` or `Option` return constructor payload type mismatch.
pub fn return_payload_type_mismatch_diagnostic(
    function_name: &str,
    actual: &str,
    expected: &str,
    label: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::RETURN_TYPE_MISMATCH,
        format!("{label} in `{function_name}` has type `{actual}`, expected `{expected}`."),
        span,
        "return type mismatch",
    )
    .with_cause(
        "Result and Option return constructors are checked against the declared return payload before Rust lowering.",
    )
    .with_fix(
        "match_return_payload_type",
        format!("Return a `{expected}` payload here."),
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "call.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn reports_shape_and_effect_diagnostics_from_resolved_facts() {
        let params = [
            CallParameterFact {
                accepts_argument: false,
                required: false,
                name: "self".to_owned(),
                effect: Some("mut"),
            },
            CallParameterFact {
                accepts_argument: true,
                required: true,
                name: "item".to_owned(),
                effect: Some("take"),
            },
            CallParameterFact {
                accepts_argument: true,
                required: true,
                name: "count".to_owned(),
                effect: Some("read"),
            },
        ];
        let args = [
            CallArgumentFact {
                explicit_name: false,
                resolved_name: None,
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
                effect_span: None,
            },
            CallArgumentFact {
                explicit_name: true,
                resolved_name: Some("item".to_owned()),
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
                effect_span: None,
            },
            CallArgumentFact {
                explicit_name: true,
                resolved_name: Some("item".to_owned()),
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
                effect_span: None,
            },
            CallArgumentFact {
                explicit_name: true,
                resolved_name: Some("unknown".to_owned()),
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
                effect_span: None,
            },
        ];

        let diagnostics = call_argument_diagnostics("push", &span(), false, &params, &args);
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();

        assert!(codes.contains(&code::UNNAMED_ARGUMENT));
        assert!(codes.contains(&code::DUPLICATE_ARGUMENT));
        assert!(codes.contains(&code::UNKNOWN_ARGUMENT));
        assert!(codes.contains(&code::MISSING_ARGUMENT));
        assert_eq!(
            codes
                .iter()
                .filter(|value| **value == code::MISSING_DATA_EFFECT)
                .count(),
            2
        );
    }

    fn parameter(name: &str, effect: Option<&'static str>) -> CallParameterFact {
        CallParameterFact {
            accepts_argument: true,
            required: true,
            name: name.to_owned(),
            effect,
        }
    }

    fn named_argument(name: &str) -> CallArgumentFact {
        CallArgumentFact {
            explicit_name: true,
            resolved_name: Some(name.to_owned()),
            span: span(),
            value_span: span(),
            constructor_shorthand: false,
            effect: None,
            effect_span: None,
        }
    }

    fn rename_suggestions(params: &[CallParameterFact], labels: &[&str]) -> Vec<Option<String>> {
        let args: Vec<CallArgumentFact> = labels.iter().map(|name| named_argument(name)).collect();
        call_argument_diagnostics("call", &span(), false, params, &args)
            .into_iter()
            .filter(|diagnostic| diagnostic.code == code::UNKNOWN_ARGUMENT)
            .map(|diagnostic| {
                diagnostic
                    .fixes
                    .iter()
                    .find(|fix| fix.applicability == "machine-applicable")
                    .and_then(|fix| fix.edit.as_ref())
                    .map(|edit| edit.replacement.clone())
            })
            .collect()
    }

    /// A wrong argument label is another language's name for the same slot far
    /// more often than it is a typo, so position decides when the call is
    /// otherwise complete, and the near-miss rule covers the rest.
    #[test]
    fn unknown_argument_names_the_parameter_in_its_own_position() {
        let three = [
            parameter("value", Some("read")),
            parameter("start", Some("read")),
            parameter("len", Some("read")),
        ];
        // `start` is spelled correctly; the other two are renamed by position
        // even though neither is anywhere near its target by edit distance.
        assert_eq!(
            rename_suggestions(&three, &["text", "start", "end"]),
            [Some("value".to_owned()), Some("len".to_owned())]
        );

        // Not a complete named call: one slot is left unfilled, so position
        // proves nothing and only an unambiguous near-miss is offered.
        let two = [
            parameter("initial", Some("read")),
            parameter("folder", None),
        ];
        assert_eq!(
            rename_suggestions(&two, &["init"]),
            [Some("initial".to_owned())]
        );
        assert_eq!(rename_suggestions(&two, &["op"]), [None]);
    }

    #[test]
    fn receiver_effect_fact_preserves_receiver_contract_diagnostics() {
        let mismatch = ReceiverCallEffectFact {
            callee_display: "List.push".to_owned(),
            method: "push".to_owned(),
            receiver_label: "items".to_owned(),
            supplied_effect: "read",
            receiver_parameter_declared: true,
            expected_effect: Some("mut"),
            span: span(),
        };
        let diagnostics = receiver_call_effect_diagnostics(&mismatch);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::MISSING_DATA_EFFECT);
        assert!(diagnostics[0].summary.contains("requires `mut`"));

        let missing_parameter = ReceiverCallEffectFact {
            receiver_parameter_declared: false,
            ..mismatch
        };
        let diagnostics = receiver_call_effect_diagnostics(&missing_parameter);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::UNKNOWN_CALLEE);
    }

    #[test]
    fn derives_return_type_diagnostics_from_resolved_types() {
        assert_eq!(
            return_type_mismatch_diagnostic("build", "String", "Int", span()).code,
            code::RETURN_TYPE_MISMATCH
        );
        assert_eq!(
            return_payload_type_mismatch_diagnostic("build", "String", "Int", "Ok payload", span())
                .code,
            code::RETURN_TYPE_MISMATCH
        );
    }
}
