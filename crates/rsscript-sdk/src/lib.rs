#![forbid(unsafe_code)]

// Public exports are deliberately explicit so adding a compiler, bytecode, or
// VM symbol cannot silently expand the embedding contract.
#[cfg(feature = "compatibility")]
pub use rsscript_compiler::syntax;
#[cfg(not(feature = "compatibility"))]
#[allow(unused_imports)]
use rsscript_compiler::*;
#[cfg(feature = "compatibility")]
pub use rsscript_compiler::{
    AnalysisResult, CommitBehavior, Completion, CompletionKind, ContinuationOptions, Continuations,
    Definition, Diagnostic, DiagnosticExplanation, Effect, ExpectedType, Fix, FixEdit,
    FrontendCompletion, FrontendInputSnapshot, FrontendStopReason, GenerateContext, LiteralClass,
    PrefixStatus, Reference, RssDocumentSymbol, SemanticDatabase, Severity, SourceFileSnapshot,
    SourceSnapshot, Span, SymbolCompleteness, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    TextRange, TypeRef, VSCODE_GRAMMAR_PATH, ValidatedProgram, analyze_source,
    analyze_source_result, analyze_source_result_with_operation, analyze_source_with_core,
    analyze_source_with_interfaces, analyze_source_with_interfaces_result,
    analyze_source_with_interfaces_result_with_operation,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
    analyze_sources_with_interfaces, analyze_sources_with_interfaces_result,
    analyze_sources_with_interfaces_result_with_operation,
    analyze_sources_with_interfaces_without_core,
    analyze_sources_with_interfaces_without_core_result, analyze_syntax_source, core_interfaces,
    core_package_index_json, document_symbols, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
    format_diagnostics_json_with_source, format_program, format_source, lint_source, prefix_status,
    standard_package_interfaces, symbol_index, valid_continuations, validate_source,
    validate_source_with_operation, validate_sources_with_interfaces,
    validate_sources_with_interfaces_with_operation, validate_sources_with_interfaces_without_core,
    vscode_tmlanguage_json,
};

