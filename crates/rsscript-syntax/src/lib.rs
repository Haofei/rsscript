#![forbid(unsafe_code)]

pub use rsscript_source_model::{FileId, SourceRevision, Span, TextRange};
pub use rsscript_work_budget::{FrontendBudget, FrontendBudgetLimits, ParseRecursionGuard};

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
pub use formatter::{format_program, format_source};
pub use function_value_desugar::desugar_function_values;
pub use lint::lint_source;
pub use parser::{parse_source, parse_source_raw, parse_source_tokens};
pub use prefix::{
    CallContext, CursorContext, ExpectedTerminal, FunctionContext, IdentifierRole, LiteralKind,
    PrefixParseResult, PrefixParseState, SyntaxSite, TerminalCompleteness, parse_source_prefix,
};
