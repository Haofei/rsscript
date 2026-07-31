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
pub mod bbom_policy;
pub mod bbom_reir;
mod call_binding;
mod capability;
mod checks;
mod core_index;
mod default_read_migration;
mod diagnostic;
mod editor_grammar;
mod eval_types;
mod execution_policy;
mod fnv;
mod formatter;
mod generate;
mod hir;
#[cfg(test)]
mod interface_metadata;
mod interfaces;
mod lexer;
mod lint;
mod native_plugin;
mod package;
mod reg_vm;
mod review;
mod runtime_abi;
mod rust_lower;
#[cfg(test)]
mod selfhost_parity;
mod semantic;
mod symbols;
pub mod syntax;
mod text_util;
mod vm_coverage;
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
pub use capability::{
    CAPABILITY_CATEGORIES, CapabilityCategory, CapabilityRisk, capability_category,
    capability_risk, is_known_capability_category,
};
pub use core_index::core_package_index_json;
pub use default_read_migration::default_read_migration_edits;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Fix, FixEdit, Severity, Span, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
    format_diagnostics_json_with_source,
};
pub use editor_grammar::{VSCODE_GRAMMAR_PATH, vscode_tmlanguage_json};
pub use eval_types::{CoverageBucket, EvalError, EvalOutput, NativeInterpreterFn, NativeValue};
pub use execution_policy::{
    AuthorityError, AuthorizedEndpoint, AuthorizedExecutable, AuthorizedPath, DeploymentProfile,
    ExecutionCapability, ExecutionContext, ExecutionContextError, ExecutionPolicyError,
    ExecutionScopeId, HostAuthority, HostCapabilities, NetworkEndpointGrant,
    ParseDeploymentProfileError, ScopedHostAdapters, SupportLevel,
};
pub use formatter::{format_program, format_source};
pub use generate::{
    CommitBehavior, Completion, CompletionKind, ContinuationOptions, Continuations, Effect,
    ExpectedType, GenerateContext, LiteralClass, PrefixStatus, SymbolCompleteness, TextRange,
    TypeRef, prefix_status, valid_continuations,
};
pub use lint::lint_source;
pub use native_plugin::{load_authorized_package_native_bindings, load_package_native_bindings};
pub use package::{
    ArtifactStore, AuthorizedPackage, PackageCheck, PackageCheckLock, PackageDependencyKind,
    PackageDiff, PackageGraphCheck, PackageIdentity, PackageInterfaceChange,
    PackageInterfaceChangeKind, PackageLock, PackageLockDiff, PackageLockFieldChange,
    PackageLockMetadata, PackageLockPackage, PackageLockPackageChange, PackageLoweringInput,
    PackageManifestChange, PackageMetadataMismatch, PackageMetadataReport,
    PackageNativeRustAuthorDeclaration, PackageNativeRustCheck, PackageNativeRustReview,
    PackageNativeRustSemanticReview, PackageNativeRustSourceScan, PackageReview,
    PackageReviewExport, PackageReviewFile, PackageReviewFileKind, PackageReviewMetadata,
    PackageReviewSummary, PackageRisk, PackageSourceFile, PackageTree, PackageTreeNode,
    PackageTreeSummary, PreparedPackage, check_package_dir, configure_reduced_build_environment,
    diff_package_dirs, diff_package_locks, format_package_check_human, format_package_check_json,
    format_package_check_reir_json, format_package_diff_human, format_package_diff_json,
    format_package_lock_diff_human, format_package_lock_diff_json,
    format_package_lock_diff_reir_json, format_package_lock_json, format_package_lock_reir_json,
    format_package_lock_reir_json_with_path, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_metadata_reir_json,
    format_package_review_human, format_package_review_json, format_package_review_markdown,
    format_package_review_reir_diff_json, format_package_review_reir_json,
    format_package_tree_human, format_package_tree_json, format_package_tree_reir_json,
    lock_package_dir, package_lowering_input, package_metadata, package_metadata_verify,
    package_sources, package_sources_with_dependency_interfaces, package_tree,
    prepare_authorized_package, prepare_package_for_execution, review_package_dir,
    write_package_artifact_atomic,
};
pub use reg_vm::{
    JitPlan, RegVmExecutable, VmLimits, reg_vm_compile_package, reg_vm_compile_package_input,
    reg_vm_compile_source, reg_vm_compile_validated, reg_vm_eval_package_main_with_args,
    reg_vm_eval_package_main_with_args_and_native_bindings,
    reg_vm_eval_package_main_with_args_and_native_bindings_and_limits,
    reg_vm_eval_package_main_with_args_and_native_bindings_streaming_stdout,
    reg_vm_eval_source_main, reg_vm_eval_source_main_jit, reg_vm_eval_source_main_with_args,
    reg_vm_eval_source_main_with_args_and_native_bindings,
    reg_vm_eval_source_main_with_args_and_native_bindings_and_limits,
    reg_vm_eval_source_main_with_args_streaming_stdout,
    reg_vm_eval_source_main_with_context_and_limits, reg_vm_eval_source_main_with_limits,
};
#[cfg(feature = "native-jit")]
pub use reg_vm::{
    NativeStats, reg_vm_eval_source_main_native,
    reg_vm_eval_source_main_native_force_all_safepoints,
    reg_vm_eval_source_main_native_force_deopt, reg_vm_eval_source_main_native_force_safepoint,
    reg_vm_eval_source_main_native_osr, reg_vm_eval_source_main_native_osr_report,
    reg_vm_eval_source_main_native_precise, with_native_cost_model_disabled,
};
pub use review::{
    ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary, ReviewMapClassification,
    ReviewMapFile, ReviewMapFileRisk, ReviewMapRegion, ReviewMapSummary, ReviewRisk,
    format_review_human, format_review_json, format_review_map_human, format_review_map_json,
    review_map_sources, review_sources,
};
pub use rust_lower::lowered_symbol_name;
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
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolInventoryEntry,
    SymbolKind, SymbolLookup, document_symbols, symbol_index, symbol_inventory,
};
pub use vm_coverage::{VmCoverageReport, vm_coverage_report};