#[cfg(feature = "compatibility")]
pub use rsscript_artifact_store::{ArtifactStore, write_package_artifact_atomic};
#[cfg(feature = "execution")]
#[cfg(not(feature = "compatibility"))]
#[allow(unused_imports)]
use rsscript_bytecode::*;
#[cfg(feature = "compatibility")]
pub use rsscript_bytecode::{
    BYTECODE_CONTAINER_FORMAT_VERSION, BYTECODE_ISA_VERSION, BYTECODE_MAGIC, BYTECODE_SCHEMA,
    BytecodeArtifact, BytecodeCompatibility, BytecodeError, BytecodeErrorCode, BytecodeHeader,
    BytecodeLimits, BytecodeVerifier, LANGUAGE_SEMANTICS_VERSION, SUPPORTED_LANGUAGE_SEMANTICS,
    VerificationContext, VerifiedBytecode, decode_executable_payload, encode_executable_payload,
};
#[cfg(feature = "compatibility")]
pub use rsscript_compiler::compatibility::{
    CompiledIr, ExecutablePackageSnapshot, GeneratedRustPackage, LowerCoverageReport, LoweredRust,
    NativeRustDependency, PackageAnalysis, PackageAnalysisAwaitSite, PackageAnalysisExport,
    PackageAnalysisExternalImport, PackageAnalysisFile, PackageAnalysisProducer,
    PackageAnalysisSummary, PackageCheck, PackageCheckLock, PackageDependencyKind, PackageDiff,
    PackageGraphCheck, PackageIdentity, PackageInterfaceChange, PackageInterfaceChangeKind,
    PackageLock, PackageLockDiff, PackageLockFieldChange, PackageLockMetadata, PackageLockPackage,
    PackageLockPackageChange, PackageLoweringInput, PackageManifestChange, PackageMetadataMismatch,
    PackageMetadataReport, PackageNativeRustAuthorDeclaration, PackageNativeRustCheck,
    PackageNativeRustReview, PackageNativeRustSemanticReview, PackageNativeRustSourceScan,
    PackageReview, PackageReviewExport, PackageReviewFile, PackageReviewFileKind,
    PackageReviewMetadata, PackageReviewSummary, PackageRisk, PackageSourceFile, PackageTree,
    PackageTreeNode, PackageTreeSummary, PreparedPackage, RemappedRustcDiagnostic, ReviewFix,
    ReviewMap, ReviewMapCategorySummary, ReviewMapClassification, ReviewMapFile, ReviewMapFileRisk,
    ReviewMapRegion, ReviewMapSummary, ReviewRisk, SymbolInventoryEntry, WorkspaceSnapshot,
    analyze_package_dir, check_package_dir, compile_ir_to_bytecode, compile_package_input_to_ir,
    compile_source_to_ir, compile_validated_to_bytecode, compile_validated_to_ir,
    diff_package_dirs, diff_package_locks, format_package_check_human, format_package_check_json,
    format_package_diff_human, format_package_diff_json, format_package_lock_diff_human,
    format_package_lock_diff_json, format_package_lock_json, format_package_lock_toml,
    format_package_metadata_human, format_package_metadata_json, format_package_review_human,
    format_package_review_json, format_package_review_markdown, format_package_tree_human,
    format_package_tree_json, format_review_human, format_review_json, format_review_map_human,
    format_review_map_json, load_workspace_snapshot, load_workspace_snapshot_with_operation,
    lock_package_dir, lower_coverage_report, lower_program_to_rust, lower_program_to_rust_with_map,
    lower_source_to_rust, lower_source_to_rust_package,
    lower_source_to_rust_package_with_interfaces, lower_source_to_rust_with_map,
    lower_sources_to_rust_package_with_interfaces, lower_sources_to_rust_package_with_options,
    lowered_symbol_name, package_lowering_input, package_metadata, package_metadata_verify,
    package_sources, package_sources_with_dependency_interfaces, package_tree,
    parse_runtime_diagnostics, parse_source_map_json, prepare_executable_package,
    prepare_package_for_execution, remap_rustc_diagnostic_json, remap_rustc_diagnostic_json_lines,
    review_map_sources, review_package_dir, review_sources, symbol_inventory,
    write_generated_rust_package,
};
#[cfg(feature = "execution")]
#[cfg(not(feature = "compatibility"))]
#[allow(unused_imports)]
use rsscript_compiler::*;
#[cfg(feature = "native-jit")]
pub use rsscript_vm::NativeStats;
#[cfg(feature = "execution")]
#[cfg(not(feature = "compatibility"))]
#[allow(unused_imports)]
use rsscript_vm::*;
#[cfg(feature = "compatibility")]
pub use rsscript_vm::{
    AsyncInterpreterFn, AsyncProviderCallContext, BlockingBehavior, CancellationBehavior,
    CoverageBucket, EvalError, EvalExecutionReport, EvalOutput, ExecutionFailureKind,
    ExecutionUsage, ExternalFunction, ExternalFunctionRegistry, ExternalImport, ExternalSymbol,
    FunctionSignature, HostCallContext, NativeInterpreterFn, NativeValue, ProviderCallContext,
    ProviderCallMode, ProviderCallTrace, ProviderCallable, ProviderDescriptor, ProviderError,
    ProviderErrorCode, ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor,
    ProviderFuture, ProviderInvocationContract, ProviderLoadError, ProviderResource,
    ProviderResourceRegistry, ProviderResourceTable, ProviderTraceSink, RegVmExecutable,
    ResolvedProviderFunction, ResourceCleanupContract, ResourceHandle, SignatureHash, VmLimits,
    compile_executable_ir,
};
#[cfg(feature = "compatibility")]
#[allow(dead_code)]
mod vm_adapter;
#[cfg(feature = "execution")]
pub use rsscript_artifact::{
    ARTIFACT_BUNDLE_MAGIC, ARTIFACT_BUNDLE_SCHEMA, AnalysisEnvelopeV1, AnalysisSchemaV1,
    ArtifactBundle, ArtifactBundleError, ArtifactIdentityV1, AwaitFactV1, BuildProvenanceV1,
    CallEdgeFactV1, ChangedFactV1, CountChangeV1, DiagnosticFactV1, ExportFactV1,
    ExternalCallFactV1, ExternalContractFactV1, FactSetDiffV1, FunctionParameterFactV1,
    InterfaceRequirementV1, PACKAGE_ANALYSIS_SCHEMA, ResourceLifetimeFactV1,
    ResourceTransferFactV1, SEMANTIC_DIFF_SCHEMA, SOURCE_ANALYSIS_SCHEMA, SemanticDiffV1,
    SourceAnalysisV1, TaskGroupFactV1,
};
#[cfg(feature = "execution")]
use sha2::{Digest, Sha256};
#[cfg(feature = "execution")]
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
#[cfg(feature = "project")]
use std::path::Path;
#[cfg(feature = "execution")]
use std::time::{Duration, Instant};
#[cfg(feature = "compatibility")]
pub use vm_adapter::{
    reg_vm_compile_mir, reg_vm_compile_package, reg_vm_compile_package_input,
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
pub use vm_adapter::{
    reg_vm_eval_source_main_native, reg_vm_eval_source_main_native_force_all_safepoints,
    reg_vm_eval_source_main_native_force_deopt, reg_vm_eval_source_main_native_force_safepoint,
    reg_vm_eval_source_main_native_osr, reg_vm_eval_source_main_native_osr_report,
    reg_vm_eval_source_main_native_precise, with_native_cost_model_disabled,
};

/// Frontend-only editor API consumed by `rsscript-language-service`.
/// Runtime and Provider types are deliberately excluded.
pub mod language {
    pub use rsscript_compiler::{
        Definition, Diagnostic, DiagnosticExplanation, Reference, RssDocumentSymbol, Severity,
        Span, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
        analyze_source_result_with_operation, analyze_source_with_core,
        analyze_source_with_interfaces, analyze_source_with_interfaces_result_with_operation,
        analyze_sources_with_interfaces, analyze_sources_with_interfaces_result_with_operation,
        document_symbols, explain_diagnostic_code, format_source, lint_source, symbol_index,
    };
}

/// Reviewed compilation entry points and diagnostics.
pub mod compile {
    pub use super::{CompileError, CompileErrorCode, Compiler};
    pub use rsscript_compiler::FrontendInputSnapshot;
    pub use rsscript_compiler::{Diagnostic, Severity};
}

/// Explicit filesystem/package convenience APIs.
///
/// This module is intentionally separate from [`compile`]. The reviewed
/// [`Compiler`] takes only immutable in-memory snapshots, while this adapter
/// owns path capture for CLI and project-oriented hosts.
#[cfg(feature = "project")]
pub mod project {
    use super::*;
    use rsscript_workspace_loader::{
        WorkspaceFileKind, WorkspaceLoadError, WorkspaceLoadErrorCode, WorkspaceLoader,
        WorkspaceSnapshot as CapturedWorkspaceSnapshot,
    };

    /// Immutable frontend input captured by the OS-facing workspace loader.
    ///
    /// This is deliberately distinct from the legacy compiler
    /// [`WorkspaceSnapshot`], which still carries package-review and native
    /// authorization compatibility state. New embedders can pass
    /// [`Self::frontend`] directly to the pure in-memory [`Compiler`] API.
    #[derive(Debug, Clone)]
    pub struct CapturedProjectSnapshot {
        workspace: CapturedWorkspaceSnapshot,
        frontend: FrontendInputSnapshot,
    }

    impl CapturedProjectSnapshot {
        pub fn frontend(&self) -> &FrontendInputSnapshot {
            &self.frontend
        }

        /// Stable source/interface identity from the loader. It excludes
        /// physical host paths and therefore remains comparable across
        /// equivalent captures on different machines.
        pub fn content_digest(&self) -> &str {
            self.workspace.content_digest()
        }

        /// Digest of exactly the source and interface files passed to the
        /// in-memory compiler. Unlike [`Self::content_digest`], this excludes
        /// test-only files retained by the project snapshot.
        pub fn frontend_digest(&self) -> String {
            frontend_snapshot_digest(&self.frontend)
        }

        pub fn files(&self) -> &[rsscript_workspace_loader::WorkspaceSourceFile] {
            self.workspace.files()
        }
    }

    #[derive(Default)]
    pub struct ProjectCompiler;

    impl ProjectCompiler {
        pub fn new() -> Self {
            Self
        }

        /// Capture a project through the explicit-base workspace loader and
        /// convert its executable source/interface files into the compiler's
        /// immutable frontend boundary. Test files remain available in the
        /// captured workspace but are intentionally excluded from a normal
        /// build input.
        ///
        /// This method never reads the process current directory. It is the
        /// preferred project entry point for hosts that do not need legacy
        /// package review or native-authorization compatibility APIs.
        pub fn capture_frontend_from(
            &self,
            base: &Path,
            package_dir: &Path,
        ) -> Result<CapturedProjectSnapshot, CompileError> {
            self.capture_frontend_inner(base, package_dir, None)
        }

        /// Operation-aware capture that keeps cancellation and deadline checks
        /// in the loader boundary before immutable frontend input is created.
        pub fn capture_frontend_from_with_operation(
            &self,
            base: &Path,
            package_dir: &Path,
            operation: &OperationContext,
        ) -> Result<CapturedProjectSnapshot, CompileError> {
            self.capture_frontend_inner(base, package_dir, Some(operation))
        }

        fn capture_frontend_inner(
            &self,
            base: &Path,
            package_dir: &Path,
            operation: Option<&OperationContext>,
        ) -> Result<CapturedProjectSnapshot, CompileError> {
            let loader = WorkspaceLoader::default();
            let workspace = match operation {
                Some(operation) => {
                    loader.snapshot_from_with_operation(base, package_dir, operation)
                }
                None => loader.snapshot_from(base, package_dir),
            }
            .map_err(map_workspace_load_error)?;
            let sources = workspace
                .files()
                .iter()
                .filter(|file| file.kind == WorkspaceFileKind::Source)
                .map(|file| (file.logical_path.as_str(), file.contents.as_str()))
                .collect::<Vec<_>>();
            let interfaces = workspace
                .files()
                .iter()
                .filter(|file| file.kind == WorkspaceFileKind::Interface)
                .map(|file| (file.logical_path.as_str(), file.contents.as_str()))
                .collect::<Vec<_>>();
            let frontend = FrontendInputSnapshot::from_sources(sources, interfaces);
            Ok(CapturedProjectSnapshot {
                workspace,
                frontend,
            })
        }

        /// Build exactly the source/interface snapshot captured by
        /// [`Self::capture_frontend_from`]. This is the project-level route
        /// that does not reread paths or reconstruct compiler inputs.
        pub fn build_captured(
            &self,
            snapshot: &CapturedProjectSnapshot,
        ) -> Result<BuiltArtifact, CompileError> {
            let built = Compiler.compile_snapshot(snapshot.frontend())?;
            debug_assert_eq!(built.snapshot_digest(), snapshot.frontend_digest());
            Ok(built)
        }

        /// Operation-aware counterpart of [`Self::build_captured`]. The
        /// loader capture remains immutable; cancellation and deadline checks
        /// apply to the pure compiler work without reopening filesystem input.
        pub fn build_captured_with_operation(
            &self,
            snapshot: &CapturedProjectSnapshot,
            operation: &OperationContext,
        ) -> Result<BuiltArtifact, CompileError> {
            let built = Compiler.compile_snapshot_with_operation(snapshot.frontend(), operation)?;
            debug_assert_eq!(built.snapshot_digest(), snapshot.frontend_digest());
            Ok(built)
        }

        pub fn compile_package(&self, path: &Path) -> Result<BuiltArtifact, CompileError> {
            let captured = self.capture_frontend_from(path, Path::new("."))?;
            self.build_captured(&captured)
        }

        pub fn compile_package_with_operation(
            &self,
            path: &Path,
            operation: &OperationContext,
        ) -> Result<BuiltArtifact, CompileError> {
            let captured =
                self.capture_frontend_from_with_operation(path, Path::new("."), operation)?;
            self.build_captured_with_operation(&captured, operation)
        }
    }

    /// Legacy package-analysis and native-compatibility operations.
    ///
    /// This module deliberately remains opt-in because its snapshot contains
    /// package review and native authorization state. New callers must use
    /// [`ProjectCompiler::capture_frontend_from`] followed by
    /// [`ProjectCompiler::build_captured`], which keeps the compiler on its
    /// immutable in-memory input boundary.
    #[cfg(feature = "compatibility")]
    pub mod legacy {
        use super::*;

        pub use rsscript_compiler::WorkspaceSnapshot;

        pub struct PackageCompatibility;

        impl PackageCompatibility {
            /// Capture all package and dependency inputs exactly once.
            pub fn snapshot(path: &Path) -> Result<WorkspaceSnapshot, CompileError> {
                load_workspace_snapshot(path).map_err(|message| CompileError::Package {
                    code: CompileErrorCode::PackageSnapshot,
                    message,
                })
            }

            /// Capture a package while honoring a caller operation boundary.
            pub fn snapshot_with_operation(
                path: &Path,
                operation: &OperationContext,
            ) -> Result<WorkspaceSnapshot, CompileError> {
                operation.check().map_err(CompileError::from)?;
                load_workspace_snapshot_with_operation(path, operation).map_err(|message| {
                    match operation.check() {
                        Ok(()) => CompileError::Package {
                            code: CompileErrorCode::PackageSnapshot,
                            message,
                        },
                        Err(abort) => CompileError::from(abort),
                    }
                })
            }

            /// Build compatibility package evidence from one immutable legacy
            /// snapshot. This path is not part of the reviewed project API.
            pub fn build(snapshot: &WorkspaceSnapshot) -> Result<BuiltArtifact, CompileError> {
                let mut analysis = snapshot.analysis().clone();
                if analysis.summary.errors != 0 {
                    return Err(CompileError::Diagnostics(analysis.diagnostics.clone()));
                }
                let compiled = compile_package_input_to_ir(snapshot.lowering_input())
                    .map_err(CompileError::Diagnostics)?;
                let artifact = compile_ir_to_bytecode(&compiled, snapshot.digest())
                    .map_err(bytecode_compile_error)?;
                analysis.module_digest = Some(artifact.header.executable_hash.clone());
                let analysis = AnalysisEnvelopeV1::package(analysis).map_err(CompileError::from)?;
                BuiltArtifact::from_bytecode(artifact, analysis)
            }

            pub fn build_with_operation(
                snapshot: &WorkspaceSnapshot,
                operation: &OperationContext,
            ) -> Result<BuiltArtifact, CompileError> {
                operation.check().map_err(CompileError::from)?;
                let package = Self::build(snapshot)?;
                operation.check().map_err(CompileError::from)?;
                Ok(package)
            }
        }
    }

    fn map_workspace_load_error(error: WorkspaceLoadError) -> CompileError {
        let code = match error.code {
            WorkspaceLoadErrorCode::Cancelled => CompileErrorCode::Cancelled,
            WorkspaceLoadErrorCode::DeadlineExceeded => CompileErrorCode::DeadlineExceeded,
            _ => CompileErrorCode::PackageSnapshot,
        };
        let message = error.to_string();
        if matches!(
            code,
            CompileErrorCode::Cancelled | CompileErrorCode::DeadlineExceeded
        ) {
            CompileError::Operation { code, message }
        } else {
            CompileError::Package { code, message }
        }
    }
}

/// Reviewed operation-control types shared by compile, verification, and run.
pub mod operation {
    pub use rsscript_operation::{
        CancellationToken, MonotonicDeadline, OperationAbort, OperationContext, OperationId,
    };
}

#[cfg(feature = "execution")]
/// Reviewed Artifact construction and verification entry points.
pub mod artifact {
    pub use super::{
        ARTIFACT_BUNDLE_MAGIC, ARTIFACT_BUNDLE_SCHEMA, AdmissionError, AdmittedArtifact,
        AnalysisEnvelopeV1, AnalysisSchemaV1, ArtifactAdmission, ArtifactAdmissionPolicy,
        ArtifactBundle, ArtifactBundleError, ArtifactVerifier, BuildProvenanceV1, BuiltArtifact,
        InterfaceRequirementV1, PACKAGE_ANALYSIS_SCHEMA, SOURCE_ANALYSIS_SCHEMA, SourceAnalysisV1,
        TrustedInputAdmission, VerifiedArtifact, VerifyError,
    };
    pub use rsscript_bytecode::{
        BYTECODE_CONTAINER_FORMAT_VERSION, BYTECODE_ISA_VERSION, BYTECODE_MAGIC, BYTECODE_SCHEMA,
        BytecodeArtifact, BytecodeHeader, BytecodeVerifier,
    };
}

#[cfg(feature = "execution")]
/// Reviewed Provider registration and host-call contract types.
pub mod provider_api {
    pub use super::ProviderRegistry;
    pub use rsscript_provider_api::{
        AsyncWireInterpreterFn, BlockingBehavior, CancellationBehavior, ExternalSymbol,
        FunctionSignature, HostCallContext, ProviderCallContext, ProviderCallMode,
        ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderFunction,
        ProviderFunctionDescriptor, ProviderLoadError, ResourceHandle, WireInterpreterFn,
        WireProviderFuture, WireValue,
    };
}

#[cfg(feature = "execution")]
/// Reviewed linking and bounded execution entry points.
pub mod runtime {
    pub use super::{ExecutionRequest, LinkError, LinkedArtifact, RunLimits, Runtime, TracePolicy};
}

#[cfg(feature = "execution")]
/// Reviewed machine-readable execution-report types.
pub mod report {
    pub use super::{
        EXECUTION_REPORT_SCHEMA, ExecutionOutcome, ExecutionReport, ExecutionTelemetry,
        ProviderFunctionTelemetry, RuntimeError, TerminationReason,
    };
}

#[cfg(feature = "execution")]
/// Reviewed neutral Artifact analysis and semantic-diff data.
pub mod analysis {
    pub use super::{
        CallEdgeFactV1, ExternalContractFactV1, FunctionParameterFactV1, ResourceLifetimeFactV1,
        ResourceTransferFactV1, SEMANTIC_DIFF_SCHEMA, SemanticDiffV1, TaskGroupFactV1,
    };
}
#[cfg(not(feature = "compatibility"))]
#[allow(unused_imports)]
use rsscript_operation::*;
#[cfg(feature = "compatibility")]
pub use rsscript_operation::{
    CancellationToken, MonotonicDeadline, OperationAbort, OperationContext, OperationId,
};
#[cfg(feature = "execution")]
#[cfg(feature = "compatibility")]
pub use rsscript_provider_api as provider;
#[cfg(all(feature = "execution", not(feature = "compatibility")))]
use rsscript_provider_api as provider;

#[derive(Default)]
pub struct Compiler;

impl Compiler {
    pub fn check(&self, file: &str, source: &str) -> Vec<Diagnostic> {
        self.check_snapshot(&FrontendInputSnapshot::single(file, source))
    }

    /// Check one immutable source/interface snapshot without reading a path or
    /// selecting a Provider.
    pub fn check_snapshot(&self, snapshot: &FrontendInputSnapshot) -> Vec<Diagnostic> {
        let sources = snapshot_pairs(snapshot.sources());
        let interfaces = snapshot_pairs(snapshot.interfaces());
        analyze_sources_with_interfaces(&sources, &interfaces)
    }

    pub fn check_with_operation(
        &self,
        file: &str,
        source: &str,
        operation: &OperationContext,
    ) -> Result<Vec<Diagnostic>, CompileError> {
        self.check_snapshot_with_operation(&FrontendInputSnapshot::single(file, source), operation)
    }

    /// Check one immutable source/interface snapshot while honoring the shared
    /// cancellation and deadline boundary. This is the operation-aware
    /// counterpart of [`Self::check_snapshot`].
    pub fn check_snapshot_with_operation(
        &self,
        snapshot: &FrontendInputSnapshot,
        operation: &OperationContext,
    ) -> Result<Vec<Diagnostic>, CompileError> {
        operation.check().map_err(CompileError::from)?;
        let sources = snapshot_pairs(snapshot.sources());
        let interfaces = snapshot_pairs(snapshot.interfaces());
        let diagnostics =
            analyze_sources_with_interfaces_result_with_operation(&sources, &interfaces, operation)
                .into_diagnostics();
        operation.check().map_err(CompileError::from)?;
        Ok(diagnostics)
    }

    #[cfg(feature = "execution")]
    pub fn compile(&self, file: &str, source: &str) -> Result<BuiltArtifact, CompileError> {
        self.compile_snapshot(&FrontendInputSnapshot::single(file, source))
    }

    /// Compile one immutable, provider-neutral frontend snapshot.
    ///
    /// This is the preferred embedding boundary. Filesystem/package loaders
    /// capture their inputs before constructing the snapshot; this method never
    /// reads a path or ambient process state.
    #[cfg(feature = "execution")]
    pub fn compile_snapshot(
        &self,
        snapshot: &FrontendInputSnapshot,
    ) -> Result<BuiltArtifact, CompileError> {
        let sources = snapshot_pairs(snapshot.sources());
        let interfaces = snapshot_pairs(snapshot.interfaces());
        let validated = validate_sources_with_interfaces(&sources, &interfaces)
            .map_err(CompileError::Diagnostics)?;
        let snapshot_digest = in_memory_snapshot_digest(&sources, &interfaces);
        let artifact = compile_validated_to_bytecode(&validated, &snapshot_digest)
            .map_err(bytecode_compile_error)?;
        BuiltArtifact::from_bytecode(artifact, source_set_analysis(&sources, &snapshot_digest))
    }

    #[cfg(feature = "execution")]
    pub fn compile_with_operation(
        &self,
        file: &str,
        source: &str,
        operation: &OperationContext,
    ) -> Result<BuiltArtifact, CompileError> {
        self.compile_snapshot_with_operation(
            &FrontendInputSnapshot::single(file, source),
            operation,
        )
    }

    #[cfg(feature = "execution")]
    pub fn compile_snapshot_with_operation(
        &self,
        snapshot: &FrontendInputSnapshot,
        operation: &OperationContext,
    ) -> Result<BuiltArtifact, CompileError> {
        let sources = snapshot_pairs(snapshot.sources());
        let interfaces = snapshot_pairs(snapshot.interfaces());
        let validated =
            validate_sources_with_interfaces_with_operation(&sources, &interfaces, operation)
                .map_err(|diagnostics| match operation.check() {
                    Ok(()) => CompileError::Diagnostics(diagnostics),
                    Err(abort) => CompileError::from(abort),
                })?;
        operation.check().map_err(CompileError::from)?;
        let snapshot_digest = in_memory_snapshot_digest(&sources, &interfaces);
        let artifact = compile_validated_to_bytecode(&validated, &snapshot_digest)
            .map_err(bytecode_compile_error)?;
        operation.check().map_err(CompileError::from)?;
        let built = BuiltArtifact::from_bytecode(
            artifact,
            source_set_analysis(&sources, &snapshot_digest),
        )?;
        operation.check().map_err(CompileError::from)?;
        Ok(built)
    }

    /// Compile a source snapshot against explicit host interfaces. The
    /// interfaces contribute semantic signatures only; provider selection is
    /// intentionally deferred until execution.
    #[cfg(feature = "execution")]
    pub fn compile_with_interfaces(
        &self,
        sources: &[(&str, &str)],
        interfaces: &[(&str, &str)],
    ) -> Result<BuiltArtifact, CompileError> {
        self.compile_snapshot(&FrontendInputSnapshot::from_sources(
            sources.iter().copied(),
            interfaces.iter().copied(),
        ))
    }

    #[cfg(feature = "execution")]
    pub fn compile_with_interfaces_and_operation(
        &self,
        sources: &[(&str, &str)],
        interfaces: &[(&str, &str)],
        operation: &OperationContext,
    ) -> Result<BuiltArtifact, CompileError> {
        self.compile_snapshot_with_operation(
            &FrontendInputSnapshot::from_sources(
                sources.iter().copied(),
                interfaces.iter().copied(),
            ),
            operation,
        )
    }
}

#[cfg(feature = "execution")]
fn bytecode_compile_error(error: impl fmt::Display) -> CompileError {
    CompileError::Bytecode {
        code: CompileErrorCode::Bytecode,
        message: error.to_string(),
    }
}

#[cfg(feature = "execution")]
#[derive(Debug)]
pub struct BuiltArtifact {
    bundle: ArtifactBundle,
}

#[cfg(feature = "execution")]
impl BuiltArtifact {
    fn from_bytecode(
        artifact: BytecodeArtifact,
        analysis: AnalysisEnvelopeV1,
    ) -> Result<Self, CompileError> {
        let bytes = artifact
            .to_bytes()
            .map_err(|error| CompileError::from(EvalError::Runtime(error.to_string())))?;
        let bundle = ArtifactBundle::new(bytes, analysis).map_err(CompileError::from)?;
        Ok(Self { bundle })
    }

    pub fn bundle(&self) -> &ArtifactBundle {
        &self.bundle
    }

    pub fn into_bundle(self) -> ArtifactBundle {
        self.bundle
    }

    pub fn bundle_bytes(&self) -> Result<Vec<u8>, CompileError> {
        self.bundle.to_bytes().map_err(CompileError::from)
    }

    pub fn artifact_bytes(&self) -> &[u8] {
        self.bundle.artifact_bytes()
    }

    pub fn analysis(&self) -> &serde_json::Value {
        self.bundle.analysis()
    }

    /// Typed source evidence for direct source/interface builds. Package
    /// compatibility builds may instead carry their distinct package-analysis
    /// schema and return `None` here.
    pub fn source_analysis(&self) -> Option<&SourceAnalysisV1> {
        self.bundle.source_analysis()
    }

    /// Typed package evidence for immutable package compatibility builds.
    /// Direct source/interface builds instead carry `source_analysis.v1`.
    #[cfg(feature = "compatibility")]
    pub fn package_analysis(&self) -> Option<&PackageAnalysis> {
        self.bundle.package_analysis()
    }

    pub fn snapshot_digest(&self) -> &str {
        &self.bundle.provenance().snapshot_digest
    }

    pub fn module_digest(&self) -> &str {
        &self.bundle.provenance().module_digest
    }

    pub fn external_imports(&self) -> &[InterfaceRequirementV1] {
        self.bundle.required_interfaces()
    }
}

#[cfg(feature = "execution")]
fn source_set_analysis(sources: &[(&str, &str)], snapshot_digest: &str) -> AnalysisEnvelopeV1 {
    AnalysisEnvelopeV1::source(SourceAnalysisV1::new(
        rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION,
        snapshot_digest,
        sources.iter().map(|(path, _)| *path),
    ))
}

fn snapshot_pairs(snapshot: &SourceSnapshot) -> Vec<(&str, &str)> {
    let mut pairs = snapshot
        .files()
        .iter()
        .map(|file| (file.path(), file.text()))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs
}

#[cfg(feature = "project")]
fn frontend_snapshot_digest(snapshot: &FrontendInputSnapshot) -> String {
    let sources = snapshot_pairs(snapshot.sources());
    let interfaces = snapshot_pairs(snapshot.interfaces());
    in_memory_snapshot_digest(&sources, &interfaces)
}

#[cfg(feature = "execution")]
fn in_memory_snapshot_digest(sources: &[(&str, &str)], interfaces: &[(&str, &str)]) -> String {
    // Direct SDK compilation has no filesystem workspace, but it still needs
    // the same immutable identity guarantee as package compilation. Domain
    // separation and role/path/byte lengths prevent ambiguous concatenations.
    let mut entries = sources
        .iter()
        .map(|(path, text)| ("source", *path, *text))
        .chain(
            interfaces
                .iter()
                .map(|(path, text)| ("interface", *path, *text)),
        )
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(b"rsscript.in_memory_snapshot.v1\0");
    for (role, path, text) in entries {
        for value in [role.as_bytes(), path.as_bytes(), text.as_bytes()] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value);
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(feature = "execution")]
#[derive(Default)]
pub struct ArtifactVerifier;

#[cfg(feature = "execution")]
impl ArtifactVerifier {
    pub fn verify(&self, built: BuiltArtifact) -> Result<VerifiedArtifact, VerifyError> {
        self.verify_bundle(built.into_bundle())
    }

    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<VerifiedArtifact, VerifyError> {
        let bundle = ArtifactBundle::from_bytes(bytes).map_err(VerifyError::Bundle)?;
        self.verify_bundle(bundle)
    }

    pub fn verify_bundle(&self, bundle: ArtifactBundle) -> Result<VerifiedArtifact, VerifyError> {
        let verified_bytecode = BytecodeVerifier::default()
            .verify(bundle.artifact_bytes())
            .map_err(|error| VerifyError::Bytecode(EvalError::Runtime(error.to_string())))?;
        verified_artifact(bundle, verified_bytecode)
    }

    pub fn verify_bytes_with_operation(
        &self,
        bytes: &[u8],
        operation: &OperationContext,
    ) -> Result<VerifiedArtifact, VerifyError> {
        operation.check().map_err(VerifyError::Operation)?;
        let bundle = ArtifactBundle::from_bytes(bytes).map_err(VerifyError::Bundle)?;
        let verified_bytecode = BytecodeVerifier::default()
            .verify_with_context(
                bundle.artifact_bytes(),
                VerificationContext {
                    cancellation: operation.cancellation.as_ref(),
                    deadline: operation.deadline,
                },
            )
            .map_err(|error| VerifyError::Bytecode(EvalError::Runtime(error.to_string())))?;
        let artifact = verified_artifact(bundle, verified_bytecode)?;
        operation.check().map_err(VerifyError::Operation)?;
        Ok(artifact)
    }
}

/// The operation-aware and ordinary verifier entry points must produce the
/// exact same phase object. Keeping this conversion in one place prevents a
/// cancellation/deadline convenience API from accidentally skipping a bundle
/// provenance invariant.
#[cfg(feature = "execution")]
fn verified_artifact(
    bundle: ArtifactBundle,
    verified_bytecode: VerifiedBytecode,
) -> Result<VerifiedArtifact, VerifyError> {
    let executable = RegVmExecutable::from_verified_bytecode(verified_bytecode)
        .map_err(VerifyError::Bytecode)?;
    if executable.bytecode_artifact().header.executable_hash != bundle.provenance().module_digest {
        return Err(VerifyError::DigestMismatch);
    }
    Ok(VerifiedArtifact { bundle, executable })
}

#[cfg(feature = "execution")]
#[derive(Debug)]
pub struct VerifiedArtifact {
    bundle: ArtifactBundle,
    executable: RegVmExecutable,
}

#[cfg(feature = "execution")]
impl VerifiedArtifact {
    pub fn bundle(&self) -> &ArtifactBundle {
        &self.bundle
    }

    pub fn module_digest(&self) -> &str {
        &self.bundle.provenance().module_digest
    }

    pub fn external_imports(&self) -> &[ExternalImport] {
        &self.executable.bytecode_artifact().imports
    }

    /// Verified bytecode metadata for inspection tools. Execution remains
    /// available only through the linked runtime stage.
    pub fn bytecode_artifact(&self) -> &BytecodeArtifact {
        self.executable.bytecode_artifact()
    }

    /// Apply the host-owned origin/admission decision before provider linking.
    ///
    /// Verification proves artifact structure and integrity. Admission is a
    /// separate host decision, for example a detached-signature, provenance,
    /// or runner-profile check; it does not create a language policy system.
    pub fn admit<P: ArtifactAdmissionPolicy>(
        self,
        policy: &P,
    ) -> Result<AdmittedArtifact, AdmissionError> {
        let admission = policy.admit(&self)?;
        Ok(AdmittedArtifact {
            artifact: self,
            admission,
        })
    }

    /// Explicitly mark bytes as trusted by this embedding host.
    ///
    /// Hosts handling external inputs should implement
    /// [`ArtifactAdmissionPolicy`] instead; an isolated runner may model its
    /// fixed profile as that policy.
    pub fn admit_trusted_input(self) -> AdmittedArtifact {
        AdmittedArtifact {
            artifact: self,
            admission: ArtifactAdmission::trusted_input(),
        }
    }
}

/// Evidence returned by one host-owned artifact admission decision.
#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactAdmission {
    policy_id: String,
    evidence_digest: Option<String>,
}

