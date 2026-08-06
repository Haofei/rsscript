#![forbid(unsafe_code)]

mod analyzer;
#[cfg(feature = "execution")]
mod call_binding;
mod checks;
mod core_index;
mod diagnostic;
mod editor_grammar;
#[cfg(feature = "execution")]
mod eval_types;
#[cfg(feature = "execution")]
mod fnv;
mod formatter;
mod generate;
mod hir;
#[cfg(all(test, feature = "execution"))]
mod interface_metadata;
mod interfaces;
mod lexer {
    pub(crate) use rsscript_syntax::lexer::*;
}
mod lint;
#[cfg(feature = "native-plugin")]
mod native_plugin;
#[cfg(feature = "execution")]
mod package;
#[cfg(feature = "execution")]
mod reg_vm;
#[cfg(feature = "execution")]
mod review;
#[cfg(feature = "execution")]
mod runtime_abi;
#[cfg(feature = "execution")]
mod rust_lower;
#[cfg(all(test, feature = "execution"))]
mod selfhost_parity;
mod semantic;
mod symbols;
pub mod syntax;
#[cfg(all(test, feature = "execution"))]
mod test_interfaces;
mod text_util;
#[cfg(feature = "execution")]
mod vm_value;

pub use analyzer::{
    analyze_source, analyze_source_result, analyze_source_with_core,
    analyze_source_with_interfaces, analyze_source_with_interfaces_result,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    analyze_sources_with_interfaces, analyze_sources_with_interfaces_result,
    analyze_sources_with_interfaces_without_core,
    analyze_sources_with_interfaces_without_core_result, analyze_syntax_source, core_interfaces,
    standard_package_interfaces, validate_source, validate_sources_with_interfaces,
    validate_sources_with_interfaces_without_core,
};
pub use core_index::core_package_index_json;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Fix, FixEdit, Severity, Span, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
    format_diagnostics_json_with_source,
};
pub use editor_grammar::{VSCODE_GRAMMAR_PATH, vscode_tmlanguage_json};
#[cfg(feature = "execution")]
pub use eval_types::{
    BlockingBehavior, CancellationBehavior, CoverageBucket, EvalError, EvalOutput,
    ExternalFunction, ExternalFunctionRegistry, ExternalImport, ExternalSymbol, FunctionSignature,
    NativeValue, ProviderCallMode, ProviderDescriptor, ProviderFunction,
    ProviderFunctionDescriptor, ProviderLoadError, SignatureHash,
};
pub use formatter::{format_program, format_source};
pub use generate::{
    CommitBehavior, Completion, CompletionKind, ContinuationOptions, Continuations, Effect,
    ExpectedType, GenerateContext, LiteralClass, PrefixStatus, SymbolCompleteness, TextRange,
    TypeRef, prefix_status, valid_continuations,
};
pub use lint::lint_source;
#[cfg(feature = "native-plugin")]
pub use native_plugin::{load_package_bindings, load_package_bindings_from_snapshot};
#[cfg(all(feature = "execution", not(feature = "native-plugin")))]
pub fn load_package_bindings(
    package_dir: &std::path::Path,
) -> Result<Vec<(String, ExternalFunction)>, String> {
    let prepared = prepare_package_for_execution(package_dir)?;
    if prepared.requires_external_provider() {
        Err("native plugin loading is disabled; rebuild the host with `native-plugin`".to_string())
    } else {
        Ok(Vec::new())
    }
}
#[cfg(all(feature = "execution", not(feature = "native-plugin")))]
pub fn load_package_bindings_from_snapshot(
    package: &ExecutablePackageSnapshot,
) -> Result<Vec<(String, ExternalFunction)>, String> {
    if package.lowering_input().native_dependencies.is_empty() {
        Ok(Vec::new())
    } else {
        Err("native plugin loading is disabled; rebuild the host with `native-plugin`".to_string())
    }
}
#[cfg(feature = "execution")]
pub use package::{
    ArtifactStore, ExecutablePackageSnapshot, PackageAnalysis, PackageAnalysisAwaitSite,
    PackageAnalysisExport, PackageAnalysisExternalImport, PackageAnalysisProducer,
    PackageAnalysisSummary, PackageCheck, PackageCheckLock, PackageDependencyKind, PackageDiff,
    PackageGraphCheck, PackageIdentity, PackageInterfaceChange, PackageInterfaceChangeKind,
    PackageLock, PackageLockDiff, PackageLockFieldChange, PackageLockMetadata, PackageLockPackage,
    PackageLockPackageChange, PackageLoweringInput, PackageManifestChange, PackageMetadataMismatch,
    PackageMetadataReport, PackageNativeRustAuthorDeclaration, PackageNativeRustCheck,
    PackageNativeRustReview, PackageNativeRustSemanticReview, PackageNativeRustSourceScan,
    PackageReview, PackageReviewExport, PackageReviewFile, PackageReviewFileKind,
    PackageReviewMetadata, PackageReviewSummary, PackageRisk, PackageSourceFile, PackageTree,
    PackageTreeNode, PackageTreeSummary, PreparedPackage, analyze_package_dir, check_package_dir,
    configure_reduced_build_environment, diff_package_dirs, diff_package_locks,
    format_package_analysis_json, format_package_check_human, format_package_check_json,
    format_package_diff_human, format_package_diff_json, format_package_lock_diff_human,
    format_package_lock_diff_json, format_package_lock_json, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_review_human,
    format_package_review_json, format_package_review_markdown, format_package_tree_human,
    format_package_tree_json, lock_package_dir, package_lowering_input, package_metadata,
    package_metadata_verify, package_sources, package_sources_with_dependency_interfaces,
    package_tree, prepare_executable_package, prepare_package_for_execution, review_package_dir,
    write_package_artifact_atomic,
};
#[cfg(feature = "execution")]
pub use reg_vm::{
    JitPlan, RegVmExecutable, VmLimits, reg_vm_compile_package, reg_vm_compile_package_input,
    reg_vm_compile_source, reg_vm_compile_validated, reg_vm_eval_package_main_with_args,
    reg_vm_eval_package_main_with_args_and_external_bindings,
    reg_vm_eval_package_main_with_args_and_external_bindings_and_limits,
    reg_vm_eval_package_main_with_args_and_external_bindings_streaming_stdout,
    reg_vm_eval_source_main, reg_vm_eval_source_main_jit, reg_vm_eval_source_main_with_args,
    reg_vm_eval_source_main_with_args_and_external_bindings,
    reg_vm_eval_source_main_with_args_and_external_bindings_and_limits,
    reg_vm_eval_source_main_with_args_streaming_stdout, reg_vm_eval_source_main_with_limits,
};
#[cfg(feature = "native-jit")]
pub use reg_vm::{
    NativeStats, reg_vm_eval_source_main_native,
    reg_vm_eval_source_main_native_force_all_safepoints,
    reg_vm_eval_source_main_native_force_deopt, reg_vm_eval_source_main_native_force_safepoint,
    reg_vm_eval_source_main_native_osr, reg_vm_eval_source_main_native_osr_report,
    reg_vm_eval_source_main_native_precise, with_native_cost_model_disabled,
};
#[cfg(feature = "execution")]
pub use review::{
    ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary, ReviewMapClassification,
    ReviewMapFile, ReviewMapFileRisk, ReviewMapRegion, ReviewMapSummary, ReviewRisk,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    review_map_sources, review_sources,
};
#[cfg(feature = "execution")]
pub use rsscript_bytecode::{
    BYTECODE_MAGIC, BYTECODE_SCHEMA, BytecodeArtifact, BytecodeError, BytecodeHeader,
    BytecodeLimits, BytecodeVerifier, VerifiedBytecode,
};
#[cfg(feature = "execution")]
pub use rust_lower::lowered_symbol_name;
#[cfg(feature = "execution")]
pub use rust_lower::{
    GeneratedRustPackage, LowerCoverageReport, LoweredRust, NativeRustDependency,
    RemappedRustcDiagnostic, RustBackendCheckResult, RustSourceMapEntry,
    check_generated_rust_package, lower_coverage_report, lower_program_to_rust,
    lower_program_to_rust_with_map, lower_source_to_rust, lower_source_to_rust_package,
    lower_source_to_rust_package_with_interfaces, lower_source_to_rust_with_map,
    lower_sources_to_rust_package_with_interfaces, lower_sources_to_rust_package_with_options,
    parse_runtime_diagnostics, parse_source_map_json, remap_rustc_diagnostic_json,
    remap_rustc_diagnostic_json_lines, write_generated_rust_package,
};
pub use semantic::{
    AnalysisResult, FrontendCompletion, FrontendStopReason, SemanticDatabase, SourceFileSnapshot,
    SourceSnapshot, ValidatedProgram,
};
pub use symbols::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
#[cfg(feature = "execution")]
pub use symbols::{SymbolInventoryEntry, symbol_inventory};
