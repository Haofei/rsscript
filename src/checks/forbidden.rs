use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::lexer::TokenKind;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    check_operator_overload_attempts(analyzer);
    check_implicit_conversion_attempts(analyzer);
}

fn check_implicit_conversion_attempts(analyzer: &mut Analyzer<'_>) {
    for index in 0..analyzer.tokens.len() {
        if !analyzer.tokens[index].is_ident_text("as") || as_belongs_to_with(analyzer, index) {
            continue;
        }
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::IMPLICIT_CONVERSION_ATTEMPT,
                "cast-style conversions are not part of RSScript.",
                analyzer.tokens[index].span.clone(),
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

fn as_belongs_to_with(analyzer: &Analyzer<'_>, as_index: usize) -> bool {
    for token in analyzer.tokens[..as_index].iter().rev() {
        if token.is_ident_text("with") {
            return true;
        }
        if token.symbol("{")
            || token.symbol("}")
            || token.is_ident_text("let")
            || token.is_ident_text("local")
            || token.is_ident_text("return")
            || token.is_ident_text("fn")
            || token.is_ident_text("class")
            || token.is_ident_text("struct")
            || token.is_ident_text("resource")
            || token.is_ident_text("if")
            || token.is_ident_text("else")
            || token.is_ident_text("loop")
            || token.is_ident_text("while")
            || token.is_ident_text("break")
            || token.is_ident_text("continue")
        {
            return false;
        }
    }
    false
}

fn check_operator_overload_attempts(analyzer: &mut Analyzer<'_>) {
    for i in 1..analyzer.tokens.len().saturating_sub(1) {
        if !(analyzer.tokens[i].symbol("+")
            || analyzer.tokens[i].symbol("-")
            || analyzer.tokens[i].symbol("*")
            || analyzer.tokens[i].symbol("/"))
        {
            continue;
        }
        let left_number = matches!(analyzer.tokens[i - 1].kind, TokenKind::Number(_));
        let right_number = matches!(analyzer.tokens[i + 1].kind, TokenKind::Number(_));
        let likely_type_name = analyzer.tokens[i - 1]
            .text()
            .chars()
            .next()
            .is_some_and(char::is_uppercase)
            || analyzer.tokens[i + 1]
                .text()
                .chars()
                .next()
                .is_some_and(char::is_uppercase);
        if !left_number && !right_number && likely_type_name {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::OPERATOR_OVERLOAD_ATTEMPT,
                    "operators cannot be overloaded for user-defined types.",
                    analyzer.tokens[i].span.clone(),
                    "operator on non-builtin-looking value",
                )
                .with_fix(
                    "use_named_function",
                    "Use a named function such as `Type.add(left: read a, right: read b)`.",
                    "manual",
                ),
            );
        }
    }
}
