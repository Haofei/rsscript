#![forbid(unsafe_code)]

/// Build fingerprint used by external composition roots to invalidate compiler
/// output caches whenever the implementation inputs change.
pub const COMPILED_CACHE_FINGERPRINT: &str = env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT");

mod analyzer;
mod checks;
#[cfg(feature = "lowering")]
mod compiler_output;
mod core_index;
mod diagnostic {
    pub use rsscript_diagnostics::*;
}
mod editor_grammar;
mod formatter;
mod generate;
mod hir;
#[cfg(all(test, feature = "execution", feature = "selfhost-parity"))]
mod interface_metadata;
mod interfaces;
mod lexer {
    pub(crate) use rsscript_syntax::lexer::*;
}
mod lint;
#[cfg(feature = "execution")]
mod lower_names;
#[cfg(feature = "package")]
mod package;
#[cfg(feature = "package")]
mod review;
#[cfg(any(feature = "package", feature = "aot-rust"))]
mod runtime_abi;
#[cfg(feature = "aot-rust")]
mod rust_lower;
#[cfg(all(test, feature = "execution", feature = "selfhost-parity"))]
mod selfhost_parity;
mod semantic;
mod symbols;
pub mod syntax;
#[cfg(all(test, feature = "package"))]
mod test_interfaces;
#[allow(dead_code)]
mod text_util {
    pub(crate) use rsscript_text::*;
}
#[cfg(all(test, feature = "execution", feature = "selfhost-parity"))]
mod vm_adapter {
    use rsscript_vm::{EvalError, RegVmExecutable};

    pub(crate) fn reg_vm_compile_sources(
        sources: &[(&str, &str)],
    ) -> Result<RegVmExecutable, EvalError> {
        let interfaces = crate::interfaces::standard_package_interfaces().collect::<Vec<_>>();
        let validated = crate::analyzer::validate_sources_with_interfaces(sources, &interfaces)
            .map_err(EvalError::Diagnostics)?;
        let compiled = crate::compiler_output::compile_validated_to_ir(&validated);
        rsscript_vm::compile_executable_ir(
            compiled.executable(),
            compiled.source_hash(),
            compiled.interface_catalog_digest(),
        )
    }
}

#[cfg(all(test, feature = "execution", feature = "selfhost-parity"))]
use rsscript_vm::RegVmExecutable;

pub use analyzer::{
    analyze_frontend_input_snapshot_with_operation, analyze_source, analyze_source_result,
    analyze_source_result_with_operation, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_with_interfaces_result, analyze_source_with_interfaces_result_with_operation,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    analyze_sources_with_interfaces, analyze_sources_with_interfaces_result,
    analyze_sources_with_interfaces_result_with_operation,
    analyze_sources_with_interfaces_without_core,
    analyze_sources_with_interfaces_without_core_result, analyze_syntax_source, core_interfaces,
    standard_package_interfaces, validate_source, validate_source_with_operation,
    validate_sources_with_interfaces, validate_sources_with_interfaces_with_operation,
    validate_sources_with_interfaces_without_core,
};
#[cfg(all(feature = "lowering", feature = "package"))]
pub use compiler_output::compile_package_input_to_ir;
#[cfg(feature = "bytecode")]
pub use compiler_output::{
    BytecodeCompileError, compile_ir_to_bytecode, compile_validated_to_bytecode,
};
#[cfg(feature = "lowering")]
pub use compiler_output::{CompiledIr, compile_source_to_ir, compile_validated_to_ir};
pub use core_index::core_package_index_json;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Fix, FixEdit, Severity, Span, explain_diagnostic_code,
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
pub use lint::lint_source;
#[cfg(feature = "execution")]
pub use lower_names::lowered_symbol_name;

