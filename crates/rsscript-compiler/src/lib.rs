#![forbid(unsafe_code)]

pub use rsscript_compiler_core::Diagnostic;
#[cfg(feature = "execution")]
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
#[cfg(feature = "execution")]
use std::path::Path;

/// Frontend-only editor API consumed by `rsscript-language-service`.
/// Runtime and Provider types are deliberately excluded.
pub mod language {
    pub use rsscript_compiler_core::{
        Definition, Diagnostic, DiagnosticExplanation, Reference, RssDocumentSymbol, Severity,
        Span, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
        analyze_source_result_with_operation, analyze_source_with_core,
        analyze_source_with_interfaces, analyze_source_with_interfaces_result_with_operation,
        analyze_sources_with_interfaces, document_symbols, explain_diagnostic_code, format_source,
        lint_source, symbol_index,
    };
}
#[cfg(feature = "execution")]
pub use rsscript_compiler_core::ExecutionUsage;
#[cfg(feature = "execution")]
pub use rsscript_compiler_core::WorkspaceSnapshot;
pub use rsscript_operation::{
    CancellationToken, MonotonicDeadline, OperationAbort, OperationContext, OperationId,
};
#[cfg(feature = "execution")]
pub use rsscript_provider_api as provider;

#[cfg(feature = "execution")]
use provider::{NativeInterpreterFn, ProviderDescriptor, ProviderFunction, ProviderLoadError};
#[cfg(feature = "execution")]
use rsscript_compiler_core::{
    EvalError, ExecutionFailureKind, ExternalFunctionRegistry, NativeValue, PackageAnalysis,
    RegVmExecutable, VmLimits, load_workspace_snapshot, load_workspace_snapshot_with_operation,
    reg_vm_compile_package_input, reg_vm_compile_source, reg_vm_compile_validated,
    validate_source_with_operation, validate_sources_with_interfaces,
    validate_sources_with_interfaces_with_operation,
};
use rsscript_compiler_core::{analyze_source, analyze_source_result_with_operation};

#[derive(Default)]
pub struct Compiler;

impl Compiler {
    pub fn check(&self, file: &str, source: &str) -> Vec<Diagnostic> {
        analyze_source(file, source)
    }

    pub fn check_with_operation(
        &self,
        file: &str,
        source: &str,
        operation: &OperationContext,
    ) -> Vec<Diagnostic> {
        analyze_source_result_with_operation(file, source, operation).into_diagnostics()
    }

    #[cfg(feature = "execution")]
    pub fn compile(&self, file: &str, source: &str) -> Result<CompiledPackage, CompileError> {
        let executable = reg_vm_compile_source(file, source).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }

    #[cfg(feature = "execution")]
    pub fn compile_with_operation(
        &self,
        file: &str,
        source: &str,
        operation: &OperationContext,
    ) -> Result<CompiledPackage, CompileError> {
        let validated =
            validate_source_with_operation(file, source, operation).map_err(|diagnostics| {
                match operation.check() {
                    Ok(()) => CompileError::Diagnostics(diagnostics),
                    Err(abort) => CompileError::from(abort),
                }
            })?;
        operation.check().map_err(CompileError::from)?;
        let executable = reg_vm_compile_validated(&validated).map_err(CompileError::from)?;
        operation.check().map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }

    /// Compile a source snapshot against explicit host interfaces. The
    /// interfaces contribute semantic signatures only; provider selection is
    /// intentionally deferred until execution.
    #[cfg(feature = "execution")]
    pub fn compile_with_interfaces(
        &self,
        sources: &[(&str, &str)],
        interfaces: &[(&str, &str)],
    ) -> Result<CompiledPackage, CompileError> {
        let validated = validate_sources_with_interfaces(sources, interfaces)
            .map_err(CompileError::Diagnostics)?;
        let executable = reg_vm_compile_validated(&validated).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }

