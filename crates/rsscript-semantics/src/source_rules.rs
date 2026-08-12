//! Source-level semantic rules that do not require compiler orchestration.

use std::collections::{HashMap, HashSet};

use rsscript_diagnostics::{Diagnostic, Span, code};
use rsscript_syntax::ast::{Item, Program};
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

/// Derive per-file module/use organization diagnostics.
///
/// A merged workspace contains declarations from multiple files, so every
/// ordering and local-import binding rule is keyed by its source file rather
/// than by the merged item stream as a whole.
pub fn module_use_layout_diagnostics(items: &[Item]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut seen_module = HashSet::new();
    let mut seen_use = HashSet::new();
    let mut seen_non_organization_item = HashSet::new();
    let mut seen_import_local: HashMap<String, HashSet<String>> = HashMap::new();

    for item in items {
        let file = item_span_file(item);
        match item {
            Item::Module(module) => {
                if seen_module.contains(&file) {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        module.span.clone(),
                        "duplicate module declaration",
                        "A source or interface file may declare at most one `module` identity.",
                    ));
                }
                if seen_non_organization_item.contains(&file) {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        module.span.clone(),
                        "misplaced module declaration",
                        "`module` is source-organization metadata and must appear before declarations.",
                    ));
                }
                if seen_use.contains(&file) {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        module.span.clone(),
                        "misplaced module declaration",
                        "`module` must be the first organization declaration when present; `use` declarations follow it.",
                    ));
                }
                seen_module.insert(file);
            }
            Item::Use(use_decl) => {
                if seen_non_organization_item.contains(&file) {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        use_decl.span.clone(),
                        "misplaced use declaration",
                        "`use` is source-organization metadata and must appear before declarations.",
                    ));
                }
                if let Some(local) = use_decl.local_name()
                    && !seen_import_local
                        .entry(file.clone())
                        .or_default()
                        .insert(local.to_owned())
                {
                    diagnostics.push(unsupported_syntax_diagnostic(
                        use_decl.span.clone(),
                        "duplicate import name",
                        "Two `use` declarations bind the same local name in this file. Rename one with `use module.name as other_name` so each import is unambiguous.",
                    ));
                }
                seen_use.insert(file);
            }
            Item::Type(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_)
            | Item::Function(_) => {
                seen_non_organization_item.insert(file);
            }
        }
    }

    diagnostics
}

fn item_span_file(item: &Item) -> String {
    match item {
        Item::Function(decl) => decl.span.file.clone(),
        Item::Const(decl) => decl.span.file.clone(),
        Item::Type(decl) => decl.span.file.clone(),
        Item::SumType(decl) => decl.span.file.clone(),
        Item::TypeAlias(decl) => decl.span.file.clone(),
        Item::Module(decl) => decl.span.file.clone(),
        Item::Use(decl) => decl.span.file.clone(),
    }
}

/// Derive declaration-level surface diagnostics from one parsed source or
/// interface program and its token stream.
///
/// These are language rules, not compiler lowering restrictions: removed
/// markers, malformed top-level declarations, generated-name reservation,
/// protocol generic reservation, and the body requirement for `.rss`
/// implementation functions.
pub fn declaration_surface_diagnostics(tokens: &[Token], program: &Program) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for index in 0..tokens.len().saturating_sub(1) {
        if tokens[index].is_ident_text("effects") && tokens[index + 1].symbol("(") {
            diagnostics.push(unsupported_syntax_diagnostic(
                tokens[index].span.clone(),
                "removed effect clause",
                "Generic declaration-effect clauses are not part of RSScript; use structured `retains(name)` only when a parameter escapes the call.",
            ));
        }
        if (tokens[index].is_ident_text("native") || tokens[index].is_ident_text("unsafe"))
            && (tokens[index + 1].is_ident_text("fn") || tokens[index + 1].is_ident_text("module"))
        {
            diagnostics.push(unsupported_syntax_diagnostic(
                tokens[index].span.clone(),
                "removed implementation marker",
                "Implementation origin and host risk belong to package binding metadata, not source declarations.",
            ));
        }
    }
    for span in &program.unknown_top_level_spans {
        diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "unsupported top-level item",
            "This top-level construct is outside the current RSScript parser surface.",
        ));
    }
    for span in &program.malformed_declaration_spans {
        diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed declaration",
            "This declaration starts like RSScript syntax but does not match the supported declaration grammar.",
        ));
    }
    diagnostics.extend(module_use_layout_diagnostics(&program.items));
    let protocol_names = program
        .protocols
        .iter()
        .map(|protocol| protocol.name.as_str())
        .collect::<HashSet<_>>();
    for item in &program.items {
        if let Some((name, span)) = declaration_name_and_span(item) {
            let leaf = name.rsplit('.').next().unwrap_or(name);
            if is_reserved_generated_name(leaf) {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "reserved declaration name",
                    "The `__rss_` and `__rsscript_` prefixes are reserved for compiler-generated symbols; rename this declaration.",
                ));
            }
        }
    }
    for index in 0..tokens.len().saturating_sub(2) {
        if (tokens[index].is_ident_text("protocol") || tokens[index].is_ident_text("impl"))
            && tokens[index + 2].symbol("<")
        {
            diagnostics.push(unsupported_syntax_diagnostic(
                tokens[index + 2].span.clone(),
                "generic protocol declaration",
                "Generic protocol and protocol-implementation declarations are reserved for a later language version; use function generics with a protocol bound instead.",
            ));
        }
    }
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if !function.has_body
            && !function.span.file.ends_with(".rssi")
            && !function
                .name
                .split_once('.')
                .is_some_and(|(namespace, _)| protocol_names.contains(namespace))
        {
            diagnostics.push(unsupported_syntax_diagnostic(
                function.span.clone(),
                "bodyless source function",
                "Implementation functions in `.rss` files require a body; put external declarations in an `.rssi` package interface.",
            ));
        }
    }
    diagnostics
}

fn declaration_name_and_span(item: &Item) -> Option<(&str, &Span)> {
    match item {
        Item::Function(decl) => Some((&decl.name, &decl.span)),
        Item::Type(decl) => Some((&decl.name, &decl.span)),
        Item::SumType(decl) => Some((&decl.name, &decl.span)),
        Item::TypeAlias(decl) => Some((&decl.name, &decl.span)),
        Item::Const(decl) => Some((&decl.name, &decl.span)),
        Item::Module(_) | Item::Use(_) => None,
    }
}

fn is_reserved_generated_name(leaf: &str) -> bool {
    leaf.starts_with("__rss_") || leaf.starts_with("__rsscript_")
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

    #[test]
    fn module_use_layout_is_checked_per_source_file() {
        let source = rsscript_syntax::parse_source(
            "one.rss",
            "use dep.item\nuse other.item as item\nfn work() -> Unit {}\nmodule late\n",
        );
        let diagnostics = module_use_layout_diagnostics(&source.items);
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(diagnostics[0].label, "duplicate import name");
        assert_eq!(diagnostics[1].label, "misplaced module declaration");
        assert_eq!(diagnostics[2].label, "misplaced module declaration");
    }

    #[test]
    fn declaration_surface_rules_cover_generated_names_and_source_bodies() {
        let source = "fn __rss_hidden() -> Unit {}\nfn external() -> Unit\n";
        let program = rsscript_syntax::parse_source("source.rss", source);
        let tokens = rsscript_syntax::lexer::lex("source.rss", source);
        let diagnostics = declaration_surface_diagnostics(&tokens, &program);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].label, "reserved declaration name");
        assert_eq!(diagnostics[1].label, "bodyless source function");
    }
}
