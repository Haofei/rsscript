#![forbid(unsafe_code)]

// Public exports are deliberately explicit so adding a compiler, bytecode, or
// VM symbol cannot silently expand the embedding contract.
#[cfg(feature = "execution")]
pub use rsscript_artifact::{
    ARTIFACT_BUNDLE_MAGIC, ARTIFACT_BUNDLE_SCHEMA, AnalysisEnvelopeV1, AnalysisSchemaV1,
    ArtifactBundle, ArtifactBundleError, ArtifactIdentityV1, AwaitFactV1, BuildProvenanceV1,
    CallEdgeFactV1, ChangedFactV1, CountChangeV1, DiagnosticFactV1, ExportFactV1,
    ExternalCallFactV1, ExternalContractFactV1, FactSetDiffV1, FunctionParameterFactV1,
    InterfaceRequirementV1, PACKAGE_ANALYSIS_SCHEMA, PackageAnalysisV1, ResourceLifetimeFactV1,
    ResourceTransferFactV1, SEMANTIC_DIFF_SCHEMA, SOURCE_ANALYSIS_SCHEMA, SemanticDiffV2,
    SourceAnalysisV1, TaskGroupFactV1,
};
#[cfg(feature = "execution")]
#[allow(unused_imports)]
use rsscript_bytecode::*;
#[allow(unused_imports)]
use rsscript_compiler::*;
use rsscript_semantics::CompilationSession;
#[cfg(feature = "execution")]
#[allow(unused_imports)]
use rsscript_vm::*;
#[cfg(feature = "execution")]
use sha2::{Digest, Sha256};
#[cfg(feature = "execution")]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
#[cfg(feature = "project")]
use std::path::Path;
#[cfg(feature = "execution")]
use std::time::{Duration, Instant};