#[cfg(feature = "execution")]
impl ArtifactAdmission {
    /// Construct non-secret admission evidence for an accepted artifact.
    pub fn new(
        policy_id: impl Into<String>,
        evidence_digest: Option<impl Into<String>>,
    ) -> Result<Self, AdmissionError> {
        let policy_id = policy_id.into();
        if policy_id.trim().is_empty() {
            return Err(AdmissionError::InvalidPolicyId);
        }
        Ok(Self {
            policy_id,
            evidence_digest: evidence_digest.map(Into::into),
        })
    }

    fn trusted_input() -> Self {
        Self {
            policy_id: "trusted_input.v1".to_string(),
            evidence_digest: None,
        }
    }

    pub fn policy_id(&self) -> &str {
        &self.policy_id
    }

    pub fn evidence_digest(&self) -> Option<&str> {
        self.evidence_digest.as_deref()
    }
}

/// Host extension point for artifact origin/provenance verification.
#[cfg(feature = "execution")]
pub trait ArtifactAdmissionPolicy {
    fn admit(&self, artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError>;
}

/// Explicit policy for a host that already trusts its artifact input channel.
#[cfg(feature = "execution")]
#[derive(Debug, Clone, Copy, Default)]
pub struct TrustedInputAdmission;

#[cfg(feature = "execution")]
impl ArtifactAdmissionPolicy for TrustedInputAdmission {
    fn admit(&self, _artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError> {
        Ok(ArtifactAdmission::trusted_input())
    }
}

/// Host-side failure while admitting a structurally verified Artifact.
#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    InvalidPolicyId,
    Rejected { message: String },
}

#[cfg(feature = "execution")]
impl AdmissionError {
    pub fn rejected(message: impl Into<String>) -> Self {
        Self::Rejected {
            message: message.into(),
        }
    }
}

#[cfg(feature = "execution")]
impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicyId => formatter.write_str("artifact admission policy ID is empty"),
            Self::Rejected { message } => {
                write!(formatter, "artifact admission rejected: {message}")
            }
        }
    }
}