/// Versioned, stable entrypoints for embedding RSScript.
pub mod api {
    /// The first curated, versioned public API surface.
    ///
    /// RSScript remains `0.1.x`; this namespace controls growth and migration
    /// without declaring a SemVer stability guarantee.
    pub mod v1 {
        /// Source analysis, language tooling, and lowering.
        pub mod frontend {
            pub use crate::syntax;
            pub use crate::{
                AnalysisResult, CAPABILITY_CATEGORIES, CapabilityCategory, CapabilityRisk,
                CommitBehavior, Completion, CompletionKind, ContinuationOptions, Continuations,
                Definition, Effect, ExpectedType, FrontendCompletion, FrontendStopReason,
                GenerateContext, GeneratedRustPackage, LiteralClass, LowerCoverageReport,
                LoweredRust, NativeRustDependency, PrefixStatus, Reference,
                RemappedRustcDiagnostic, RssDocumentSymbol, RustBackendCheckResult,
                RustSourceMapEntry, SemanticDatabase, SourceFileSnapshot, SourceSnapshot,
                SymbolCompleteness, SymbolIndex, SymbolInfo, SymbolInventoryEntry, SymbolKind,
                SymbolLookup, TextRange, TypeRef, VSCODE_GRAMMAR_PATH, ValidatedProgram,
                analyze_source, analyze_source_result, analyze_source_with_core,
                analyze_source_with_interfaces, analyze_source_with_interfaces_result,
                analyze_source_with_interfaces_without_core, analyze_source_without_core,
                analyze_sources_with_interfaces, analyze_sources_with_interfaces_result,
                analyze_sources_with_interfaces_without_core,
                analyze_sources_with_interfaces_without_core_result, analyze_syntax_source,
                capability_category, capability_risk, check_generated_rust_package,
                core_interfaces, core_package_index_json, default_read_migration_edits,
                document_symbols, format_program, format_source, is_known_capability_category,
                lint_source, lower_coverage_report, lower_program_to_rust,
                lower_program_to_rust_with_map, lower_source_to_rust, lower_source_to_rust_package,
                lower_source_to_rust_package_with_interfaces, lower_source_to_rust_with_map,
                lower_sources_to_rust_package_with_interfaces,
                lower_sources_to_rust_package_with_options, lowered_symbol_name,
                parse_runtime_diagnostics, parse_source_map_json, prefix_status,
                remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines,
                standard_package_interfaces, symbol_index, symbol_inventory, valid_continuations,
                validate_source, validate_sources_with_interfaces,
                validate_sources_with_interfaces_without_core, vscode_tmlanguage_json,
                write_generated_rust_package,
            };
        }

        /// Structured diagnostics and their renderers.
        pub mod diagnostics {
            pub use crate::{
                Diagnostic, DiagnosticExplanation, Fix, FixEdit, Severity, Span,
                explain_diagnostic_code, format_diagnostic_explanation, format_diagnostics_human,
                format_diagnostics_json, format_diagnostics_json_with_source,
            };
        }

