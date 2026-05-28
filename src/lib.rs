mod analyzer;
mod ast;
mod diagnostic;
mod lexer;
pub mod syntax;

pub use analyzer::analyze_source;
pub use diagnostic::{Diagnostic, Severity, format_diagnostics_human, format_diagnostics_json};
