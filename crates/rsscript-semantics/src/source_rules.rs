//! Source-level semantic rules that do not require compiler orchestration.

use std::collections::{HashMap, HashSet};

use rsscript_diagnostics::{Diagnostic, Span, code};
use rsscript_syntax::ast::{Item, Program, TypeRef};
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

/// Derive source-only diagnostics for one declaration item.
///
/// Type-alias-aware callback placement remains a later semantic query, but
/// malformed declaration fragments, opaque/drop restrictions, and const
/// literal requirements need no resolved type facts.
pub fn declaration_item_surface_diagnostics(item: &Item) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    match item {
        Item::Function(function) => {
            for span in &function.malformed_generic_param_spans {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "malformed generic parameter declaration",
                    "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                ));
            }
            for span in &function.malformed_param_spans {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "malformed parameter declaration",
                    "Function parameters must use `name: Type`, `name: read Type`, `name: mut Type`, or `name: take Type`.",
                ));
            }
        }
        Item::Type(type_decl) => {
            for span in &type_decl.malformed_generic_param_spans {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "malformed generic parameter declaration",
                    "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                ));
            }
            for span in &type_decl.malformed_field_spans {
                diagnostics.push(unsupported_syntax_diagnostic(
                    span.clone(),
                    "malformed field declaration",
                    "Type fields must use `name: Type`, `name: handle Type`, or `name: weak Type`.",
                ));
            }
            if type_decl.is_opaque && !type_decl.fields.is_empty() {
                diagnostics.push(unsupported_syntax_diagnostic(
                    type_decl.span.clone(),
                    "unsupported opaque type body",
                    "Opaque interface types hide their representation. Declare `opaque struct Name`, `opaque class Name`, or `opaque resource Name` without fields.",
                ));
            }
            if type_decl.is_opaque && type_decl.drop_body.is_some() {
                diagnostics.push(unsupported_syntax_diagnostic(
                    type_decl.span.clone(),
                    "unsupported opaque type body",
                    "Opaque resource contracts hide their implementation details, including drop bodies. Resource cleanup belongs to the implementation, not the `.rssi` contract.",
                ));
            }
            if !matches!(type_decl.kind, rsscript_syntax::ast::TypeKind::Resource)
                && let Some(drop_body) = &type_decl.drop_body
            {
                diagnostics.push(unsupported_syntax_diagnostic(
                    drop_body.span.clone(),
                    "unsupported managed drop",
                    "Managed class and struct values do not have user-observable destructors in v0.7. Use `resource` with `with` for deterministic cleanup.",
                ));
            }
        }
        Item::Const(decl) => {
            let is_literal = matches!(
                &decl.value,
                rsscript_syntax::ast::Expr::Number(_, _)
                    | rsscript_syntax::ast::Expr::String(_, _)
                    | rsscript_syntax::ast::Expr::MultilineString(_, _)
            ) || matches!(&decl.value, rsscript_syntax::ast::Expr::Ident(name, _) if name == "true" || name == "false");
            if !is_literal {
                diagnostics.push(unsupported_syntax_diagnostic(
                    decl.span.clone(),
                    "unsupported const initializer",
                    "A v0.7 `const` initializer must be a literal (number, string, or `true`/`false`). Compute the value and write it as a literal; expressions and calls in `const` position are not supported yet.",
                ));
            }
        }
        Item::Module(_) | Item::Use(_) | Item::SumType(_) | Item::TypeAlias(_) => {}
    }
    diagnostics
}

/// Derive callback qualifier and malformed type-argument diagnostics for a
/// canonical type reference.
///
/// Callers may expand aliases before invoking this query, but the placement
/// rules and recursive diagnostic traversal are semantic and backend-neutral.
pub fn type_ref_surface_diagnostics(
    ty: &TypeRef,
    allow_noescape: bool,
    allow_owned: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    type_ref_surface_diagnostics_inner(ty, allow_noescape, allow_owned, &mut diagnostics);
    diagnostics
}

fn type_ref_surface_diagnostics_inner(
    ty: &TypeRef,
    allow_noescape: bool,
    allow_owned: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if ty.is_noescape && (!allow_noescape || ty.name != "Fn") {
        diagnostics.push(unsupported_syntax_diagnostic(
            ty.span.clone(),
            "unsupported noescape position",
            "`noescape Fn(...)` is only supported as a direct function parameter type.",
        ));
    }
    if ty.is_owned && (!allow_owned || ty.name != "Fn") {
        diagnostics.push(unsupported_syntax_diagnostic(
            ty.span.clone(),
            "unsupported owned position",
            "`owned Fn(...)` is supported as a function parameter and in storable positions (generic argument, struct field, binding, or return type).",
        ));
    }
    for span in &ty.malformed_arg_spans {
        diagnostics.push(unsupported_syntax_diagnostic(
            span.clone(),
            "malformed type argument",
            "Type arguments must be valid type references; empty or unsupported type argument slots are not allowed.",
        ));
    }
    for arg in &ty.args {
        type_ref_surface_diagnostics_inner(arg, false, allow_owned, diagnostics);
    }
    for param in &ty.fn_params {
        type_ref_surface_diagnostics_inner(param, false, allow_owned, diagnostics);
    }
    if let Some(ret) = &ty.fn_return {
        type_ref_surface_diagnostics_inner(ret, false, allow_owned, diagnostics);
    }
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

    #[test]
    fn declaration_item_rules_reject_non_literal_constants() {
        let program = rsscript_syntax::parse_source("const.rss", "const value = make()\n");
        assert_eq!(
            declaration_item_surface_diagnostics(&program.items[0])[0].label,
            "unsupported const initializer"
        );
    }

    #[test]
    fn type_ref_rules_reject_noescape_outside_a_direct_parameter() {
        let program =
            rsscript_syntax::parse_source("type.rss", "fn make() -> noescape Fn() -> Unit {}\n");
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function declaration");
        };
        let return_ty = function.return_ty.as_ref().expect("return type");
        assert_eq!(
            type_ref_surface_diagnostics(return_ty, false, true)[0].label,
            "unsupported noescape position"
        );
    }
}
