use crate::diagnostic::{Severity, Span};
use crate::lexer::{TokenKind, lex};
use rsscript_semantics::analyze_source_with_core;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolCompleteness {
    Complete,
    Partial,
}

#[derive(Debug)]
pub struct GenerateContext<'a> {
    pub file: &'a str,
    pub partial_source: &'a str,
    pub symbol_completeness: SymbolCompleteness,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum PrefixStatus {
    Complete,
    Incomplete,
    Dead { at: Span, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationOptions {
    pub max_returned_names: usize,
    pub max_probe_candidates: usize,
    pub include_snippets: bool,
}

impl Default for ContinuationOptions {
    fn default() -> Self {
        Self {
            max_returned_names: 50,
            max_probe_candidates: 100,
            include_snippets: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Continuations {
    pub keywords: Vec<String>,
    pub symbols: Vec<String>,
    pub literals: Vec<LiteralClass>,
    pub names: Vec<Completion>,
    pub expected_type: Option<ExpectedType>,
    pub may_stop: bool,
    pub truncated: bool,
    pub total_name_candidates: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiteralClass {
    String,
    Int,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Completion {
    pub text: String,
    pub insert_text: String,
    pub replace: TextRange,
    pub kind: CompletionKind,
    pub signature: Option<String>,
    pub required_effect: Option<Effect>,
    pub result_type: Option<TypeRef>,
    pub commit: CommitBehavior,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExpectedType {
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TypeRef {
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    Mut,
    Take,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitBehavior {
    InsertName,
    ReplacePartialToken,
    InsertArgumentWithEffect,
    InsertSnippet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    Local,
    Param,
    Type,
    Function,
    Method,
    ArgName,
    Variant,
}

const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "features", "module", "use", "pub", "fn", "async", "native", "struct", "class", "resource",
    "opaque", "sum", "type", "const", "protocol", "impl",
];

const STATEMENT_KEYWORDS: &[&str] = &[
    "let",
    "return",
    "if",
    "else",
    "match",
    "while",
    "for",
    "loop",
    "break",
    "continue",
    "task_group",
    "select",
    "await",
];

const EFFECT_KEYWORDS: &[&str] = &["read", "mut", "take"];

pub fn prefix_status(ctx: &GenerateContext<'_>) -> PrefixStatus {
    if let Some(dead) = committed_delimiter_error(ctx.file, ctx.partial_source) {
        return dead;
    }
    if has_unclosed_delimiter(ctx.partial_source) || trailing_partial_construct(ctx.partial_source)
    {
        return PrefixStatus::Incomplete;
    }
    let diagnostics = analyze_source_with_core(ctx.file, ctx.partial_source);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return PrefixStatus::Incomplete;
    }
    PrefixStatus::Complete
}

pub fn valid_continuations(
    ctx: &GenerateContext<'_>,
    options: ContinuationOptions,
) -> Continuations {
    let partial = trailing_ident(ctx.partial_source);
    let mut keywords = Vec::new();
    let mut symbols = Vec::new();
    let mut literals = Vec::new();

    if expects_effect_keyword(ctx.partial_source) {
        keywords.extend(filter_words(EFFECT_KEYWORDS, &partial.text));
    } else if is_top_level_position(ctx.partial_source) {
        keywords.extend(filter_words(TOP_LEVEL_KEYWORDS, &partial.text));
    } else {
        keywords.extend(filter_words(STATEMENT_KEYWORDS, &partial.text));
        literals.extend([LiteralClass::String, LiteralClass::Int, LiteralClass::Bool]);
    }

    let balance = delimiter_balance(ctx.partial_source);
    if balance.parens > 0 {
        symbols.extend([",".to_string(), ")".to_string()]);
    }
    if balance.braces > 0 {
        symbols.push("}".to_string());
    }
    if symbols.is_empty() && !matches!(prefix_status(ctx), PrefixStatus::Complete) {
        symbols.extend(["(".to_string(), "{".to_string()]);
    }
    symbols.sort();
    symbols.dedup();

    let truncated = keywords.len() > options.max_probe_candidates;
    keywords.truncate(options.max_probe_candidates);

    Continuations {
        keywords,
        symbols,
        literals,
        names: Vec::new(),
        expected_type: None,
        may_stop: matches!(prefix_status(ctx), PrefixStatus::Complete),
        truncated,
        total_name_candidates: Some(0),
    }
}

fn filter_words(words: &[&str], partial: &str) -> Vec<String> {
    words
        .iter()
        .filter(|word| word.starts_with(partial))
        .map(|word| (*word).to_string())
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct DelimiterBalance {
    parens: i32,
    braces: i32,
    brackets: i32,
}

fn delimiter_balance(source: &str) -> DelimiterBalance {
    let mut balance = DelimiterBalance {
        parens: 0,
        braces: 0,
        brackets: 0,
    };
    for token in lex("<generate>", source) {
        match token.kind {
            TokenKind::Symbol("(") => balance.parens += 1,
            TokenKind::Symbol(")") => balance.parens -= 1,
            TokenKind::Symbol("{") => balance.braces += 1,
            TokenKind::Symbol("}") => balance.braces -= 1,
            TokenKind::Symbol("[") => balance.brackets += 1,
            TokenKind::Symbol("]") => balance.brackets -= 1,
            TokenKind::Eof => break,
            _ => {}
        }
    }
    balance
}

fn committed_delimiter_error(file: &str, source: &str) -> Option<PrefixStatus> {
    let mut parens = Vec::new();
    let mut braces = Vec::new();
    let mut brackets = Vec::new();
    for token in lex(file, source) {
        match token.kind {
            TokenKind::Symbol("(") => parens.push(token.span),
            TokenKind::Symbol("{") => braces.push(token.span),
            TokenKind::Symbol("[") => brackets.push(token.span),
            TokenKind::Symbol(")") if parens.pop().is_none() => {
                return Some(PrefixStatus::Dead {
                    at: token.span,
                    reason: "unmatched `)` cannot be repaired by appending more source."
                        .to_string(),
                });
            }
            TokenKind::Symbol("}") if braces.pop().is_none() => {
                return Some(PrefixStatus::Dead {
                    at: token.span,
                    reason: "unmatched `}` cannot be repaired by appending more source."
                        .to_string(),
                });
            }
            TokenKind::Symbol("]") if brackets.pop().is_none() => {
                return Some(PrefixStatus::Dead {
                    at: token.span,
                    reason: "unmatched `]` cannot be repaired by appending more source."
                        .to_string(),
                });
            }
            TokenKind::Eof => break,
            _ => {}
        }
    }
    None
}

fn has_unclosed_delimiter(source: &str) -> bool {
    let balance = delimiter_balance(source);
    balance.parens > 0 || balance.braces > 0 || balance.brackets > 0
}

fn trailing_partial_construct(source: &str) -> bool {
    let trimmed = source.trim_end();
    trimmed.is_empty()
        || trimmed.ends_with("->")
        || trimmed.ends_with(':')
        || trimmed.ends_with(',')
        || trimmed.ends_with('.')
        || trimmed.ends_with('=')
        || trimmed.ends_with("=>")
}

fn is_top_level_position(source: &str) -> bool {
    let balance = delimiter_balance(source);
    balance.parens <= 0 && balance.braces <= 0 && balance.brackets <= 0
}

fn expects_effect_keyword(source: &str) -> bool {
    let trimmed = source.trim_end();
    let Some(colon) = trimmed.rfind(':') else {
        return false;
    };
    let after_colon = trimmed[colon + 1..].trim_start();
    if after_colon.is_empty() {
        return true;
    }
    EFFECT_KEYWORDS
        .iter()
        .any(|effect| effect.starts_with(after_colon))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PartialToken {
    text: String,
    range: TextRange,
}

fn trailing_ident(source: &str) -> PartialToken {
    let end = source.len();
    let start = source
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || *ch == '_')
        .last()
        .map(|(index, _)| index)
        .unwrap_or(end);
    PartialToken {
        text: source[start..end].to_string(),
        range: TextRange { start, end },
    }
}
