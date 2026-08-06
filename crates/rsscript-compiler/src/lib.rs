#![forbid(unsafe_code)]

#[cfg(feature = "execution")]
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
#[cfg(feature = "execution")]
use std::path::Path;
#[cfg(feature = "execution")]
use std::sync::Arc;
#[cfg(feature = "execution")]
use std::sync::atomic::AtomicBool;

pub use rsscript::Diagnostic;
#[cfg(feature = "execution")]
pub use rsscript::WorkspaceSnapshot;
#[cfg(feature = "execution")]
pub use rsscript_provider_api as provider;

#[cfg(feature = "execution")]
use provider::{NativeInterpreterFn, ProviderDescriptor, ProviderFunction, ProviderLoadError};
use rsscript::analyze_source;
#[cfg(feature = "execution")]
use rsscript::{
    EvalError, ExternalFunctionRegistry, NativeValue, PackageAnalysis, RegVmExecutable, VmLimits,
    load_workspace_snapshot, reg_vm_compile_package_input, reg_vm_compile_source,
    reg_vm_compile_validated, validate_sources_with_interfaces,
};

#[derive(Default)]
pub struct Compiler;

impl Compiler {
    pub fn check(&self, file: &str, source: &str) -> Vec<Diagnostic> {
        analyze_source(file, source)
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
    pub fn compile_package(&self, path: &Path) -> Result<CompiledPackage, CompileError> {
        let snapshot = self.snapshot(path)?;
        self.build(&snapshot)
    }

    /// Capture all package and dependency inputs exactly once.
    #[cfg(feature = "execution")]
    pub fn snapshot(&self, path: &Path) -> Result<WorkspaceSnapshot, CompileError> {
        load_workspace_snapshot(path).map_err(CompileError::Package)
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
    pub fn load_verified(&self, bytecode: &[u8]) -> Result<CompiledPackage, CompileError> {
        let executable = RegVmExecutable::from_bytecode(bytecode).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
            snapshot_digest: None,
        })
    }
}

#[cfg(feature = "execution")]
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

    pub fn external_imports(&self) -> &[rsscript::ExternalImport] {
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
    pub memory_budget: Option<usize>,
    pub cancellation: Option<Arc<AtomicBool>>,
    pub output_budget: Option<usize>,
    pub intrinsic_call_budget: Option<u64>,
    pub provider_call_budget: Option<u64>,
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
            memory_budget: limits.mem_budget,
            cancellation: limits.cancel,
            output_budget: limits.stdout_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
        }
    }
}

#[cfg(feature = "execution")]
impl From<RunLimits> for VmLimits {
    fn from(limits: RunLimits) -> Self {
        Self {
            max_depth: limits.max_depth,
            step_budget: limits.step_budget,
            mem_budget: limits.memory_budget,
            cancel: limits.cancellation,
            stdout_budget: limits.output_budget,
            intrinsic_call_budget: limits.intrinsic_call_budget,
            provider_call_budget: limits.provider_call_budget,
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

    pub fn run(
        &self,
        package: &CompiledPackage,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<ExecutionReport, RuntimeError> {
        for import in package.external_imports() {
            self.providers
                .inner
                .resolve(import)
                .map_err(|error| RuntimeError(error.to_string()))?;
        }
        let output = package
            .executable
            .eval_main_with_args_and_external_bindings_and_limits(
                args,
                self.providers.inner.bindings(),
                self.limits.clone().into(),
            )
            .map_err(RuntimeError::from)?;
        Ok(ExecutionReport {
            value: output.value,
            display_value: output.display_value,
            native_value: output.native_value,
            stdout: output.stdout,
            stderr: output.stderr,
        })
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
    pub value: String,
    pub display_value: String,
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum CompileError {
    Diagnostics(Vec<Diagnostic>),
    Package(String),
    Runtime(String),
}

#[cfg(feature = "execution")]
impl From<EvalError> for CompileError {
    fn from(error: EvalError) -> Self {
        match error {
            EvalError::Diagnostics(diagnostics) => Self::Diagnostics(diagnostics),
            EvalError::Runtime(message) => Self::Runtime(message),
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
            Self::Package(message) => write!(formatter, "package compilation failed: {message}"),
            Self::Runtime(message) => write!(formatter, "bytecode compilation failed: {message}"),
        }
    }
}

impl Error for CompileError {}

#[cfg(feature = "execution")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError(pub String);

#[cfg(feature = "execution")]
impl From<EvalError> for RuntimeError {
    fn from(error: EvalError) -> Self {
        Self(match error {
            EvalError::Diagnostics(diagnostics) => {
                format!(
                    "execution rejected with {} diagnostic(s)",
                    diagnostics.len()
                )
            }
            EvalError::Runtime(message) => message,
        })
    }
}

#[cfg(feature = "execution")]
impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
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
                resource_cleanup_contract: "none".into(),
                error_mapping: "string".into(),
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
        assert!(error.0.contains("ImportSignatureMismatch"));
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
                resource_cleanup_contract: "none".into(),
                error_mapping: "string".into(),
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

        let error = Runtime::new(providers, limits)
            .run(&package, Vec::<String>::new())
            .expect_err("second provider call must exceed the provider budget");
        assert!(error.0.contains("provider call budget exceeded"));
    }
}
