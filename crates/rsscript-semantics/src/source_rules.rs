//! Source-token semantic rules that do not require compiler orchestration.

use rsscript_diagnostics::{Diagnostic, Span, code};
use rsscript_syntax::lexer::{Token, TokenKind};

/// Derive diagnostics for deliberately unsupported surface forms.
///
/// These rules are token-local and platform-neutral: they reject legacy `own
/// struct`, surface-reference, and cast syntax without requiring HIR, lowering,
/// or a runtime backend.
pub fn forbidden_surface_syntax_diagnostics(tokens: &[Token]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    check_own_struct_attempts(tokens, &mut diagnostics);
    check_surface_reference_attempts(tokens, &mut diagnostics);
    check_implicit_conversion_attempts(tokens, &mut diagnostics);
    diagnostics
}

/// Build the canonical diagnostic for a parsed construct that RSScript does
/// not support.
///
/// The compiler may still discover the construct while adapting syntax into
/// transitional HIR, but the user-facing language contract belongs to the
/// platform-neutral semantics layer.
pub fn unsupported_syntax_diagnostic(
    span: Span,
    label: impl Into<String>,
    cause: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(
        code::UNSUPPORTED_SYNTAX,
        "unsupported RSScript syntax.",
        span,
        label,
    )
    .with_cause(cause)
    .with_fix(
        "rewrite_supported_syntax",
        "Rewrite this construct using the currently supported RSScript syntax.",
        "manual",
    )
}

fn check_own_struct_attempts(tokens: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    for index in 0..tokens.len().saturating_sub(1) {
        if tokens[index].is_ident_text("own") && tokens[index + 1].is_ident_text("struct") {
            diagnostics.push(
                Diagnostic::error(
                    code::OWN_STRUCT_ATTEMPT,
                    "`own struct` is not part of RSScript v0.7.",
                    tokens[index].span.clone(),
                    "own struct attempt",
                )
                .with_cause("v0.7 has only `class`, `struct`, and `resource` type declarations.")
                .with_fix(
                    "choose_type_kind",
                    "Use `struct` for inline values, `class` for managed identity, or `resource` for deterministic cleanup.",
                    "manual",
                ),
            );
        }
    }
}

fn check_surface_reference_attempts(tokens: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    for index in 0..tokens.len() {
        if !tokens[index].symbol("&") || is_boolean_and(tokens, index) || is_bit_and(tokens, index)
        {
            continue;
        }
        diagnostics.push(
            Diagnostic::error(
                code::SURFACE_REFERENCE_ATTEMPT,
                "surface reference syntax is not part of RSScript.",
                tokens[index].span.clone(),
                "surface reference attempt",
            )
            .with_cause("RSScript uses explicit data effects instead of `&T` or `&mut T` syntax.")
            .with_fix(
                "use_data_effect",
                "Use a parameter effect such as `value: read T` or `value: mut T`.",
                "manual",
            ),
        );
    }
}

fn is_boolean_and(tokens: &[Token], index: usize) -> bool {
    tokens
        .get(index.wrapping_sub(1))
        .is_some_and(|token| token.symbol("&"))
        || tokens.get(index + 1).is_some_and(|token| token.symbol("&"))
}

fn is_bit_and(tokens: &[Token], index: usize) -> bool {
    tokens
        .get(index.wrapping_sub(1))
        .is_some_and(token_can_end_expr)
        && tokens.get(index + 1).is_some_and(token_can_start_expr)
}

fn token_can_end_expr(token: &Token) -> bool {
    matches!(
        token.kind,
        TokenKind::Ident(_)
            | TokenKind::Keyword(_)
            | TokenKind::Number(_)
            | TokenKind::String(_)
            | TokenKind::InterpolatedString(_)
            | TokenKind::MultilineString(_)
    ) || token.symbol(")")
        || token.symbol("]")
        || token.symbol("}")
}

fn token_can_start_expr(token: &Token) -> bool {
    matches!(
        token.kind,
        TokenKind::Ident(_)
            | TokenKind::Keyword(_)
            | TokenKind::Number(_)
            | TokenKind::String(_)
            | TokenKind::InterpolatedString(_)
            | TokenKind::MultilineString(_)
    ) || token.symbol("(")
        || token.symbol("[")
        || token.symbol("{")
}

fn check_implicit_conversion_attempts(tokens: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    for index in 0..tokens.len() {
        if !tokens[index].is_ident_text("as")
            || as_belongs_to_with(tokens, index)
            || as_belongs_to_use(tokens, index)
        {
            continue;
        }
        diagnostics.push(
            Diagnostic::error(
                code::IMPLICIT_CONVERSION_ATTEMPT,
                "cast-style conversions are not part of RSScript.",
                tokens[index].span.clone(),
                "implicit conversion attempt",
            )
            .with_cause("RSScript requires conversions to be explicit named APIs so review tools can see them.")
            .with_fix(
                "use_named_conversion",
                "Use a named conversion such as `Target.from(value: read source)`.",
                "manual",
            ),
        );
    }
}

fn as_belongs_to_use(tokens: &[Token], as_index: usize) -> bool {
    for token in tokens[..as_index].iter().rev() {
        if token.is_ident_text("use") {
            return true;
        }
        let is_path_token =
            matches!(token.kind, TokenKind::Ident(_) | TokenKind::Keyword(_)) || token.symbol(".");
        if !is_path_token {
            return false;
        }
    }
    false
}

fn as_belongs_to_with(tokens: &[Token], as_index: usize) -> bool {
    for token in tokens[..as_index].iter().rev() {
        if token.is_ident_text("with") {
            return true;
        }
        if token.symbol("{") || token.symbol("}") || is_statement_boundary_keyword(token) {
            return false;
        }
    }
    false
}

fn is_statement_boundary_keyword(token: &Token) -> bool {
    [
        "let", "local", "return", "fn", "class", "struct", "resource", "if", "else", "loop",
        "while", "break", "continue",
    ]
    .iter()
    .any(|keyword| token.is_ident_text(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_forbidden_forms_without_rejecting_valid_and_or_import_as() {
        let tokens = rsscript_syntax::lexer::lex(
            "forms.rss",
            "own struct Value {}\nfn f(value: Bool) { let x = &value; value && value; value as Int }\nuse host.fs as fs\n",
        );
        let diagnostics = forbidden_surface_syntax_diagnostics(&tokens);
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&code::OWN_STRUCT_ATTEMPT));
        assert!(codes.contains(&code::SURFACE_REFERENCE_ATTEMPT));
        assert!(codes.contains(&code::IMPLICIT_CONVERSION_ATTEMPT));
        assert_eq!(
            codes
                .iter()
                .filter(|code| **code == code::IMPLICIT_CONVERSION_ATTEMPT)
                .count(),
            1
        );
    }

    #[test]
    fn unsupported_syntax_contract_is_canonical() {
        let diagnostic = unsupported_syntax_diagnostic(
            Span {
                file: "forms.rss".into(),
                line: 1,
                column: 4,
                length: 7,
            },
            "unsupported form",
            "the form has no RSScript semantic contract.",
        );

        assert_eq!(diagnostic.code, code::UNSUPPORTED_SYNTAX);
        assert_eq!(diagnostic.summary, "unsupported RSScript syntax.");
        assert_eq!(diagnostic.label, "unsupported form");
        assert_eq!(
            diagnostic.causes[0],
            "the form has no RSScript semantic contract."
        );
        assert_eq!(diagnostic.fixes[0].kind, "rewrite_supported_syntax");
    }
}
