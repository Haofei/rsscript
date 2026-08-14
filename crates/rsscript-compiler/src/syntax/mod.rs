pub use rsscript_syntax::ast;

pub use rsscript_semantics::{demangle_diagnostics, isolate_module_namespaces};
pub(crate) use rsscript_syntax::parse_source_tokens;
pub use rsscript_syntax::{parse_source, parse_source_raw};