#[cfg(feature = "execution")]
impl Error for AdmissionError {}

/// Structurally verified Artifact accepted by one explicit host policy.
#[cfg(feature = "execution")]
#[derive(Debug)]
pub struct AdmittedArtifact {
    artifact: VerifiedArtifact,
    admission: ArtifactAdmission,
}

#[cfg(feature = "execution")]
impl AdmittedArtifact {
    pub fn bundle(&self) -> &ArtifactBundle {
        self.artifact.bundle()
    }

    pub fn module_digest(&self) -> &str {
        self.artifact.module_digest()
    }

    pub fn external_imports(&self) -> &[ExternalImport] {
        self.artifact.external_imports()
    }

    pub fn admission(&self) -> &ArtifactAdmission {
        &self.admission
    }
}

#[cfg(feature = "execution")]
#[derive(Default)]
pub struct ProviderRegistry {
    inner: ExternalFunctionRegistry,
}

#[cfg(feature = "execution")]
impl ProviderRegistry {
    /// Attach host-defined, instance-local context to every resolved call.
    /// Providers decide how to interpret its labels; the language does not.
    pub fn set_host_call_context(&mut self, context: provider::HostCallContext) {
        self.inner.set_host_call_context(context);
    }

    pub fn register<T: Into<provider::ProviderCallable>>(
        &mut self,
        descriptor: &ProviderDescriptor,
        functions: BTreeMap<provider::ExternalSymbol, ProviderFunction<T>>,
    ) -> Result<(), ProviderLoadError> {
        self.inner.register_provider(descriptor, functions)
    }
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone)]
pub struct RunLimits {
    max_depth: usize,
    step_budget: Option<u64>,
    allocation_budget: Option<usize>,
    live_memory_limit: Option<usize>,
    cancellation: Option<CancellationToken>,
    deadline: Option<MonotonicDeadline>,
    output_budget: Option<usize>,
    intrinsic_call_budget: Option<u64>,
    provider_call_budget: Option<u64>,
    resource_limit: Option<usize>,
    allow_blocking_provider_calls: bool,
}

#[cfg(feature = "execution")]
impl RunLimits {
    /// Return the bounded public execution defaults.
    pub fn bounded() -> Self {
        Self::default()
    }

    /// Disable budgets for a host-controlled, trusted workload.
    ///
    /// This does not create an isolation boundary. The embedding host remains
    /// responsible for process isolation and provider authority.
    pub fn unbounded_for_trusted_host() -> Self {
        VmLimits::unbounded_for_trusted_host().into()
    }

    pub fn with_max_depth(mut self, maximum: usize) -> Self {
        self.max_depth = maximum;
        self
    }

    pub fn with_step_budget(mut self, budget: u64) -> Self {
        self.step_budget = Some(budget);
        self
    }

    pub fn with_allocation_budget(mut self, budget: usize) -> Self {
        self.allocation_budget = Some(budget);
        self
    }

    pub fn with_live_memory_limit(mut self, limit: usize) -> Self {
        self.live_memory_limit = Some(limit);
        self
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_deadline(mut self, deadline: MonotonicDeadline) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub fn with_output_budget(mut self, budget: usize) -> Self {
        self.output_budget = Some(budget);
        self
    }

    pub fn with_intrinsic_call_budget(mut self, budget: u64) -> Self {
        self.intrinsic_call_budget = Some(budget);
        self
    }

    pub fn with_provider_call_budget(mut self, budget: u64) -> Self {
        self.provider_call_budget = Some(budget);
        self
    }

    pub fn with_resource_limit(mut self, limit: usize) -> Self {
        self.resource_limit = Some(limit);
        self
    }

    pub fn allow_blocking_provider_calls(mut self, allow: bool) -> Self {
        self.allow_blocking_provider_calls = allow;
        self
    }

    pub fn blocking_provider_calls_allowed(&self) -> bool {
        self.allow_blocking_provider_calls
    }
}

#[cfg(feature = "execution")]
impl Default for RunLimits {
    fn default() -> Self {
        VmLimits::default().into()
    }
}

#[cfg(feature = "execution")]
impl From<VmLimits> for RunLimits {
    fn from(limits: VmLimits) -> Self {
        Self {
            max_depth: limits.max_depth,
            step_budget: limits.step_budget,
            allocation_budget: limits.allocation_budget,
            live_memory_limit: limits.live_memory_limit,
            cancellation: limits.cancel,
            deadline: limits.deadline,
            output_budget: limits.stdout_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
            resource_limit: limits.resource_limit,
            allow_blocking_provider_calls: limits.allow_blocking_provider_calls,
        }
    }
}

#[cfg(feature = "execution")]
impl From<RunLimits> for VmLimits {
    fn from(limits: RunLimits) -> Self {
        Self {
            max_depth: limits.max_depth,
            step_budget: limits.step_budget,
            allocation_budget: limits.allocation_budget,
            live_memory_limit: limits.live_memory_limit,
            cancel: limits.cancellation,
            deadline: limits.deadline,
            stdout_budget: limits.output_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
            resource_limit: limits.resource_limit,
            allow_blocking_provider_calls: limits.allow_blocking_provider_calls,
        }
    }
}

#[cfg(feature = "execution")]
pub struct Runtime {
    providers: ProviderRegistry,
}

#[cfg(feature = "execution")]
impl Runtime {
    pub fn new(providers: ProviderRegistry) -> Self {
        Self { providers }
    }

