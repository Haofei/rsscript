mod analyzer;
mod ast;
mod checks;
mod diagnostic;
mod hir;
mod lexer;
pub mod syntax;

pub use analyzer::analyze_source;
pub use diagnostic::{Diagnostic, Severity, format_diagnostics_human, format_diagnostics_json};
