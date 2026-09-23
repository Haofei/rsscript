#![forbid(unsafe_code)]

pub use rsscript_core_types::{FileId, SourceRevision, Span, TextRange};

/// Bounded frontend work accounting. Owned by the syntax crate because the
/// parser is the first consumer and every other consumer (semantics) already
/// depends on syntax; the VM must never see it.
mod work_budget;
pub use work_budget::{
    AnalysisRecursionGuard, BudgetExhaustion, FrontendBudget, FrontendBudgetLimits,
    ParseRecursionGuard,
};

pub mod ast;
mod async_await_hoist;
mod desugar;
mod formatter;
mod function_value_desugar;
pub mod lexer;
mod lint;
mod parser;
mod prefix;

pub use async_await_hoist::hoist_async_awaits;
pub use formatter::{
    format_declaration_signature, format_program, format_source, format_source_without_comments,
};
pub use function_value_desugar::desugar_function_values;
pub use lint::lint_source;
pub use parser::{PARSER_KEYWORDS, parse_source, parse_source_raw, parse_source_tokens};
pub use prefix::{
    CallContext, CursorContext, ExpectedTerminal, FunctionContext, IdentifierRole, LiteralKind,
    PrefixParseResult, PrefixParseState, SyntaxSite, TerminalCompleteness, parse_source_prefix,
};