    /// Resolve every external import before any instruction can run.
    /// `LinkedArtifact` has no public constructor, so the
    /// stable SDK cannot bypass Provider preflight.
    pub fn link<'artifact>(
        &self,
        artifact: &'artifact AdmittedArtifact,
    ) -> Result<LinkedArtifact<'artifact>, LinkError> {
        for import in artifact.external_imports() {
            if let Err(error) = self.providers.inner.resolve(import) {
                return Err(LinkError::Provider(error));
            }
        }
        Ok(LinkedArtifact {
            artifact,
            bindings: self.providers.inner.bindings().collect(),
        })
    }
}

#[cfg(feature = "execution")]
pub struct LinkedArtifact<'artifact> {
    artifact: &'artifact AdmittedArtifact,
    bindings: Vec<(String, ExternalFunction)>,
}

#[cfg(feature = "execution")]
impl LinkedArtifact<'_> {
    pub fn module_digest(&self) -> &str {
        self.artifact.module_digest()
    }

    /// Execute and always return an audit report, including partial evidence
    /// for cancellation, budget exhaustion, and Provider failures.
    pub fn execute(&self, request: ExecutionRequest) -> ExecutionReport {
        let started = Instant::now();
        let limits: VmLimits = request.limits.into();
        let output = match self
            .artifact
            .artifact
            .executable
            .execute_main_with_args_and_external_bindings_and_limits(
                request.args,
                self.bindings.iter().cloned(),
                limits.clone(),
            ) {
            Ok(output) => output,
            Err(error) => {
                let diagnostics = match &error {
                    EvalError::Diagnostics(diagnostics) => diagnostics.clone(),
                    _ => Vec::new(),
                };
                return ExecutionReport::failed(
                    self.artifact.module_digest(),
                    RuntimeError::from_execution(error),
                    diagnostics,
                    started.elapsed(),
                    limits.cancel.as_ref(),
                );
            }
        };
        let diagnostics = match &output.failure {
            Some(EvalError::Diagnostics(diagnostics)) => diagnostics.clone(),
            _ => Vec::new(),
        };
        let outcome = match output.failure.map(RuntimeError::from_execution) {
            Some(failure) => ExecutionOutcome::Failed(failure),
            None => ExecutionOutcome::Completed {
                value: output.value.unwrap_or_default(),
                display_value: output.display_value.unwrap_or_default(),
            },
        };
        let termination_reason = outcome.termination_reason();
        let telemetry = ExecutionTelemetry::from_traces(
            started.elapsed(),
            termination_reason,
            limits.cancel.as_ref(),
            &output.provider_call_traces,
        );
        ExecutionReport {
            schema: EXECUTION_REPORT_SCHEMA,
            artifact_digest: self.artifact.module_digest().to_string(),
            usage: output.usage,
            telemetry,
            outcome,
            // The v1 JSON report still carries the legacy projection for
            // backwards-compatible machine consumers. It is deliberately not
            // part of the reviewed Rust API: new embedders receive only the
            // stable textual result until the typed report outcome is ready.
            #[cfg(not(feature = "compatibility"))]
            legacy_native_value: output.native_value.map(|value| {
                serde_json::to_value(value)
                    .expect("NativeValue must remain serializable for the v1 report contract")
            }),
            #[cfg(feature = "compatibility")]
            native_value: output.native_value,
            stdout: output.stdout,
            stderr: output.stderr,
            provider_call_traces: match request.trace_policy {
                TracePolicy::None => Vec::new(),
                TracePolicy::MetadataOnly | TracePolicy::RedactedDebug => {
                    output.provider_call_traces
                }
            },
            diagnostics,
        }
    }
}

#[cfg(feature = "execution")]
impl Default for Runtime {
    fn default() -> Self {
        Self::new(ProviderRegistry::default())
    }
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TracePolicy {
    #[default]
    None,
    MetadataOnly,
    RedactedDebug,
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    args: Vec<String>,
    limits: RunLimits,
    trace_policy: TracePolicy,
}

#[cfg(feature = "execution")]
impl ExecutionRequest {
    pub fn new(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            limits: RunLimits::bounded(),
            trace_policy: TracePolicy::None,
        }
    }

    pub fn limits(mut self, limits: RunLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn trace(mut self, policy: TracePolicy) -> Self {
        self.trace_policy = policy;
        self
    }
}

#[cfg(feature = "execution")]
impl Default for ExecutionRequest {
    fn default() -> Self {
        Self::new(std::iter::empty::<String>())
    }
}

#[cfg(feature = "execution")]
pub const EXECUTION_REPORT_SCHEMA: &str = "rsscript.execution_report.v1";

#[cfg(feature = "execution")]
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionTelemetry {
    pub execution_duration_ns: u64,
    pub cancellation_latency_ns: Option<u64>,
    pub provider_functions: Vec<ProviderFunctionTelemetry>,
}

#[cfg(feature = "execution")]
impl ExecutionTelemetry {
    fn from_traces(
        elapsed: Duration,
        termination_reason: TerminationReason,
        cancellation: Option<&CancellationToken>,
        traces: &[provider::ProviderCallTrace],
    ) -> Self {
        let mut summaries = BTreeMap::<(String, String, String), ProviderFunctionTelemetry>::new();
        for trace in traces {
            let key = (
                trace.provider_id.clone(),
                trace.provider_version.clone(),
                trace.symbol.clone(),
            );
            let summary = summaries
                .entry(key)
                .or_insert_with(|| ProviderFunctionTelemetry {
                    provider_id: trace.provider_id.clone(),
                    provider_version: trace.provider_version.clone(),
                    symbol: trace.symbol.clone(),
                    ..ProviderFunctionTelemetry::default()
                });
            summary.calls = summary.calls.saturating_add(1);
            summary.failures = summary
                .failures
                .saturating_add(u64::from(trace.result.is_err()));
            summary.request_bytes = summary.request_bytes.saturating_add(trace.request_bytes);
            summary.response_bytes = summary.response_bytes.saturating_add(trace.response_bytes);
            let elapsed_ns = duration_ns(trace.elapsed);
            summary.total_duration_ns = summary.total_duration_ns.saturating_add(elapsed_ns);
            summary.max_duration_ns = summary.max_duration_ns.max(elapsed_ns);
        }
        let cancellation_latency_ns = (termination_reason == TerminationReason::Cancelled)
            .then(|| cancellation.and_then(CancellationToken::cancelled_at))
            .flatten()
            .map(|cancelled_at| duration_ns(cancelled_at.elapsed()));
        Self {
            execution_duration_ns: duration_ns(elapsed),
            cancellation_latency_ns,
            provider_functions: summaries.into_values().collect(),
        }
    }
}

#[cfg(feature = "execution")]
fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProviderFunctionTelemetry {
    pub provider_id: String,
    pub provider_version: String,
    pub symbol: String,
    pub calls: u64,
    pub failures: u64,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub total_duration_ns: u64,
    pub max_duration_ns: u64,
}

#[cfg(feature = "execution")]
/// Mutually exclusive script-level completion states.
///
/// An execution report always has exactly one of these outcomes. Host protocol,
/// linking, and verification failures stay outside execution and therefore do
/// not manufacture a partial report.
#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionOutcome {
    Completed {
        value: String,
        display_value: String,
    },
    Failed(RuntimeError),
}

#[cfg(feature = "execution")]
impl ExecutionOutcome {
    pub const fn termination_reason(&self) -> TerminationReason {
        match self {
            Self::Completed { .. } => TerminationReason::Completed,
            Self::Failed(error) => error.reason,
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Completed { value, .. } => Some(value),
            Self::Failed(_) => None,
        }
    }

    pub fn display_value(&self) -> Option<&str> {
        match self {
            Self::Completed { display_value, .. } => Some(display_value),
            Self::Failed(_) => None,
        }
    }

    pub const fn failure(&self) -> Option<&RuntimeError> {
        match self {
            Self::Completed { .. } => None,
            Self::Failed(error) => Some(error),
        }
    }
}

/// Structured execution evidence for one linked Artifact run.
///
/// [`Self::outcome`] is the only terminal program state. The JSON serializer
/// intentionally retains the checked-in v1 wire projection while Rust callers
/// use the phase-safe outcome accessors below.
#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionReport {
    pub schema: &'static str,
    pub artifact_digest: String,
    outcome: ExecutionOutcome,
    pub usage: ExecutionUsage,
    pub telemetry: ExecutionTelemetry,
    /// Legacy v1 JSON projection retained only for consumers of the checked-in
    /// report schema. New Rust embedders cannot observe or construct the
    /// dynamic representation through the reviewed SDK.
    #[cfg(not(feature = "compatibility"))]
    legacy_native_value: Option<serde_json::Value>,
    /// Legacy dynamic result projection for compatibility callers only.
    #[cfg(feature = "compatibility")]
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
    pub provider_call_traces: Vec<provider::ProviderCallTrace>,
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(feature = "execution")]
impl ExecutionReport {
    pub const fn outcome(&self) -> &ExecutionOutcome {
        &self.outcome
    }

    pub const fn termination_reason(&self) -> TerminationReason {
        self.outcome.termination_reason()
    }

    pub fn value(&self) -> Option<&str> {
        self.outcome.value()
    }

    pub fn display_value(&self) -> Option<&str> {
        self.outcome.display_value()
    }

    pub const fn failure(&self) -> Option<&RuntimeError> {
        self.outcome.failure()
    }

    fn failed(
        artifact_digest: impl Into<String>,
        failure: RuntimeError,
        diagnostics: Vec<Diagnostic>,
        elapsed: Duration,
        cancellation: Option<&CancellationToken>,
    ) -> Self {
        let outcome = ExecutionOutcome::Failed(failure);
        let termination_reason = outcome.termination_reason();
        Self {
            schema: EXECUTION_REPORT_SCHEMA,
            artifact_digest: artifact_digest.into(),
            outcome,
            usage: ExecutionUsage::default(),
            telemetry: ExecutionTelemetry::from_traces(
                elapsed,
                termination_reason,
                cancellation,
                &[],
            ),
            #[cfg(not(feature = "compatibility"))]
            legacy_native_value: None,
            #[cfg(feature = "compatibility")]
            native_value: None,
            stdout: String::new(),
            stderr: String::new(),
            provider_call_traces: Vec::new(),
            diagnostics,
        }
    }
}

/// Preserve the versioned JSON report contract while the Rust SDK moves from
/// independent optional terminal fields to [`ExecutionOutcome`].
#[cfg(feature = "execution")]
impl serde::Serialize for ExecutionReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ExecutionReport", 13)?;
        state.serialize_field("schema", self.schema)?;
        state.serialize_field("artifact_digest", &self.artifact_digest)?;
        state.serialize_field("termination_reason", &self.termination_reason())?;
        state.serialize_field("usage", &self.usage)?;
        state.serialize_field("telemetry", &self.telemetry)?;
        state.serialize_field("value", self.value().unwrap_or_default())?;
        state.serialize_field("display_value", self.display_value().unwrap_or_default())?;
        #[cfg(not(feature = "compatibility"))]
        state.serialize_field("native_value", &self.legacy_native_value)?;
        #[cfg(feature = "compatibility")]
        state.serialize_field("native_value", &self.native_value)?;
        state.serialize_field("stdout", &self.stdout)?;
        state.serialize_field("stderr", &self.stderr)?;
        state.serialize_field("provider_call_traces", &self.provider_call_traces)?;
        state.serialize_field("diagnostics", &self.diagnostics)?;
        state.serialize_field("failure", &self.failure())?;
        state.end()
    }
}