/// Explicit migration-only APIs for package persistence, review presentation,
/// and Rust AOT emission.
///
/// The reviewed compiler surface is source/interface analysis and provider-neutral
/// lowering. These historical APIs remain available only to compatibility
/// adapters and experimental tooling while their implementations move to their
/// respective project, review, and AOT boundaries.
#[cfg(any(feature = "package", feature = "aot-rust"))]
pub mod compatibility {
    #[cfg(all(feature = "lowering", feature = "package"))]
    pub use crate::compiler_output::compile_package_input_to_ir;
    #[cfg(feature = "bytecode")]
    pub use crate::compiler_output::{
        BytecodeCompileError, compile_ir_to_bytecode, compile_validated_to_bytecode,
    };
    #[cfg(feature = "lowering")]
    pub use crate::compiler_output::{CompiledIr, compile_source_to_ir, compile_validated_to_ir};
    #[cfg(feature = "execution")]
    pub use crate::lower_names::lowered_symbol_name;
    #[cfg(feature = "package")]
    pub use crate::package::{
        ExecutablePackageSnapshot, PackageAnalysis, PackageAnalysisAwaitSite,
        PackageAnalysisExport, PackageAnalysisExternalImport, PackageAnalysisFile,
        PackageAnalysisProducer, PackageAnalysisSummary, PackageCheck, PackageCheckLock,
        PackageDependencyKind, PackageDiff, PackageGraphCheck, PackageIdentity,
        PackageInterfaceChange, PackageInterfaceChangeKind, PackageLock, PackageLockDiff,
        PackageLockFieldChange, PackageLockMetadata, PackageLockPackage, PackageLockPackageChange,
        PackageLoweringInput, PackageManifestChange, PackageMetadataMismatch,
        PackageMetadataReport, PackageNativeRustAuthorDeclaration, PackageNativeRustCheck,
        PackageNativeRustReview, PackageNativeRustSemanticReview, PackageNativeRustSourceScan,
        PackageReview, PackageReviewExport, PackageReviewFile, PackageReviewFileKind,
        PackageReviewMetadata, PackageReviewSummary, PackageRisk, PackageSourceFile, PackageTree,
        PackageTreeNode, PackageTreeSummary, PreparedPackage, WorkspaceSnapshot,
        analyze_package_dir, check_package_dir, diff_package_dirs, diff_package_locks,
        format_package_analysis_json, format_package_check_human, format_package_check_json,
        format_package_diff_human, format_package_diff_json, format_package_lock_diff_human,
        format_package_lock_diff_json, format_package_lock_json, format_package_lock_toml,
        format_package_metadata_human, format_package_metadata_json, format_package_review_human,
        format_package_review_json, format_package_review_markdown, format_package_tree_human,
        format_package_tree_json, load_workspace_snapshot, load_workspace_snapshot_with_operation,
        lock_package_dir, package_lowering_input, package_metadata, package_metadata_verify,
        package_sources, package_sources_with_dependency_interfaces, package_tree,
        prepare_executable_package, prepare_package_for_execution, review_package_dir,
    };
    #[cfg(feature = "package")]
    pub use crate::review::{
        ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary, ReviewMapClassification,
        ReviewMapFile, ReviewMapFileRisk, ReviewMapRegion, ReviewMapSummary, ReviewRisk,
        format_review_human, format_review_json, format_review_map_human, format_review_map_json,
        review_map_sources, review_sources,
    };
    #[cfg(feature = "aot-rust")]
    pub use crate::rust_lower::{
        GeneratedRustPackage, LowerCoverageReport, LoweredRust, NativeRustDependency,
        RemappedRustcDiagnostic, RustSourceMapEntry, lower_coverage_report, lower_program_to_rust,
        lower_program_to_rust_with_map, lower_source_to_rust, lower_source_to_rust_package,
        lower_source_to_rust_package_with_interfaces, lower_source_to_rust_with_map,
        lower_sources_to_rust_package_with_interfaces, lower_sources_to_rust_package_with_options,
        parse_runtime_diagnostics, parse_source_map_json, remap_rustc_diagnostic_json,
        remap_rustc_diagnostic_json_lines, write_generated_rust_package,
    };
    #[cfg(feature = "execution")]
    pub use crate::symbols::{SymbolInventoryEntry, symbol_inventory};
}
#[cfg(feature = "execution")]
pub use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationId};
pub use semantic::{
    AnalysisResult, FrontendCompletion, FrontendInputSnapshot, FrontendStopReason,
    SemanticDatabase, SourceFileSnapshot, SourceSnapshot, ValidatedProgram,
};
pub use symbols::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
#[cfg(feature = "execution")]
pub use symbols::{SymbolInventoryEntry, symbol_inventory};