        /// Source and package review models and renderers.
        pub mod review {
            pub use crate::{
                ReviewFinding, ReviewFix, ReviewMap, ReviewMapCategorySummary,
                ReviewMapClassification, ReviewMapFile, ReviewMapFileRisk, ReviewMapRegion,
                ReviewMapSummary, ReviewRisk, format_review_human, format_review_json,
                format_review_map_human, format_review_map_json, review_map_sources,
                review_sources,
            };
        }

        /// Package preparation and inspection.
        pub mod package {
            pub use crate::{
                ArtifactStore, AuthorizedPackage, PackageCheck, PackageCheckLock,
                PackageDependencyKind, PackageDiff, PackageGraphCheck, PackageIdentity,
                PackageInterfaceChange, PackageInterfaceChangeKind, PackageLock, PackageLockDiff,
                PackageLockFieldChange, PackageLockMetadata, PackageLockPackage,
                PackageLockPackageChange, PackageLoweringInput, PackageManifestChange,
                PackageMetadataMismatch, PackageMetadataReport, PackageNativeRustAuthorDeclaration,
                PackageNativeRustCheck, PackageNativeRustReview, PackageNativeRustSemanticReview,
                PackageNativeRustSourceScan, PackageReview, PackageReviewExport, PackageReviewFile,
                PackageReviewFileKind, PackageReviewMetadata, PackageReviewSummary, PackageRisk,
                PackageSourceFile, PackageTree, PackageTreeNode, PackageTreeSummary,
                PreparedPackage, check_package_dir, configure_reduced_build_environment,
                diff_package_dirs, diff_package_locks, format_package_check_human,
                format_package_check_json, format_package_check_reir_json,
                format_package_diff_human, format_package_diff_json,
                format_package_lock_diff_human, format_package_lock_diff_json,
                format_package_lock_diff_reir_json, format_package_lock_json,
                format_package_lock_reir_json, format_package_lock_reir_json_with_path,
                format_package_lock_toml, format_package_metadata_human,
                format_package_metadata_json, format_package_metadata_reir_json,
                format_package_review_human, format_package_review_json,
                format_package_review_markdown, format_package_review_reir_diff_json,
                format_package_review_reir_json, format_package_tree_human,
                format_package_tree_json, format_package_tree_reir_json, lock_package_dir,
                package_lowering_input, package_metadata, package_metadata_verify, package_sources,
                package_sources_with_dependency_interfaces, package_tree,
                prepare_authorized_package, prepare_package_for_execution, review_package_dir,
                write_package_artifact_atomic,
            };
        }

        /// Register-VM compilation and execution.
        pub mod vm {
            pub use crate::{
                AuthorityError, AuthorizedEndpoint, AuthorizedExecutable, AuthorizedPath,
                CoverageBucket, DeploymentProfile, EvalError, EvalOutput, ExecutionCapability,
                ExecutionContext, ExecutionContextError, ExecutionPolicyError, ExecutionScopeId,
                HostAuthority, HostCapabilities, JitPlan, NativeInterpreterFn, NativeValue,
                NetworkEndpointGrant, ParseDeploymentProfileError, RegVmExecutable, SupportLevel,
                VmCoverageReport, VmLimits, load_authorized_package_native_bindings,
                load_package_native_bindings, reg_vm_compile_package, reg_vm_compile_package_input,
                reg_vm_compile_source, reg_vm_compile_validated,
                reg_vm_eval_package_main_with_args,
                reg_vm_eval_package_main_with_args_and_native_bindings,
                reg_vm_eval_package_main_with_args_and_native_bindings_and_limits,
                reg_vm_eval_package_main_with_args_and_native_bindings_streaming_stdout,
                reg_vm_eval_source_main, reg_vm_eval_source_main_jit,
                reg_vm_eval_source_main_with_args,
                reg_vm_eval_source_main_with_args_and_native_bindings,
                reg_vm_eval_source_main_with_args_and_native_bindings_and_limits,
                reg_vm_eval_source_main_with_args_streaming_stdout,
                reg_vm_eval_source_main_with_context_and_limits,
                reg_vm_eval_source_main_with_limits, vm_coverage_report,
            };
            #[cfg(feature = "native-jit")]
            pub use crate::{
                NativeStats, reg_vm_eval_source_main_native,
                reg_vm_eval_source_main_native_force_all_safepoints,
                reg_vm_eval_source_main_native_force_deopt,
                reg_vm_eval_source_main_native_force_safepoint, reg_vm_eval_source_main_native_osr,
                reg_vm_eval_source_main_native_osr_report, reg_vm_eval_source_main_native_precise,
                with_native_cost_model_disabled,
            };
        }
    }
}