#[derive(Debug)]
pub enum CompileError {
    Diagnostics(Vec<Diagnostic>),
    Package {
        code: CompileErrorCode,
        message: String,
    },
    Bytecode {
        code: CompileErrorCode,
        message: String,
    },
    Operation {
        code: CompileErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileErrorCode {
    Diagnostics,
    PackageSnapshot,
    PackageAnalysis,
    Bytecode,
    ArtifactBundle,
    Cancelled,
    DeadlineExceeded,
}

impl CompileError {
    pub fn code(&self) -> CompileErrorCode {
        match self {
            Self::Diagnostics(_) => CompileErrorCode::Diagnostics,
            Self::Package { code, .. }
            | Self::Bytecode { code, .. }
            | Self::Operation { code, .. } => *code,
        }
    }
}

impl CompileErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Diagnostics => "diagnostics",
            Self::PackageSnapshot => "package_snapshot",
            Self::PackageAnalysis => "package_analysis",
            Self::Bytecode => "bytecode",
            Self::ArtifactBundle => "artifact_bundle",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
        }
    }
}

#[cfg(feature = "execution")]
impl From<ArtifactBundleError> for CompileError {
    fn from(error: ArtifactBundleError) -> Self {
        Self::Bytecode {
            code: CompileErrorCode::ArtifactBundle,
            message: error.to_string(),
        }
    }
}

impl From<OperationAbort> for CompileError {
    fn from(abort: OperationAbort) -> Self {
        match abort {
            OperationAbort::Cancelled => Self::Operation {
                code: CompileErrorCode::Cancelled,
                message: "compiler operation cancelled".to_string(),
            },
            OperationAbort::DeadlineExceeded => Self::Operation {
                code: CompileErrorCode::DeadlineExceeded,
                message: "compiler operation deadline exceeded".to_string(),
            },
        }
    }
}

#[cfg(feature = "execution")]
impl From<EvalError> for CompileError {
    fn from(error: EvalError) -> Self {
        match error {
            EvalError::Diagnostics(diagnostics) => Self::Diagnostics(diagnostics),
            EvalError::Runtime(message) => Self::Bytecode {
                code: CompileErrorCode::Bytecode,
                message,
            },
            EvalError::Execution { message, .. } => Self::Bytecode {
                code: CompileErrorCode::Bytecode,
                message,
            },
            EvalError::Provider(error) => Self::Bytecode {
                code: CompileErrorCode::Bytecode,
                message: error.to_string(),
            },
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostics(diagnostics) => {
                write!(
                    formatter,
                    "compilation failed with {} diagnostic(s)",
                    diagnostics.len()
                )
            }
            Self::Package { message, .. } => {
                write!(formatter, "package compilation failed: {message}")
            }
            Self::Bytecode { message, .. } => {
                write!(formatter, "bytecode compilation failed: {message}")
            }
            Self::Operation { message, .. } => formatter.write_str(message),
        }
    }
}

impl Error for CompileError {}

#[cfg(feature = "execution")]
#[derive(Debug)]
pub enum VerifyError {
    Bundle(ArtifactBundleError),
    Bytecode(EvalError),
    Operation(OperationAbort),
    DigestMismatch,
}

#[cfg(feature = "execution")]
impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundle(error) => write!(formatter, "bundle verification failed: {error}"),
            Self::Bytecode(error) => write!(formatter, "bytecode verification failed: {error:?}"),
            Self::Operation(OperationAbort::Cancelled) => {
                formatter.write_str("verification cancelled")
            }
            Self::Operation(OperationAbort::DeadlineExceeded) => {
                formatter.write_str("verification deadline exceeded")
            }
            Self::DigestMismatch => formatter.write_str("verified artifact digest mismatch"),
        }
    }
}

#[cfg(feature = "execution")]
impl Error for VerifyError {}

#[cfg(feature = "execution")]
#[derive(Debug)]
pub enum LinkError {
    Provider(ProviderLoadError),
}

#[cfg(feature = "execution")]
impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => write!(formatter, "provider link failed: {error}"),
        }
    }
}

#[cfg(feature = "execution")]
impl Error for LinkError {}

#[cfg(feature = "execution")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Completed,
    ScriptError,
    Cancelled,
    DeadlineExceeded,
    StepBudgetExceeded,
    AllocationBudgetExceeded,
    LiveMemoryLimitExceeded,
    OutputLimitExceeded,
    ProviderError,
    ProviderBudgetExceeded,
    IntrinsicBudgetExceeded,
    ResourceLimitExceeded,
    VerificationFailure,
    InternalError,
}

#[cfg(feature = "execution")]
impl TerminationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ScriptError => "script_error",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::StepBudgetExceeded => "step_budget_exceeded",
            Self::AllocationBudgetExceeded => "allocation_budget_exceeded",
            Self::LiveMemoryLimitExceeded => "live_memory_limit_exceeded",
            Self::OutputLimitExceeded => "output_limit_exceeded",
            Self::ProviderError => "provider_error",
            Self::ProviderBudgetExceeded => "provider_budget_exceeded",
            Self::IntrinsicBudgetExceeded => "intrinsic_budget_exceeded",
            Self::ResourceLimitExceeded => "resource_limit_exceeded",
            Self::VerificationFailure => "verification_failure",
            Self::InternalError => "internal_error",
        }
    }
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeError {
    pub reason: TerminationReason,
    pub message: String,
}

#[cfg(feature = "execution")]
impl From<EvalError> for RuntimeError {
    fn from(error: EvalError) -> Self {
        Self::from_execution(error)
    }
}

#[cfg(feature = "execution")]
impl RuntimeError {
    /// Convert execution failures into report-safe evidence.
    ///
    /// Provider messages and structured details are host-owned data and can
    /// contain request paths, endpoints, credentials, or response fragments.
    /// The default report keeps a stable machine-readable error code but does
    /// not serialize that Provider-controlled content. Hosts that need richer
    /// diagnostics must keep it in their own redacted trace sink.
    fn from_execution(error: EvalError) -> Self {
        match error {
            EvalError::Diagnostics(diagnostics) => Self {
                reason: TerminationReason::VerificationFailure,
                message: format!(
                    "execution rejected with {} diagnostic(s)",
                    diagnostics.len()
                ),
            },
            EvalError::Runtime(message) => Self {
                reason: TerminationReason::ScriptError,
                message,
            },
            EvalError::Execution { kind, message } => Self {
                reason: match kind {
                    ExecutionFailureKind::Cancelled => TerminationReason::Cancelled,
                    ExecutionFailureKind::DeadlineExceeded => TerminationReason::DeadlineExceeded,
                    ExecutionFailureKind::StepBudgetExceeded => {
                        TerminationReason::StepBudgetExceeded
                    }
                    ExecutionFailureKind::AllocationBudgetExceeded => {
                        TerminationReason::AllocationBudgetExceeded
                    }
                    ExecutionFailureKind::LiveMemoryLimitExceeded => {
                        TerminationReason::LiveMemoryLimitExceeded
                    }
                    ExecutionFailureKind::OutputLimitExceeded => {
                        TerminationReason::OutputLimitExceeded
                    }
                    ExecutionFailureKind::ProviderBudgetExceeded => {
                        TerminationReason::ProviderBudgetExceeded
                    }
                    ExecutionFailureKind::IntrinsicBudgetExceeded => {
                        TerminationReason::IntrinsicBudgetExceeded
                    }
                    ExecutionFailureKind::ResourceLimitExceeded => {
                        TerminationReason::ResourceLimitExceeded
                    }
                },
                message,
            },
            EvalError::Provider(error) => Self {
                reason: match error.code {
                    provider::ProviderErrorCode::Cancelled => TerminationReason::Cancelled,
                    provider::ProviderErrorCode::DeadlineExceeded => {
                        TerminationReason::DeadlineExceeded
                    }
                    _ => TerminationReason::ProviderError,
                },
                message: format!("provider call failed ({})", error.code.as_str()),
            },
        }
    }
}

#[cfg(feature = "execution")]
impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(feature = "execution")]
impl Error for RuntimeError {}

#[cfg(all(test, feature = "execution"))]
mod tests {
    use super::*;
    use provider::{
        BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
        ParameterSignature, ProviderCallMode, ProviderFunctionDescriptor, RUNTIME_ABI_VERSION,
    };
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    fn admitted(built: BuiltArtifact) -> AdmittedArtifact {
        ArtifactVerifier
            .verify(built)
            .expect("verify artifact")
            .admit_trusted_input()
    }

    struct RejectAdmission;

    impl ArtifactAdmissionPolicy for RejectAdmission {
        fn admit(&self, _artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError> {
            Err(AdmissionError::rejected(
                "test admission policy rejected artifact",
            ))
        }
    }

    #[test]
    fn verification_and_host_admission_are_distinct_phases() {
        let built = Compiler
            .compile("admission.rss", "fn main() -> Unit { return Unit }")
            .expect("compile");
        let verified = ArtifactVerifier.verify(built).expect("verify");
        let rejection = verified
            .admit(&RejectAdmission)
            .expect_err("host admission policy rejects artifact");
        assert_eq!(
            rejection.to_string(),
            "artifact admission rejected: test admission policy rejected artifact"
        );

        let built = Compiler
            .compile("admission.rss", "fn main() -> Unit { return Unit }")
            .expect("compile");
        let admitted = ArtifactVerifier
            .verify(built)
            .expect("verify")
            .admit(&TrustedInputAdmission)
            .expect("admit");
        assert_eq!(admitted.admission().policy_id(), "trusted_input.v1");
        assert_eq!(admitted.admission().evidence_digest(), None);
        let report = Runtime::default()
            .link(&admitted)
            .expect("link admitted artifact")
            .execute(ExecutionRequest::default());
        assert_eq!(report.termination_reason(), TerminationReason::Completed);
    }

    #[cfg(all(feature = "project", feature = "compatibility"))]
    #[test]
    fn package_build_uses_one_immutable_snapshot_and_binds_its_digest() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(directory.path().join("src")).expect("source directory");
        std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"snapshot-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("manifest");
        let source_path = directory.path().join("src/main.rss");
        std::fs::write(&source_path, "fn main() -> Int { return 1 }").expect("source");

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        let error = project::legacy::PackageCompatibility::snapshot_with_operation(
            directory.path(),
            &cancelled,
        )
        .expect_err("cancelled snapshot");
        assert_eq!(error.code(), CompileErrorCode::Cancelled);
        let snapshot =
            project::legacy::PackageCompatibility::snapshot(directory.path()).expect("snapshot");
        std::fs::write(&source_path, "fn main() -> Int { return 2 }").expect("mutate checkout");