#[cfg(feature = "execution")]
mod execution;
#[cfg(feature = "execution")]
pub use execution::*;

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
    use rsscript_project::{ProjectLoadError, ProjectLoadErrorCode};

    /// The explicit OS/project capture boundary. It owns workspace traversal,
    /// manifest dependency capture, and conversion into immutable frontend
    /// inputs; it does not depend on the compiler, Artifact, or VM.
    pub use rsscript_project::ProjectLoader;
    /// Compatibility name for the project crate's immutable capture result.
    /// New code can use [`ProjectLoader`] and this type without depending on
    /// any SDK implementation detail.
    pub use rsscript_project::ProjectSnapshot as CapturedProjectSnapshot;

    /// SDK composition helper for a project capture plus the pure in-memory
    /// compiler. Filesystem work stays in [`ProjectLoader`].
    #[derive(Default)]
    pub struct ProjectCompiler {
        loader: ProjectLoader,
    }

    impl ProjectCompiler {
        pub fn new() -> Self {
            Self {
                loader: ProjectLoader::default(),
            }
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
            self.loader
                .capture_from(base, package_dir)
                .map_err(map_project_load_error)
        }

        /// Operation-aware capture that keeps cancellation and deadline checks
        /// in the loader boundary before immutable frontend input is created.
        pub fn capture_frontend_from_with_operation(
            &self,
            base: &Path,
            package_dir: &Path,
            operation: &OperationContext,
        ) -> Result<CapturedProjectSnapshot, CompileError> {
            self.loader
                .capture_from_with_operation(base, package_dir, operation)
                .map_err(map_project_load_error)
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

    fn map_project_load_error(error: ProjectLoadError) -> CompileError {
        let code = match error.code() {
            ProjectLoadErrorCode::Cancelled => CompileErrorCode::Cancelled,
            ProjectLoadErrorCode::DeadlineExceeded => CompileErrorCode::DeadlineExceeded,
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
        ArtifactBundle, ArtifactBundleError, ArtifactOriginVerifier, ArtifactVerifier,
        BuildProvenanceV1, BuiltArtifact, InterfaceRequirementV1, OriginVerifiedAdmission,
        PACKAGE_ANALYSIS_SCHEMA, PackageAnalysisV1, SOURCE_ANALYSIS_SCHEMA, SourceAnalysisV1,
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
        AsyncWireInterpreterFn, AsyncWireMutationInterpreterFn, BlockingBehavior,
        CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature, HostCallContext,
        ParameterSignature, ProviderCallContext, ProviderCallMode, ProviderDescriptor,
        ProviderError, ProviderErrorCode, ProviderErrorMapping, ProviderFunction,
        ProviderFunctionDescriptor, ProviderLoadError, ProviderReplayContract, ProviderReplayEntry,
        ProviderReplayMode, ProviderReplayNormalization, ProviderReplayPersistence,
        ProviderReplayRedaction, ProviderReplayTape, ProviderReplayability, ProviderResource,
        RUNTIME_ABI_VERSION, ResourceCleanupContract, ResourceHandle, WireInterpreterFn,
        WireMutationInterpreterFn, WireMutationProviderFuture, WireMutationResult,
        WireProviderFuture, WireValue, replayable_async_wire_callable, replayable_wire_callable,
    };
}

#[cfg(feature = "execution")]
/// Reviewed linking and bounded execution entry points.
pub mod runtime {
    pub use super::{
        AuditPolicy, ExecutionProfileV1, ExecutionRequest, LinkError, LinkedArtifact,
        NondeterminismPolicy, RunLimits, Runtime, TracePolicy,
    };
}

/// Explicitly unstable, opt-in execution engines.
///
/// Types in this module are outside the reviewed SDK compatibility contract.
/// Hosts must opt into the `native-jit` Cargo feature and pin the repository
/// revision while using them.
#[cfg(feature = "native-jit")]
pub mod experimental {
    pub mod native_jit {
        pub use rsscript_vm::{NativeCostModel, NativeJitOptions};
    }
}

#[cfg(feature = "execution")]
/// Reviewed machine-readable execution-report types.
pub mod report {
    pub use super::{
        EXECUTION_REPORT_SCHEMA, ExecutionOutcome, ExecutionReport, ExecutionTelemetry,
        ProviderFunctionTelemetry, RuntimeError, TerminationReason,
    };
    pub use rsscript_vm::ExecutionEngineTelemetry;
}

#[cfg(feature = "execution")]
/// Reviewed neutral Artifact analysis and semantic-diff data.
pub mod analysis {
    pub use super::{
        CallEdgeFactV1, ExternalContractFactV1, FunctionParameterFactV1, ResourceLifetimeFactV1,
        ResourceTransferFactV1, SEMANTIC_DIFF_SCHEMA, SemanticDiffV2, TaskGroupFactV1,
    };
}
#[allow(unused_imports)]
use rsscript_operation::*;
#[cfg(feature = "execution")]
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
        analyze_snapshot_with_session(snapshot, None)
            .expect("an unchecked SDK snapshot check cannot abort")
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
        analyze_snapshot_with_session(snapshot, Some(operation))
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
        let validated = validate_snapshot_with_session(snapshot, None)?;
        let snapshot_digest = in_memory_snapshot_digest(&sources, &interfaces);
        let artifact = compile_validated_to_bytecode(&validated, &snapshot_digest)
            .map_err(bytecode_compile_error)?;
        BuiltArtifact::from_bytecode(
            artifact,
            source_set_analysis(&validated, &sources, &snapshot_digest),
        )
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
        let validated = validate_snapshot_with_session(snapshot, Some(operation))?;
        operation.check().map_err(CompileError::from)?;
        let snapshot_digest = in_memory_snapshot_digest(&sources, &interfaces);
        let artifact = compile_validated_to_bytecode(&validated, &snapshot_digest)
            .map_err(bytecode_compile_error)?;
        operation.check().map_err(CompileError::from)?;
        let built = BuiltArtifact::from_bytecode(
            artifact,
            source_set_analysis(&validated, &sources, &snapshot_digest),
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

/// Adopt the semantic-owned session query for ordinary immutable snapshots.
/// Exceptional legacy snapshots are delegated to a private compatibility
/// adapter. Normal production callers have no switch that can select that
/// adapter: all session-compatible inputs use the semantic-owned query below.
#[cfg(feature = "execution")]
fn validate_snapshot_with_session(
    snapshot: &FrontendInputSnapshot,
    operation: Option<&OperationContext>,
) -> Result<ValidatedProgram, CompileError> {
    if legacy_frontend_fixtures::snapshot_reason(snapshot).is_some() {
        return legacy_frontend_fixtures::validate_snapshot(snapshot, operation);
    }

    let mut session = session_for_snapshot(snapshot);
    match operation {
        Some(operation) => session
            .workspace_validated_with_operation(operation)
            .map_err(CompileError::from)?
            .map_err(CompileError::Diagnostics),
        None => session
            .workspace_validated()
            .map_err(CompileError::Diagnostics),
    }
}

/// Analyze a snapshot through the same session-owned workspace query used by
/// compilation. The sole fallback is the private legacy-fixture adapter used
/// for historical inputs that cannot have stable session identities.
fn analyze_snapshot_with_session(
    snapshot: &FrontendInputSnapshot,
    operation: Option<&OperationContext>,
) -> Result<Vec<Diagnostic>, CompileError> {
    if legacy_frontend_fixtures::snapshot_reason(snapshot).is_some() {
        return legacy_frontend_fixtures::analyze_snapshot(snapshot, operation);
    }

    let mut session = session_for_snapshot(snapshot);
    match operation {
        Some(operation) => session
            .workspace_analysis_with_operation(operation)
            .map_err(CompileError::from)
            .map(|analysis| (*analysis).clone().into_diagnostics()),
        None => Ok((*session.workspace_analysis()).clone().into_diagnostics()),
    }
}

fn session_for_snapshot(snapshot: &FrontendInputSnapshot) -> CompilationSession {
    debug_assert!(legacy_frontend_fixtures::snapshot_reason(snapshot).is_none());
    let mut session = CompilationSession::default();
    for file in snapshot.sources().files() {
        session
            .set_file(file.path(), file.text())
            .expect("non-empty immutable snapshot source path must be session-valid");
    }
    for file in snapshot.interfaces().files() {
        session
            .set_interface(file.path(), file.text())
            .expect("non-empty immutable snapshot interface path must be session-valid");
    }
    session
}

/// Historical frontend inputs which cannot be represented by the immutable
/// session source store without changing their asserted diagnostic behavior.
///
/// This module is intentionally private. It exists solely to preserve a small
/// migration corpus while session callers take the production path. In
/// particular, neither [`Compiler`] nor an embedding API exposes a mode that
/// chooses direct analyzer calls.
mod legacy_frontend_fixtures {
    use super::*;

    /// A session assigns one stable identity per non-empty logical path.
    /// Historical direct-analyzer fixtures may instead use an empty path or
    /// duplicate a source/interface path to assert an old diagnostic.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum SnapshotReason {
        Empty,
        DuplicateSource,
        DuplicateInterface,
    }

    pub(super) fn snapshot_reason(snapshot: &FrontendInputSnapshot) -> Option<SnapshotReason> {
        let source_paths = snapshot
            .sources()
            .files()
            .iter()
            .map(|file| file.path())
            .collect::<BTreeSet<_>>();
        let interface_paths = snapshot
            .interfaces()
            .files()
            .iter()
            .map(|file| file.path())
            .collect::<BTreeSet<_>>();
        if source_paths.iter().any(|path| path.is_empty())
            || interface_paths.iter().any(|path| path.is_empty())
        {
            Some(SnapshotReason::Empty)
        } else if source_paths.len() != snapshot.sources().files().len() {
            Some(SnapshotReason::DuplicateSource)
        } else if interface_paths.len() != snapshot.interfaces().files().len() {
            Some(SnapshotReason::DuplicateInterface)
        } else {
            None
        }
    }

    #[cfg(feature = "execution")]
    pub(super) fn validate_snapshot(
        snapshot: &FrontendInputSnapshot,
        operation: Option<&OperationContext>,
    ) -> Result<ValidatedProgram, CompileError> {
        let sources = snapshot_pairs(snapshot.sources());
        let interfaces = snapshot_pairs(snapshot.interfaces());
        match operation {
            Some(operation) => {
                validate_sources_with_interfaces_with_operation(&sources, &interfaces, operation)
                    .map_err(|diagnostics| match operation.check() {
                        Ok(()) => CompileError::Diagnostics(diagnostics),
                        Err(abort) => CompileError::from(abort),
                    })
            }
            None => validate_sources_with_interfaces(&sources, &interfaces)
                .map_err(CompileError::Diagnostics),
        }
    }

    pub(super) fn analyze_snapshot(
        snapshot: &FrontendInputSnapshot,
        operation: Option<&OperationContext>,
    ) -> Result<Vec<Diagnostic>, CompileError> {
        let sources = snapshot_pairs(snapshot.sources());
        let interfaces = snapshot_pairs(snapshot.interfaces());
        match operation {
            Some(operation) => {
                operation.check().map_err(CompileError::from)?;
                let diagnostics = analyze_sources_with_interfaces_result_with_operation(
                    &sources,
                    &interfaces,
                    operation,
                )
                .into_diagnostics();
                operation.check().map_err(CompileError::from)?;
                Ok(diagnostics)
            }
            None => Ok(analyze_sources_with_interfaces(&sources, &interfaces)),
        }
    }
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

#[cfg(feature = "execution")]
fn bytecode_compile_error(error: impl fmt::Display) -> CompileError {
    CompileError::Bytecode {
        code: CompileErrorCode::Bytecode,
        message: error.to_string(),
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
    Profile(String),
}

#[cfg(feature = "execution")]
impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => write!(formatter, "provider link failed: {error}"),
            Self::Profile(error) => write!(formatter, "execution profile rejected link: {error}"),
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
mod tests;
