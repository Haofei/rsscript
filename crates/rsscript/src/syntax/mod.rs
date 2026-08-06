pub use rsscript_syntax::ast;
pub(crate) mod module_isolation;

pub use module_isolation::{demangle_diagnostics, isolate_module_namespaces};
pub(crate) use rsscript_syntax::{
    desugar_function_values, hoist_async_awaits, parse_source_tokens,
};
pub use rsscript_syntax::{parse_source, parse_source_raw};
