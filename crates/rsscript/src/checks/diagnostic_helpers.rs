//! Small private constructors for the most repetitive diagnostic shapes.
//!
//! These helpers exist purely to remove construction boilerplate; they forward
//! every string verbatim and must reproduce the exact same `Diagnostic` value as
//! the inline `Diagnostic::error(...).with_cause(...).with_fix(...)` chains they
//! replace. Do not bake message/cause/fix text into them — that varies per site.

use crate::diagnostic::{Diagnostic, Span};

/// `Diagnostic::error(code, summary, span, label).with_cause(cause).with_fix(fix_kind, fix_title, fix_applicability)`.
///
/// Reproduces the single-cause / single-fix error shape that recurs across the
/// body and call checks. Callers still push the result themselves so push order
/// and the surrounding control flow are unchanged.
pub(crate) fn error_cause_fix(
    code: &str,
    summary: impl Into<String>,
    span: Span,
    label: impl Into<String>,
    cause: impl Into<String>,
    fix_kind: impl Into<String>,
    fix_title: impl Into<String>,
    fix_applicability: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(code, summary, span, label)
        .with_cause(cause)
        .with_fix(fix_kind, fix_title, fix_applicability)
}

/// Like [`error_cause_fix`] but with the `"manual"` fix applicability that the
/// overwhelming majority of these sites use.
pub(crate) fn error_cause_manual_fix(
    code: &str,
    summary: impl Into<String>,
    span: Span,
    label: impl Into<String>,
    cause: impl Into<String>,
    fix_kind: impl Into<String>,
    fix_title: impl Into<String>,
) -> Diagnostic {
    error_cause_fix(
        code,
        summary,
        span,
        label,
        cause,
        fix_kind,
        fix_title,
        "manual",
    )
}