        let first = project::legacy::PackageCompatibility::build(&snapshot).expect("first build");
        let second = project::legacy::PackageCompatibility::build(&snapshot).expect("repeat build");
        assert_eq!(first.artifact_bytes(), second.artifact_bytes());
        assert_eq!(first.snapshot_digest(), snapshot.digest());
        let analysis = first.analysis();
        assert_eq!(analysis["snapshot_digest"], snapshot.digest());
        assert_eq!(analysis["module_digest"], first.module_digest());
        let artifact = rsscript_bytecode::BytecodeArtifact::from_bytes(first.artifact_bytes())
            .expect("artifact envelope");
        assert_eq!(
            artifact.header.snapshot_digest.as_deref(),
            Some(snapshot.digest())
        );
        assert_eq!(
            analysis["language_version"],
            artifact.header.language_version
        );
        assert_eq!(
            analysis["producer"]["version"],
            artifact.header.compiler_version
        );
        assert_eq!(
            analysis["interface_catalog_digest"],
            artifact.header.interface_catalog_digest
        );
        let first = admitted(first);
        let runtime = Runtime::default();
        let output = runtime
            .link(&first)
            .expect("link captured source")
            .execute(ExecutionRequest::default());
        assert_eq!(output.value, "1");
    }

    #[cfg(feature = "project")]
    #[test]
    fn project_loader_capture_feeds_the_pure_frontend_compiler_boundary() {
        let directory = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(directory.path().join("src")).expect("source directory");
        std::fs::create_dir(directory.path().join("interfaces")).expect("interface directory");
        std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"captured-project\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("manifest");
        std::fs::write(
            directory.path().join("src/main.rss"),
            "fn main() -> Int { return 42 }\n",
        )
        .expect("source");
        std::fs::write(
            directory.path().join("interfaces/host.rssi"),
            "module host\npub fn version() -> Int\n",
        )
        .expect("interface");

        let project = project::ProjectCompiler::new();
        let captured = project
            .capture_frontend_from(directory.path(), std::path::Path::new("."))
            .expect("explicit-base loader capture");
        assert!(captured.content_digest().starts_with("sha256:"));
        assert!(
            captured
                .files()
                .iter()
                .all(|file| !file.logical_path.starts_with('/')),
            "compiler-facing snapshot identity must not contain host-absolute paths"
        );
        assert!(
            captured
                .frontend()
                .sources()
                .files()
                .iter()
                .any(|file| file.path() == "root/src/main.rss")
        );
        assert!(
            captured
                .frontend()
                .interfaces()
                .files()
                .iter()
                .any(|file| file.path() == "root/interfaces/host.rssi")
        );
        let built = project
            .build_captured(&captured)
            .expect("pure compiler accepts the loader-captured input");
        assert!(!built.artifact_bytes().is_empty());
        assert_eq!(built.snapshot_digest(), captured.frontend_digest());
        let convenience = project
            .compile_package(directory.path())
            .expect("package convenience path captures once then uses the pure compiler");
        assert_eq!(convenience.artifact_bytes(), built.artifact_bytes());
        assert_eq!(convenience.snapshot_digest(), captured.frontend_digest());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = project
            .build_captured_with_operation(
                &captured,
                &OperationContext {
                    cancellation: Some(cancellation),
                    ..OperationContext::default()
                },
            )
            .expect_err("cancelled captured build");
        assert_eq!(cancelled.code(), CompileErrorCode::Cancelled);
        let loader_cancel = CancellationToken::new();
        loader_cancel.cancel();
        let cancelled = project
            .compile_package_with_operation(
                directory.path(),
                &OperationContext {
                    cancellation: Some(loader_cancel),
                    ..OperationContext::default()
                },
            )
            .expect_err("cancelled package capture");
        assert_eq!(cancelled.code(), CompileErrorCode::Cancelled);
    }

    #[cfg(feature = "project")]
    #[test]
    fn project_capture_builds_with_dependency_interface_inputs() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path().join("root");
        let dependency = workspace.path().join("dependency");
        for directory in [root.join("src"), dependency.join("interfaces")] {
            std::fs::create_dir_all(directory).expect("package directory");
        }
        std::fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[dependencies]\ndependency = { path = \"../dependency\" }\n",
        )
        .expect("root manifest");
        std::fs::write(
            root.join("src/main.rss"),
            "fn main() -> Int { return Dependency.value() }\n",
        )
        .expect("root source");
        std::fs::write(
            dependency.join("rsspkg.toml"),
            "[package]\nname = \"dependency\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .expect("dependency manifest");
        std::fs::write(
            dependency.join("interfaces/dependency.rssi"),
            "pub fn Dependency.value() -> Int\n",
        )
        .expect("dependency interface");

        let project = project::ProjectCompiler::new();
        let captured = project
            .capture_frontend_from(workspace.path(), std::path::Path::new("root"))
            .expect("capture dependency interfaces");
        assert!(captured.frontend().interfaces().files().iter().any(|file| {
            file.path().starts_with("dependency/")
                && file.path().ends_with("/interfaces/dependency.rssi")
        }));
        let direct = project.build_captured(&captured).expect("pure build");
        let convenience = project.compile_package(&root).expect("convenience build");
        assert_eq!(direct.artifact_bytes(), convenience.artifact_bytes());
        assert_eq!(direct.snapshot_digest(), captured.frontend_digest());
    }

    #[test]
    fn stable_facade_compiles_serializes_loads_and_runs() {
        let compiler = Compiler;
        let package = compiler
            .compile("main.rss", "fn main() -> Unit { return Unit }")
            .expect("compile");
        let bundle_bytes = package.bundle_bytes().expect("bundle");
        let loaded = ArtifactVerifier
            .verify_bytes(&bundle_bytes)
            .expect("load verified")
            .admit_trusted_input();
        let runtime = Runtime::default();
        let report = runtime
            .link(&loaded)
            .expect("link")
            .execute(ExecutionRequest::default());
        assert!(matches!(
            report.outcome(),
            ExecutionOutcome::Completed {
                value,
                display_value,
            } if value == "Unit" && display_value == "Unit"
        ));
        assert_eq!(report.value(), Some("Unit"));
        assert_eq!(report.termination_reason(), TerminationReason::Completed);
        assert_eq!(report.artifact_digest, loaded.module_digest());
        assert!(report.usage.steps_consumed > 0);
        assert_eq!(report.termination_reason().as_str(), "completed");
        let json = serde_json::to_value(&report).expect("serialize execution report");
        assert_eq!(json["schema"], EXECUTION_REPORT_SCHEMA);
        assert_eq!(json["termination_reason"], "completed");
        assert!(json["usage"]["steps_consumed"].as_u64().unwrap() > 0);
        assert_eq!(
            CompileErrorCode::PackageSnapshot.as_str(),
            "package_snapshot"
        );
        assert!(!RunLimits::bounded().blocking_provider_calls_allowed());
        assert!(RunLimits::unbounded_for_trusted_host().blocking_provider_calls_allowed());
    }

    #[test]
    fn execution_usage_reports_structured_task_lifecycle() {
        let source = r#"
async fn work(value: Int) -> Result<Int, String> {
    return Ok(value)
}

fn main() -> Result<Unit, String> {
    task_group {
        async let first = work(value: 1)
        async let second = work(value: 2)
        let first_value = await first?
        let second_value = await second?
        let total = first_value + second_value
    }
    return Ok(Unit)
}
"#;
        let package = admitted(Compiler.compile("tasks.rss", source).expect("compile"));
        let report = Runtime::default()
            .link(&package)
            .expect("link")
            .execute(ExecutionRequest::default());
        assert_eq!(report.termination_reason(), TerminationReason::Completed);
        assert_eq!(report.usage.tasks_created, 3);
        assert_eq!(report.usage.tasks_completed, 3);
        assert_eq!(report.usage.tasks_cancelled, 0);
        assert_eq!(report.usage.tasks_peak_live, 3);
        assert_eq!(report.usage.tasks_live_at_return, 0);
    }

    #[test]
    fn mir_result_try_short_circuits_through_verified_bytecode() {
        let source = r#"
fn fail() -> Result<Int, String> {
    return Err("boom")
}

fn main() -> Result<Int, String> {
    let value = fail()?
    return Ok(value)
}
"#;
        let package = admitted(Compiler.compile("result-try.rss", source).expect("compile"));
        let report = Runtime::default()
            .link(&package)
            .expect("link")
            .execute(ExecutionRequest::default());
        assert_eq!(report.termination_reason(), TerminationReason::Completed);
        assert!(report.value().is_some_and(|value| value.contains("Err")));
        assert!(report.value().is_some_and(|value| value.contains("boom")));
    }

