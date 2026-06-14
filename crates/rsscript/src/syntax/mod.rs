pub mod ast;
pub(crate) mod desugar;
pub(crate) mod module_isolation;
mod parser;

pub use module_isolation::{demangle_diagnostics, isolate_module_namespaces};
pub use parser::{parse_source, parse_source_raw};
