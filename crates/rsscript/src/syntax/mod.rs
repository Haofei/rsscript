pub use rsscript_syntax::ast;
pub(crate) mod async_await_hoist;
pub(crate) mod desugar;
pub(crate) mod function_value_desugar;
pub(crate) mod module_isolation;
mod parser;

pub use module_isolation::{demangle_diagnostics, isolate_module_namespaces};
pub(crate) use parser::parse_source_tokens;
pub use parser::{parse_source, parse_source_raw};