    #[cfg(feature = "execution")]
    pub fn compile_with_interfaces_and_operation(
        &self,
        sources: &[(&str, &str)],
        interfaces: &[(&str, &str)],
        operation: &OperationContext,
    ) -> Result<CompiledPackage, CompileError> {
        let validated =
            validate_sources_with_interfaces_with_operation(sources, interfaces, operation)
                .map_err(|diagnostics| match operation.check() {
                    Ok(()) => CompileError::Diagnostics(diagnostics),
                    Err(abort) => CompileError::from(abort),
                })?;
        operation.check().map_err(CompileError::from)?;
        let executable = reg_vm_compile_validated(&validated).map_err(CompileError::from)?;
        operation.check().map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }

    #[cfg(feature = "execution")]
    pub fn compile_package(&self, path: &Path) -> Result<CompiledPackage, CompileError> {
        let snapshot = self.snapshot(path)?;
        self.build(&snapshot)
    }

    #[cfg(feature = "execution")]
    pub fn compile_package_with_operation(
        &self,
        path: &Path,
        operation: &OperationContext,
    ) -> Result<CompiledPackage, CompileError> {
        let snapshot = self.snapshot_with_operation(path, operation)?;
        self.build_with_operation(&snapshot, operation)
    }

    /// Capture all package and dependency inputs exactly once.
    #[cfg(feature = "execution")]
    pub fn snapshot(&self, path: &Path) -> Result<WorkspaceSnapshot, CompileError> {
        load_workspace_snapshot(path).map_err(|message| CompileError::Package {
            code: CompileErrorCode::PackageSnapshot,
            message,
        })
    }

    #[cfg(feature = "execution")]
    pub fn snapshot_with_operation(
        &self,
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

    /// Build analysis and executable bytes from one immutable snapshot.
    #[cfg(feature = "execution")]
    pub fn build(&self, snapshot: &WorkspaceSnapshot) -> Result<CompiledPackage, CompileError> {
        let mut analysis = snapshot.analysis().clone();
        if analysis.summary.errors != 0 {
            return Err(CompileError::Diagnostics(analysis.diagnostics.clone()));
        }
        let mut executable =
            reg_vm_compile_package_input(snapshot.lowering_input()).map_err(CompileError::from)?;
        executable.bind_snapshot_digest(snapshot.digest())?;
        analysis.module_digest = Some(
            executable
                .bytecode_artifact()
                .header
                .executable_hash
                .clone(),
        );
        Ok(CompiledPackage {
            executable,
            analysis: Some(analysis),
            snapshot_digest: Some(snapshot.digest().to_string()),
        })
    }

    #[cfg(feature = "execution")]
    pub fn build_with_operation(
        &self,
        snapshot: &WorkspaceSnapshot,
        operation: &OperationContext,
    ) -> Result<CompiledPackage, CompileError> {
        operation.check().map_err(CompileError::from)?;
        let package = self.build(snapshot)?;
        operation.check().map_err(CompileError::from)?;
        Ok(package)
    }

    #[cfg(feature = "execution")]
    pub fn load_verified(&self, bytecode: &[u8]) -> Result<CompiledPackage, CompileError> {
        let executable = RegVmExecutable::from_bytecode(bytecode).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }

    #[cfg(feature = "execution")]
    pub fn load_verified_with_operation(
        &self,
        bytecode: &[u8],
        operation: &OperationContext,
    ) -> Result<CompiledPackage, CompileError> {
        operation.check().map_err(CompileError::from)?;
        let executable = RegVmExecutable::from_bytecode_with_operation(bytecode, operation)
            .map_err(|error| match operation.check() {
                Ok(()) => CompileError::from(error),
                Err(abort) => CompileError::from(abort),
            })?;
        operation.check().map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }
}

#[cfg(feature = "execution")]
#[derive(Debug)]
pub struct CompiledPackage {
    executable: RegVmExecutable,
    analysis: Option<PackageAnalysis>,
    snapshot_digest: Option<String>,
}

#[cfg(feature = "execution")]
impl CompiledPackage {
    pub fn bytecode(&self) -> Result<Vec<u8>, CompileError> {
        self.executable.to_bytecode().map_err(CompileError::from)
    }

