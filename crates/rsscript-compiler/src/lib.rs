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
pub use rsscript_provider_api as provider;

#[cfg(feature = "execution")]
use provider::{NativeInterpreterFn, ProviderDescriptor, ProviderFunction, ProviderLoadError};
use rsscript::analyze_source;
#[cfg(feature = "execution")]
use rsscript::{
    EvalError, ExternalFunctionRegistry, NativeValue, PackageAnalysis, RegVmExecutable, VmLimits,
    analyze_package_dir, reg_vm_compile_package, reg_vm_compile_source, reg_vm_compile_validated,
    validate_sources_with_interfaces,
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
        })
    }

    #[cfg(feature = "execution")]
    pub fn compile_package(&self, path: &Path) -> Result<CompiledPackage, CompileError> {
        let analysis = analyze_package_dir(path).map_err(CompileError::Package)?;
        if analysis.summary.errors != 0 {
            return Err(CompileError::Diagnostics(analysis.diagnostics.clone()));
        }
        let executable = reg_vm_compile_package(path).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: Some(analysis),
        })
    }

    #[cfg(feature = "execution")]
    pub fn load_verified(&self, bytecode: &[u8]) -> Result<CompiledPackage, CompileError> {
        let executable = RegVmExecutable::from_bytecode(bytecode).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
        })
    }
}

#[cfg(feature = "execution")]
pub struct CompiledPackage {
    executable: RegVmExecutable,
    analysis: Option<PackageAnalysis>,
}

#[cfg(feature = "execution")]
impl CompiledPackage {
    pub fn bytecode(&self) -> Result<Vec<u8>, CompileError> {
        self.executable.to_bytecode().map_err(CompileError::from)
    }

    pub fn analysis(&self) -> Option<&PackageAnalysis> {
        self.analysis.as_ref()
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
    pub provider_call_budget: Option<u64>,
}

#[cfg(feature = "execution")]
impl RunLimits {
    pub fn bounded() -> Self {
        VmLimits::safe_default().into()
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
            provider_call_budget: limits.host_call_budget,
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
            host_call_budget: limits.provider_call_budget,
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
                type_name: "String".into(),
                retained: false,
            }],
            return_type: "Unit".into(),
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
}
