//! Canonical call argument-shape and effect diagnostics.

use std::collections::HashSet;

use rsscript_diagnostics::{Diagnostic, FixEdit, Span, code};

/// Resolved, syntax-independent information about one source call argument.
#[derive(Debug, Clone)]
pub struct CallArgumentFact {
    pub explicit_name: bool,
    pub resolved_name: Option<String>,
    pub span: Span,
    pub value_span: Span,
    pub constructor_shorthand: bool,
    /// `None` is the source language's implicit `read` effect.
    pub effect: Option<&'static str>,
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

    let mut seen_names = HashSet::new();
    let mut seen_positional_params = HashSet::new();
    for arg in args {
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
            diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_ARGUMENT,
                    format!("call to `{call_name}` has no argument named `{name}`."),
                    arg.span.clone(),
                    "unknown argument",
                )
                .with_cause(format!(
                    "`{call_name}` does not declare a parameter named `{name}`."
                ))
                .with_fix(
                    "rename_argument",
                    format!("Use one of: {declared}."),
                    "manual",
                ),
            );
        }
    }

    let provided_names = args
        .iter()
        .filter_map(|arg| arg.resolved_name.as_deref())
        .collect::<HashSet<_>>();
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
            diagnostics.push(
                Diagnostic::error(
                    code::MISSING_DATA_EFFECT,
                    format!("argument `{name}` for `{call_name}` must use `{expected}`."),
                    arg.value_span.clone(),
                    "data effect mismatch",
                )
                .with_cause("A bare argument is `read`; `mut` and `take` must be written explicitly and match the parameter.")
                .with_fix_edit(
                    "add_data_effect",
                    format!("Write `{name}: {expected} ...` at the call site."),
                    FixEdit::insert_before(&arg.value_span, format!("{expected} ")),
                ),
            );
        }
    }

    diagnostics
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
            },
            CallArgumentFact {
                explicit_name: true,
                resolved_name: Some("item".to_owned()),
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
            },
            CallArgumentFact {
                explicit_name: true,
                resolved_name: Some("item".to_owned()),
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
            },
            CallArgumentFact {
                explicit_name: true,
                resolved_name: Some("unknown".to_owned()),
                span: span(),
                value_span: span(),
                constructor_shorthand: false,
                effect: None,
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
}