    pub fn analysis(&self) -> Option<&PackageAnalysis> {
        self.analysis.as_ref()
    }

    pub fn snapshot_digest(&self) -> Option<&str> {
        self.snapshot_digest.as_deref()
    }

    pub fn module_digest(&self) -> &str {
        &self.executable.bytecode_artifact().header.executable_hash
    }

    pub fn external_imports(&self) -> &[rsscript_compiler_core::ExternalImport] {
        &self.executable.bytecode_artifact().imports
    }
}

#[cfg(feature = "execution")]
#[derive(Default)]
pub struct ProviderRegistry {
    inner: ExternalFunctionRegistry,
}

#[cfg(feature = "execution")]
impl ProviderRegistry {
    /// Attach host-defined, instance-local authority to every resolved call.
    /// Providers decide how to interpret these scopes; the language does not.
    pub fn set_authority(&mut self, authority: provider::ProviderAuthority) {
        self.inner.set_authority(authority);
    }

    pub fn register(
        &mut self,
        descriptor: &ProviderDescriptor,
        functions: BTreeMap<provider::ExternalSymbol, ProviderFunction<NativeInterpreterFn>>,
    ) -> Result<(), ProviderLoadError> {
        self.inner.register_provider(descriptor, functions)
    }
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone)]
pub struct RunLimits {
    pub max_depth: usize,
    pub step_budget: Option<u64>,
    pub allocation_budget: Option<usize>,
    pub cancellation: Option<CancellationToken>,
    pub deadline: Option<MonotonicDeadline>,
    pub output_budget: Option<usize>,
    pub intrinsic_call_budget: Option<u64>,
    pub provider_call_budget: Option<u64>,
    pub resource_limit: Option<usize>,
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
            cancellation: limits.cancel,
            deadline: limits.deadline,
            output_budget: limits.stdout_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
            resource_limit: limits.resource_limit,
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
            cancel: limits.cancellation,
            deadline: limits.deadline,
            stdout_budget: limits.output_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
            resource_limit: limits.resource_limit,
        }
    }
}

#[cfg(feature = "execution")]
pub struct Runtime {
    providers: ProviderRegistry,
    limits: RunLimits,
}

#[cfg(feature = "execution")]
impl Runtime {
    pub fn new(providers: ProviderRegistry, limits: RunLimits) -> Self {
        Self { providers, limits }
    }

