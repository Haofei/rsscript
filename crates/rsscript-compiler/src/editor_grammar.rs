//! Generator for the VS Code TextMate grammar.
//!
//! The keyword highlight rules are derived from the single source of truth in
//! [`crate::lexer`] (`KEYWORDS`, `CONTEXTUAL_KEYWORDS`, `BUILTIN_CONSTANTS`), so
//! editing the lexer keeps the grammar honest. The static structural rules
//! (comments, strings, numbers, calls, operators) live here.
//!
//! Regenerate the committed file with `cargo run --bin gen-grammar`. The
//! `vscode_grammar_is_up_to_date` integration test fails whenever the committed
//! file drifts from this generator.

use serde_json::{Value, json};

use crate::lexer::{BUILTIN_CONSTANTS, CONTEXTUAL_KEYWORDS, KEYWORDS, KeywordCategory};

/// Path of the generated grammar, relative to the crate root.
pub const VSCODE_GRAMMAR_PATH: &str = "editors/vscode/syntaxes/rsscript.tmLanguage.json";

/// Categories emitted as keyword rules, in the order they appear in the grammar.
const CATEGORIES: &[KeywordCategory] = &[
    KeywordCategory::Control,
    KeywordCategory::Declaration,
    KeywordCategory::Modifier,
    KeywordCategory::Ownership,
];

fn scope(category: KeywordCategory) -> &'static str {
    match category {
        KeywordCategory::Control => "keyword.control.rsscript",
        KeywordCategory::Declaration => "keyword.declaration.rsscript",
        KeywordCategory::Modifier => "storage.modifier.rsscript",
        KeywordCategory::Ownership => "storage.modifier.ownership.rsscript",
    }
}

/// `if|else|for|...` for one category, drawn from the lexer keyword tables in
/// declaration order.
fn alternation(category: KeywordCategory) -> String {
    KEYWORDS
        .iter()
        .chain(CONTEXTUAL_KEYWORDS.iter())
        .filter(|(_, value)| *value == category)
        .map(|(word, _)| *word)
        .collect::<Vec<_>>()
        .join("|")
}

fn keyword_rules() -> Vec<Value> {
    CATEGORIES
        .iter()
        .map(|category| {
            json!({
                "name": scope(*category),
                "match": format!(r"\b({})\b", alternation(*category)),
            })
        })
        .collect()
}

/// Render the full TextMate grammar as pretty JSON (trailing newline included).
pub fn vscode_tmlanguage_json() -> String {
    let grammar = json!({
        "$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
        "name": "RsScript",
        "scopeName": "source.rsscript",
        "patterns": [
            { "include": "#comments" },
            { "include": "#strings" },
            { "include": "#numbers" },
            { "include": "#keywords" },
            { "include": "#constants" },
            { "include": "#function-definition" },
            { "include": "#named-arguments" },
            { "include": "#call" },
            { "include": "#types" },
            { "include": "#operators" }
        ],
        "repository": {
            "comments": {
                "patterns": [
                    {
                        "name": "comment.line.double-slash.rsscript",
                        "begin": "//",
                        "end": "$"
                    }
                ]
            },
            "strings": {
                "patterns": [
                    {
                        "name": "string.quoted.triple.rsscript",
                        "begin": "\"\"\"",
                        "end": "\"\"\""
                    },
                    {
                        "name": "string.quoted.double.rsscript",
                        "begin": "\"",
                        "end": "\"",
                        "patterns": [
                            { "name": "constant.character.escape.rsscript", "match": r"\\." }
                        ]
                    }
                ]
            },
            "numbers": {
                "patterns": [
                    { "name": "constant.numeric.rsscript", "match": r"\b[0-9][0-9]*(\.[0-9]+)?\b" }
                ]
            },
            "keywords": { "patterns": keyword_rules() },
            "constants": {
                "patterns": [
                    {
                        "name": "constant.language.rsscript",
                        "match": format!(r"\b({})\b", BUILTIN_CONSTANTS.join("|")),
                    }
                ]
            },
            "function-definition": {
                "patterns": [
                    {
                        "match": r"\b(fn)\s+(?:([A-Z][A-Za-z0-9_]*)\s*(\.))?([a-z_][A-Za-z0-9_]*)",
                        "captures": {
                            "1": { "name": "keyword.declaration.rsscript" },
                            "2": { "name": "entity.name.type.rsscript" },
                            "3": { "name": "punctuation.accessor.rsscript" },
                            "4": { "name": "entity.name.function.rsscript" }
                        }
                    }
                ]
            },
            "named-arguments": {
                "patterns": [
                    {
                        "match": r"\b([a-z_][A-Za-z0-9_]*)\s*(:)(?!:)",
                        "captures": {
                            "1": { "name": "variable.parameter.rsscript" },
                            "2": { "name": "punctuation.separator.rsscript" }
                        }
                    }
                ]
            },
            "call": {
                "patterns": [
                    {
                        "match": r"(?:([A-Z][A-Za-z0-9_]*)\s*(\.))?([a-z_][A-Za-z0-9_]*)\s*(?=\()",
                        "captures": {
                            "1": { "name": "entity.name.type.rsscript" },
                            "2": { "name": "punctuation.accessor.rsscript" },
                            "3": { "name": "entity.name.function.call.rsscript" }
                        }
                    }
                ]
            },
            "types": {
                "patterns": [
                    { "name": "entity.name.type.rsscript", "match": r"\b[A-Z][A-Za-z0-9_]*\b" }
                ]
            },
            "operators": {
                "patterns": [
                    { "name": "keyword.operator.arrow.rsscript", "match": "->" },
                    { "name": "keyword.operator.error-propagation.rsscript", "match": r"\?" },
                    { "name": "keyword.operator.rsscript", "match": r"(==|!=|<=|>=|&&|\|\||[+\-*/=<>!&])" }
                ]
            }
        }
    });

    let mut text = serde_json::to_string_pretty(&grammar).expect("grammar serializes");
    text.push('\n');
    text
}
