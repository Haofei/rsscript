//! Type-name, field-shape, and resource-type validation.
//!
//! Kept as a named phase so the self-hosted checker can mirror this boundary
//! without coupling its partial AST to Rust implementation details.

use crate::analyzer::Analyzer;

pub(crate) fn check_names(analyzer: &mut Analyzer<'_>) {
    analyzer.check_unknown_types();
    analyzer.check_unknown_fields();
    analyzer.check_unknown_bindings();
    analyzer.check_fd_surface();
}

pub(crate) fn check_resource_shapes(analyzer: &mut Analyzer<'_>) {
    analyzer.check_resource_fields();
    analyzer.check_weak_fields();
    analyzer.check_resource_pool_type_arguments();
    analyzer.check_resource_generic_arguments();
}