    /// Execute and always return an audit report, including partial evidence
    /// for cancellation, budget exhaustion, Provider failures, and preflight
    /// rejection.
    pub fn execute(
        &self,
        package: &CompiledPackage,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> ExecutionReport {
        for import in package.external_imports() {
            if let Err(error) = self.providers.inner.resolve(import) {
                let failure = RuntimeError {
                    reason: TerminationReason::VerificationFailure,
                    message: error.to_string(),
                };
                return ExecutionReport::failed(package.module_digest(), failure, Vec::new());
            }
        }
        let output = match package
            .executable
            .execute_main_with_args_and_external_bindings_and_limits(
                args,
                self.providers.inner.bindings(),
                self.limits.clone().into(),
            ) {
            Ok(output) => output,
            Err(error) => {
                let diagnostics = match &error {
                    EvalError::Diagnostics(diagnostics) => diagnostics.clone(),
                    _ => Vec::new(),
                };
                return ExecutionReport::failed(
                    package.module_digest(),
                    RuntimeError::from(error),
                    diagnostics,
                );
            }
        };
        let diagnostics = match &output.failure {
            Some(EvalError::Diagnostics(diagnostics)) => diagnostics.clone(),
            _ => Vec::new(),
        };
        let failure = output.failure.map(RuntimeError::from);
        ExecutionReport {
            artifact_digest: package.module_digest().to_string(),
            termination_reason: failure
                .as_ref()
                .map_or(TerminationReason::Completed, |error| error.reason),
            usage: output.usage,
            value: output.value.unwrap_or_default(),
            display_value: output.display_value.unwrap_or_default(),
            native_value: output.native_value,
            stdout: output.stdout,
            stderr: output.stderr,
            provider_call_traces: output.provider_call_traces,
            diagnostics,
            failure,
        }
    }

    /// Compatibility helper for callers that prefer ordinary `Result`
    /// control flow. Use [`Self::execute`] when failure evidence is required.
    pub fn run(
        &self,
        package: &CompiledPackage,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<ExecutionReport, RuntimeError> {
        let report = self.execute(package, args);
        match report.failure.clone() {
            Some(error) => Err(error),
            None => Ok(report),
        }
    }
}

#[cfg(feature = "execution")]
impl Default for Runtime {
    fn default() -> Self {
        Self::new(ProviderRegistry::default(), RunLimits::default())
    }
}

#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionReport {
    pub artifact_digest: String,
    pub termination_reason: TerminationReason,
    pub usage: ExecutionUsage,
    pub value: String,
    pub display_value: String,
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
    pub provider_call_traces: Vec<provider::ProviderCallTrace>,
    pub diagnostics: Vec<Diagnostic>,
    pub failure: Option<RuntimeError>,
}

#[cfg(feature = "execution")]
impl ExecutionReport {
    fn failed(
        artifact_digest: impl Into<String>,
        failure: RuntimeError,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            artifact_digest: artifact_digest.into(),
            termination_reason: failure.reason,
            usage: ExecutionUsage::default(),
            value: String::new(),
            display_value: String::new(),
            native_value: None,
            stdout: String::new(),
            stderr: String::new(),
            provider_call_traces: Vec::new(),
            diagnostics,
            failure: Some(failure),
        }
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
    Bytecode,
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
            Self::Bytecode => "bytecode",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationReason {
    Completed,
    ScriptError,
    Cancelled,
    DeadlineExceeded,
    StepBudgetExceeded,
    AllocationBudgetExceeded,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub reason: TerminationReason,
    pub message: String,
}

#[cfg(feature = "execution")]
impl From<EvalError> for RuntimeError {
    fn from(error: EvalError) -> Self {
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
                reason: TerminationReason::ProviderError,
                message: error.to_string(),
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

        let compiler = Compiler;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        let error = compiler
            .snapshot_with_operation(directory.path(), &cancelled)
            .expect_err("cancelled snapshot");
        assert_eq!(error.code(), CompileErrorCode::Cancelled);
        let snapshot = compiler.snapshot(directory.path()).expect("snapshot");
        std::fs::write(&source_path, "fn main() -> Int { return 2 }").expect("mutate checkout");

        let first = compiler.build(&snapshot).expect("first build");
        let second = compiler.build(&snapshot).expect("repeat build");
        assert_eq!(first.bytecode().unwrap(), second.bytecode().unwrap());
        assert_eq!(first.snapshot_digest(), Some(snapshot.digest()));
        let analysis = first.analysis().expect("package analysis");
        assert_eq!(analysis.snapshot_digest, snapshot.digest());
        assert_eq!(
            analysis.module_digest.as_deref(),
            Some(first.module_digest())
        );
        let artifact = rsscript_bytecode::BytecodeArtifact::from_bytes(&first.bytecode().unwrap())
            .expect("artifact envelope");
        assert_eq!(
            artifact.header.snapshot_digest.as_deref(),
            Some(snapshot.digest())
        );
        assert_eq!(analysis.language_version, artifact.header.language_version);
        assert_eq!(analysis.producer.version, artifact.header.compiler_version);
        assert_eq!(
            analysis.interface_catalog_digest,
            artifact.header.interface_catalog_digest
        );
        let output = Runtime::default()
            .run(&first, Vec::<String>::new())
            .expect("run captured source");
        assert_eq!(output.value, "1");
    }

