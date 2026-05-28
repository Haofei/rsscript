mod analyzer;
mod checks;
mod diagnostic;
mod hir;
mod lexer;
mod review;
mod rust_lower;
pub mod syntax;

pub use analyzer::analyze_source;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Severity, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
};
pub use review::{
    ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary, ReviewMapClassification,
    ReviewMapFile, ReviewMapRegion, ReviewMapSummary, ReviewRisk, format_review_human,
    format_review_json, format_review_map_human, format_review_map_json, review_map_sources,
    review_sources,
};
pub use rust_lower::{
    GeneratedRustPackage, LoweredRust, RustSourceMapEntry, lower_program_to_rust,
    lower_program_to_rust_with_map, lower_source_to_rust, lower_source_to_rust_package,
    lower_source_to_rust_with_map, parse_source_map_json, remap_rustc_diagnostic_json,
    remap_rustc_diagnostic_json_lines,
};
