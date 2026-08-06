#![forbid(unsafe_code)]

pub use rsscript_source_model::{FileId, SourceRevision, Span, TextRange};
pub use rsscript_work_budget::{FrontendBudget, FrontendBudgetLimits, ParseRecursionGuard};

pub mod ast;
mod async_await_hoist;
mod desugar;
mod function_value_desugar;
pub mod lexer;
mod parser;

pub use async_await_hoist::hoist_async_awaits;
pub use function_value_desugar::desugar_function_values;
pub use parser::{parse_source, parse_source_raw, parse_source_tokens};
