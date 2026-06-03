#![forbid(unsafe_code)]
#![allow(
    clippy::collapsible_if,
    clippy::needless_borrow,
    clippy::needless_lifetimes,
    clippy::question_mark,
    clippy::redundant_closure
)]

mod analyzer;
pub mod bbom;
mod checks;
mod core_index;
mod diagnostic;
mod editor_grammar;
mod formatter;
mod generate;
mod hir;
mod interfaces;
mod interpreter;
mod lexer;
mod lint;
mod package;
mod review;
mod runtime_abi;
mod rust_lower;
mod symbols;
pub mod syntax;

pub use analyzer::{
    analyze_source, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    analyze_sources_with_interfaces, analyze_sources_with_interfaces_without_core,
    analyze_syntax_source, core_interfaces, standard_package_interfaces,
};
pub use core_index::core_package_index_json;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Severity, Span, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
    format_diagnostics_json_with_source,
};
pub use editor_grammar::{VSCODE_GRAMMAR_PATH, vscode_tmlanguage_json};
pub use formatter::{format_program, format_source};
pub use generate::{
    CommitBehavior, Completion, CompletionKind, ContinuationOptions, Continuations, Effect,
    ExpectedType, GenerateContext, LiteralClass, PrefixStatus, SymbolCompleteness, TextRange,
    TypeRef, prefix_status, valid_continuations,
};
pub use interpreter::{EvalError, EvalOutput, eval_source_main};
pub use lint::lint_source;
pub use package::{
    PackageArchiveFile, PackageCheck, PackageCheckLock, PackageDependencyKind, PackageDiff,
    PackageGraphCheck, PackageIdentity, PackageInterfaceChange, PackageInterfaceChangeKind,
    PackageLock, PackageLockDiff, PackageLockFieldChange, PackageLockMetadata, PackageLockPackage,
    PackageLockPackageChange, PackageLoweringInput, PackageManifestChange, PackageMetadataMismatch,
    PackageMetadataReport, PackageNativeRustAuthorDeclaration, PackageNativeRustCheck,
    PackageNativeRustReview, PackageNativeRustSemanticReview, PackageNativeRustSourceScan,
    PackagePublishCheck, PackagePublishDryRun, PackageRegistryIndexEntry,
    PackageRegistryPublishTarget, PackageReview, PackageReviewExport, PackageReviewFile,
    PackageReviewFileKind, PackageReviewMetadata, PackageReviewSummary, PackageRisk,
    PackageSourceFile, PackageTree, PackageTreeNode, PackageTreeSummary, PackageVendorEntry,
    PackageVendorReport, PackageVendorUnresolved, check_package_dir, diff_package_dirs,
    diff_package_locks, format_package_check_human, format_package_check_json,
    format_package_check_reir_json, format_package_diff_human, format_package_diff_json,
    format_package_lock_diff_human, format_package_lock_diff_json,
    format_package_lock_diff_reir_json, format_package_lock_json, format_package_lock_reir_json,
    format_package_lock_reir_json_with_path, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_metadata_reir_json,
    format_package_publish_human, format_package_publish_json, format_package_publish_reir_json,
    format_package_review_human, format_package_review_json, format_package_review_reir_diff_json,
    format_package_review_reir_json, format_package_tree_human, format_package_tree_json,
    format_package_tree_reir_json, format_package_vendor_human, format_package_vendor_json,
    format_package_vendor_reir_json, lock_package_dir, package_lowering_input, package_metadata,
    package_metadata_verify, package_sources, package_sources_with_dependency_interfaces,
    package_tree, publish_package_dry_run, publish_package_dry_run_with_registry,
    review_package_dir, vendor_package_dir,
};
pub use review::{
    ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary, ReviewMapClassification,
    ReviewMapFile, ReviewMapFileRisk, ReviewMapRegion, ReviewMapSummary, ReviewRisk,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    review_map_sources, review_sources,
};
pub use rust_lower::{
    GeneratedRustPackage, LoweredRust, NativeRustDependency, RemappedRustcDiagnostic,
    RustBackendCheckResult, RustSourceMapEntry, check_generated_rust_package,
    lower_program_to_rust, lower_program_to_rust_with_map, lower_source_to_rust,
    lower_source_to_rust_package, lower_source_to_rust_package_with_interfaces,
    lower_source_to_rust_with_map, lower_sources_to_rust_package_with_interfaces,
    lower_sources_to_rust_package_with_options, parse_runtime_diagnostics, parse_source_map_json,
    remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines, write_generated_rust_package,
};
pub use symbols::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