    #[test]
    fn cancelled_execution_reports_request_to_observation_latency() {
        let package = admitted(
            Compiler
                .compile(
                    "cancel.rss",
                    "fn main() -> Unit { while true {} return Unit }",
                )
                .expect("compile"),
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = Runtime::new(ProviderRegistry::default());
        let report = runtime.link(&package).expect("link").execute(
            ExecutionRequest::default()
                .limits(RunLimits::bounded().with_cancellation(cancellation)),
        );
        assert_eq!(report.termination_reason(), TerminationReason::Cancelled);
        assert!(matches!(
            report.outcome(),
            ExecutionOutcome::Failed(error) if error.reason == TerminationReason::Cancelled
        ));
        assert!(report.telemetry.cancellation_latency_ns.is_some());
        assert!(report.telemetry.execution_duration_ns > 0);
    }

    #[test]
    fn compiler_and_loader_observe_shared_operation_control() {
        let compiler = Compiler;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        let error = compiler
            .check_with_operation(
                "cancelled.rss",
                "fn main() -> Unit { return Unit }",
                &cancelled,
            )
            .expect_err("cancelled check");
        assert_eq!(error.code(), CompileErrorCode::Cancelled);
        let error = compiler
            .compile_with_operation(
                "cancelled.rss",
                "fn main() -> Unit { return Unit }",
                &cancelled,
            )
            .expect_err("cancelled compile");
        assert_eq!(error.code(), CompileErrorCode::Cancelled);

        let package = compiler
            .compile("main.rss", "fn main() -> Unit { return Unit }")
            .expect("compile fixture");
        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                std::time::Instant::now() - std::time::Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        let error = ArtifactVerifier
            .verify_bytes_with_operation(&package.bundle_bytes().unwrap(), &expired)
            .expect_err("expired verifier deadline");
        assert!(matches!(
            error,
            VerifyError::Operation(OperationAbort::DeadlineExceeded)
        ));
        assert!(error.to_string().contains("deadline exceeded"));

        let bytes = package.bundle_bytes().expect("bundle bytes");
        let ordinary = ArtifactVerifier
            .verify_bytes(&bytes)
            .expect("ordinary verification");
        let operation_aware = ArtifactVerifier
            .verify_bytes_with_operation(&bytes, &OperationContext::default())
            .expect("operation-aware verification");
        assert_eq!(ordinary.module_digest(), operation_aware.module_digest());
        assert_eq!(
            ordinary.bytecode_artifact().header.executable_hash,
            operation_aware.bytecode_artifact().header.executable_hash
        );
    }

    #[test]
    fn frontend_snapshot_is_the_shared_check_and_compile_input() {
        let input = FrontendInputSnapshot::from_sources(
            [(
                "main.rss",
                "module app\nuse host.*\nfn main() -> Int { return value() }\n",
            )],
            [("host.rssi", "module host\npub fn value() -> Int\n")],
        );
        let compiler = Compiler;
        assert!(compiler.check_snapshot(&input).is_empty());
        let artifact = compiler
            .compile_snapshot(&input)
            .expect("the checked snapshot should compile");
        assert_eq!(
            artifact.analysis()["snapshot_digest"],
            artifact.snapshot_digest()
        );
    }

    #[test]
    fn frontend_snapshot_file_enumeration_does_not_change_artifact_bytes() {
        let first = FrontendInputSnapshot::from_sources(
            [
                ("helper.rss", "fn helper() -> Int { return 41 }\n"),
                ("main.rss", "fn main() -> Int { return helper() + 1 }\n"),
            ],
            std::iter::empty(),
        );
        let second = FrontendInputSnapshot::from_sources(
            [
                ("main.rss", "fn main() -> Int { return helper() + 1 }\n"),
                ("helper.rss", "fn helper() -> Int { return 41 }\n"),
            ],
            std::iter::empty(),
        );
        let compiler = Compiler;
        let first = compiler.compile_snapshot(&first).expect("first snapshot");
        let second = compiler.compile_snapshot(&second).expect("second snapshot");
        assert_eq!(first.snapshot_digest(), second.snapshot_digest());
        assert_eq!(
            first.bundle_bytes().unwrap(),
            second.bundle_bytes().unwrap()
        );
    }

    #[test]
    fn module_interface_keeps_stable_external_symbol_and_preflights_signature() {
        let compiler = Compiler;
        let input = FrontendInputSnapshot::from_sources(
            [(
                "main.rss",
                "module app\nuse host.log.*\nfn main() -> Unit { emit(message: read \"ok\"); return Unit }",
            )],
            [(
                "log.rssi",
                "module host.log\npub fn emit(message: read String) -> Unit\n",
            )],
        );
        let package = compiler
            .compile_snapshot(&input)
            .expect("compile external call");
        assert_eq!(package.external_imports().len(), 1);
        assert_eq!(package.external_imports()[0].symbol, "host.log.emit");

        let incompatible = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "message".into(),
                effect: DataEffect::Take,
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        };
        let symbol = ExternalSymbol::new("host.log.emit").expect("symbol");
        let descriptor = ProviderDescriptor {
            provider_id: "test.log".into(),
            provider_version: "1".into(),
            supported_abi: vec![RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: incompatible.clone(),
                entry: "emit".into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::NotApplicable,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: provider::ResourceCleanupContract::None,
                error_mapping: provider::ProviderErrorMapping::StructuredV1,
            }],
        };
        let called = Arc::new(AtomicBool::new(false));
        let called_by_provider = Arc::clone(&called);
        let mut providers = ProviderRegistry::default();
        providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature: incompatible,
                        callable: NativeInterpreterFn::new(move |_| {
                            called_by_provider.store(true, Ordering::SeqCst);
                            Ok(NativeValue::Unit)
                        }),
                    },
                )]),
            )
            .expect("provider descriptor and implementation should match");

        let package = admitted(package);
        let runtime = Runtime::new(providers);
        let error = match runtime.link(&package) {
            Ok(_) => panic!("import signature must fail before execution"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("ImportSignatureMismatch"));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn provider_calls_have_a_budget_separate_from_intrinsics() {
        let compiler = Compiler;
        let package = compiler
            .compile_with_interfaces(
                &[(
                    "main.rss",
                    "module app\nuse host.log.*\nfn main() -> Unit { emit(message: read \"one\"); emit(message: read \"two\"); return Unit }",
                )],
                &[(
                    "log.rssi",
                    "module host.log\npub fn emit(message: read String) -> Unit\n",
                )],
            )
            .expect("compile external calls");
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "message".into(),
                effect: DataEffect::Read,
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        };
        let symbol = ExternalSymbol::new("host.log.emit").expect("symbol");
        let descriptor = ProviderDescriptor {
            provider_id: "test.log".into(),
            provider_version: "1".into(),
            supported_abi: vec![RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "emit".into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::NotApplicable,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: provider::ResourceCleanupContract::None,
                error_mapping: provider::ProviderErrorMapping::StructuredV1,
            }],
        };
        let mut providers = ProviderRegistry::default();
        providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable: NativeInterpreterFn::new(|_| Ok(NativeValue::Unit)),
                    },
                )]),
            )
            .expect("register provider");
        let limits = RunLimits::default().with_provider_call_budget(1);

        let package = admitted(package);
        let runtime = Runtime::new(providers);
        let report = runtime.link(&package).expect("link providers").execute(
            ExecutionRequest::default()
                .limits(limits)
                .trace(TracePolicy::MetadataOnly),
        );
        assert_eq!(
            report.termination_reason(),
            TerminationReason::ProviderBudgetExceeded
        );
        assert_eq!(report.usage.provider_calls, 2);
        assert_eq!(report.provider_call_traces.len(), 1);
        assert!(
            report
                .failure()
                .is_some_and(|error| error.message.contains("provider call budget exceeded"))
        );

        let failure_symbol = descriptor.functions[0].symbol.clone();
        let failure_signature = descriptor.functions[0].signature.clone();
        let mut failing_providers = ProviderRegistry::default();
        failing_providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    failure_symbol,
                    ProviderFunction {
                        signature: failure_signature,
                        callable: NativeInterpreterFn::new(|_| {
                            Err(provider::ProviderError::invalid_argument(
                                "rejected by provider",
                            ))
                        }),
                    },
                )]),
            )
            .expect("register failing provider");
        let runtime = Runtime::new(failing_providers);
        let report = runtime
            .link(&package)
            .expect("link failing provider")
            .execute(ExecutionRequest::default().trace(TracePolicy::MetadataOnly));
        assert_eq!(
            report.termination_reason(),
            TerminationReason::ProviderError
        );
        assert_eq!(report.provider_call_traces.len(), 1);
        assert_eq!(
            report.provider_call_traces[0].result,
            Err(provider::ProviderErrorCode::InvalidArgument)
        );
        assert!(
            report
                .failure()
                .is_some_and(|error| error.message == "provider call failed (invalid_argument)")
        );
    }

    #[test]
    fn default_reports_redact_provider_controlled_failure_text_and_payloads() {
        let compiler = Compiler;
        let package = compiler
            .compile_with_interfaces(
                &[(
                    "main.rss",
                    "module app\nuse host.test.*\nfn main() -> Unit { fail(); return Unit }",
                )],
                &[("test.rssi", "module host.test\npub fn fail() -> Unit\n")],
            )
            .expect("compile package");
        let package = admitted(package);
        let signature = FunctionSignature {
            parameters: vec![],
            result: "Unit".into(),
            asynchronous: false,
        };
        let symbol = ExternalSymbol::new("host.test.fail").expect("symbol");
        let descriptor = ProviderDescriptor {
            provider_id: "test.failure".into(),
            provider_version: "1".into(),
            supported_abi: vec![RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "fail".into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::NotApplicable,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: provider::ResourceCleanupContract::None,
                error_mapping: provider::ProviderErrorMapping::StructuredV1,
            }],
        };
        let mut providers = ProviderRegistry::default();
        providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable: NativeInterpreterFn::new(|_| {
                            let mut error = provider::ProviderError::invalid_argument(
                                "secret-token=do-not-report",
                            );
                            error.details = Some(serde_json::json!({
                                "credential": "do-not-report"
                            }));
                            Err(error)
                        }),
                    },
                )]),
            )
            .expect("register provider");

        let report = Runtime::new(providers)
            .link(&package)
            .expect("link provider")
            .execute(ExecutionRequest::default());
        assert_eq!(
            report.termination_reason(),
            TerminationReason::ProviderError
        );
        assert!(report.provider_call_traces.is_empty());
        assert_eq!(report.telemetry.provider_functions.len(), 1);
        let failure = report.failure().expect("provider failure evidence");
        assert_eq!(failure.message, "provider call failed (invalid_argument)");
        let serialized = serde_json::to_string(&report).expect("serialize report");
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("credential"));
    }

    #[test]
    fn provider_host_context_and_trace_reach_the_execution_report() {
        let compiler = Compiler;
        let package = compiler
            .compile_with_interfaces(
                &[(
                    "main.rss",
                    "module app\nuse host.log.*\nfn main() -> Unit { emit(message: read \"ok\"); return Unit }",
                )],
                &[(
                    "log.rssi",
                    "module host.log\npub fn emit(message: read String) -> Unit\n",
                )],
            )
            .expect("compile external call");
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "message".into(),
                effect: DataEffect::Read,
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        };
        let symbol = ExternalSymbol::new("host.log.emit").expect("symbol");
        let descriptor = ProviderDescriptor {
            provider_id: "test.log".into(),
            provider_version: "1".into(),
            supported_abi: vec![RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "emit".into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::NotApplicable,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: provider::ResourceCleanupContract::None,
                error_mapping: provider::ProviderErrorMapping::StructuredV1,
            }],
        };
        let mut providers = ProviderRegistry::default();
        providers.set_host_call_context(provider::HostCallContext::with_labels(["log.emit"]));
        providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable: NativeInterpreterFn::new_contextual(|context, _| {
                            assert!(context.host_context.has_label("log.emit"));
                            assert_eq!(context.provider_id, "test.log");
                            assert_eq!(context.symbol, "host.log.emit");
                            Ok(NativeValue::Unit)
                        }),
                    },
                )]),
            )
            .expect("register provider");

        let package = admitted(package);
        let runtime = Runtime::new(providers);
        let report = runtime
            .link(&package)
            .expect("link provider")
            .execute(ExecutionRequest::default().trace(TracePolicy::MetadataOnly));
        assert_eq!(report.provider_call_traces.len(), 1);
        let trace = &report.provider_call_traces[0];
        assert_eq!(trace.provider_id, "test.log");
        assert_eq!(trace.provider_version, "1");
        assert_eq!(trace.symbol, "host.log.emit");
        assert_eq!(trace.request_bytes, 2);
        assert_eq!(trace.response_bytes, 0);
        assert_eq!(trace.result, Ok(()));
        assert_eq!(report.telemetry.provider_functions.len(), 1);
        let summary = &report.telemetry.provider_functions[0];
        assert_eq!(summary.provider_id, "test.log");
        assert_eq!(summary.symbol, "host.log.emit");
        assert_eq!(summary.calls, 1);
        assert_eq!(summary.failures, 0);
        assert_eq!(summary.request_bytes, 2);
        assert_eq!(summary.response_bytes, 0);
        assert_eq!(summary.total_duration_ns, summary.max_duration_ns);
    }
}