    #[test]
    fn stable_facade_compiles_serializes_loads_and_runs() {
        let compiler = Compiler;
        let package = compiler
            .compile("main.rss", "fn main() -> Unit { return Unit }")
            .expect("compile");
        let bytecode = package.bytecode().expect("bytecode");
        let loaded = compiler.load_verified(&bytecode).expect("load verified");
        let report = Runtime::default()
            .run(&loaded, Vec::<String>::new())
            .expect("run");
        assert_eq!(report.value, "Unit");
        assert_eq!(report.termination_reason, TerminationReason::Completed);
        assert_eq!(report.artifact_digest, loaded.module_digest());
        assert!(report.usage.steps_consumed > 0);
        assert_eq!(report.termination_reason.as_str(), "completed");
        assert_eq!(
            CompileErrorCode::PackageSnapshot.as_str(),
            "package_snapshot"
        );
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
        let error = compiler
            .load_verified_with_operation(&package.bytecode().unwrap(), &expired)
            .expect_err("expired verifier deadline");
        assert_eq!(error.code(), CompileErrorCode::DeadlineExceeded);
        assert!(error.to_string().contains("deadline exceeded"));
    }

    #[test]
    fn module_interface_keeps_stable_external_symbol_and_preflights_signature() {
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
        assert_eq!(package.external_imports().len(), 1);
        assert_eq!(
            package.external_imports()[0].symbol.as_str(),
            "host.log.emit"
        );

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

        let error = Runtime::new(providers, RunLimits::bounded())
            .run(&package, Vec::<String>::new())
            .expect_err("import signature must fail before execution");
        assert_eq!(error.reason, TerminationReason::VerificationFailure);
        assert!(error.message.contains("ImportSignatureMismatch"));
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
        let limits = RunLimits {
            provider_call_budget: Some(1),
            ..RunLimits::default()
        };

        let report = Runtime::new(providers, limits).execute(&package, Vec::<String>::new());
        assert_eq!(
            report.termination_reason,
            TerminationReason::ProviderBudgetExceeded
        );
        assert_eq!(report.usage.provider_calls, 2);
        assert_eq!(report.provider_call_traces.len(), 1);
        assert!(
            report
                .failure
                .as_ref()
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
        let report = Runtime::new(failing_providers, RunLimits::bounded())
            .execute(&package, Vec::<String>::new());
        assert_eq!(report.termination_reason, TerminationReason::ProviderError);
        assert_eq!(report.provider_call_traces.len(), 1);
        assert_eq!(
            report.provider_call_traces[0].result,
            Err(provider::ProviderErrorCode::InvalidArgument)
        );
        assert!(
            report
                .failure
                .as_ref()
                .is_some_and(|error| error.message.contains("InvalidArgument"))
        );
    }

    #[test]
    fn provider_authority_and_trace_reach_the_execution_report() {
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
        providers.set_authority(provider::ProviderAuthority::scoped(["log.emit"]));
        providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable: NativeInterpreterFn::new_contextual(|context, _| {
                            assert!(context.authority.allows("log.emit"));
                            assert_eq!(context.provider_id, "test.log");
                            assert_eq!(context.symbol, "host.log.emit");
                            Ok(NativeValue::Unit)
                        }),
                    },
                )]),
            )
            .expect("register provider");

        let report = Runtime::new(providers, RunLimits::bounded())
            .run(&package, Vec::<String>::new())
            .expect("run provider");
        assert_eq!(report.provider_call_traces.len(), 1);
        let trace = &report.provider_call_traces[0];
        assert_eq!(trace.provider_id, "test.log");
        assert_eq!(trace.provider_version, "1");
        assert_eq!(trace.symbol, "host.log.emit");
        assert_eq!(trace.result, Ok(()));
    }
}
