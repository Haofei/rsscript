mod analyzer;
mod checks;
mod diagnostic;
mod hir;
mod lexer;
mod review;
pub mod syntax;

pub use analyzer::analyze_source;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Severity, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
};
pub use review::{ReviewFinding, ReviewRisk, format_review_human, review_sources};
