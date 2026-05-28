use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::lexer::TokenKind;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    check_operator_overload_attempts(analyzer);
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
