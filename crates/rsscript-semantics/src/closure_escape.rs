//! Canonical diagnostics for resolved closure escape facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

/// The resolved operation through which a callback would escape its owner.
#[derive(Debug, Clone, Copy)]
pub enum ClosureEscapeContext<'a> {
    Store,
    Return,
    UseAsValue,
    Pass { callee: &'a str },
}

/// Diagnose an escaping `noescape` callback.
pub fn noescape_escape_diagnostic(
    name: &str,
    use_span: Span,
    context_span: Span,
    context: ClosureEscapeContext<'_>,
) -> Diagnostic {
    let (summary, cause) = match context {
        ClosureEscapeContext::Store => (
            format!("noescape callback `{name}` cannot be stored."),
            "`noescape Fn()` parameters are temporary callback external_bindings and cannot be bound into stored values.".to_string(),
        ),
        ClosureEscapeContext::Return => (
            format!("noescape callback `{name}` cannot be returned."),
            "`noescape Fn()` parameters cannot escape the current function through a return value.".to_string(),
        ),
        ClosureEscapeContext::UseAsValue => (
            format!("noescape callback `{name}` cannot be used as an ordinary value."),
            "Call the noescape callback directly, or pass it to another resolved `noescape Fn()` parameter.".to_string(),
        ),
        ClosureEscapeContext::Pass { callee } => (
            format!("noescape callback `{name}` cannot be passed to `{callee}` as an ordinary value."),
            "Forwarding a noescape callback is only allowed when the target parameter is also `noescape Fn()`.".to_string(),
        ),
    };
    Diagnostic::error(
        code::NOESCAPE_CALLBACK_ESCAPE,
        summary,
        use_span,
        "noescape callback escapes",
    )
    .with_cause(cause)
    .with_cause(format!(
        "The escaping context starts at {}:{}.",
        context_span.line, context_span.column
    ))
    .with_fix(
        "keep_noescape_local",
        "Call the callback directly, or change the API to an ordinary managed callback type.",
        "manual",
    )
}

/// Diagnose an escaping local closure.
pub fn local_closure_escape_diagnostic(
    name: &str,
    use_span: Span,
    context_span: Span,
    context: ClosureEscapeContext<'_>,
) -> Diagnostic {
    let (summary, cause) = match context {
        ClosureEscapeContext::Store => (
            format!("local closure `{name}` cannot be stored in a managed binding."),
            "A closure bound with `local` is an exclusive local external_binding and cannot become managed data.".to_string(),
        ),
        ClosureEscapeContext::Return => (
            format!("local closure `{name}` cannot be returned."),
            "A local closure cannot escape the function where its local captures are valid.".to_string(),
        ),
        ClosureEscapeContext::UseAsValue => (
            format!("local closure `{name}` cannot be used as an ordinary value."),
            "Call the local closure directly, or pass it to a resolved `noescape Fn()` parameter.".to_string(),
        ),
        ClosureEscapeContext::Pass { callee } => (
            format!("local closure `{name}` cannot be passed to `{callee}` as an ordinary value."),
            "Forwarding a local closure is only allowed when the target parameter is `noescape Fn()`.".to_string(),
        ),
    };
    Diagnostic::error(
        code::LOCAL_CLOSURE_ESCAPE,
        summary,
        use_span,
        "local closure escapes",
    )
    .with_cause(cause)
    .with_cause(format!(
        "The escaping context starts at {}:{}.",
        context_span.line, context_span.column
    ))
    .with_fix(
        "keep_local_closure_noescape",
        "Call the closure locally, or pass it to a noescape callback parameter.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(column: usize) -> Span {
        Span {
            file: "closure.rss".to_owned(),
            line: 1,
            column,
            length: 1,
        }
    }

    #[test]
    fn derives_escape_diagnostics_from_resolved_contexts() {
        assert_eq!(
            noescape_escape_diagnostic("callback", span(1), span(3), ClosureEscapeContext::Return)
                .code,
            code::NOESCAPE_CALLBACK_ESCAPE
        );
        assert_eq!(
            local_closure_escape_diagnostic(
                "callback",
                span(1),
                span(3),
                ClosureEscapeContext::Pass { callee: "store" }
            )
            .code,
            code::LOCAL_CLOSURE_ESCAPE
        );
    }
}
