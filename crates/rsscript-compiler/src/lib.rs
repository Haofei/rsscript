#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub use rsscript::Diagnostic;
pub use rsscript_provider_api as provider;

use provider::{NativeInterpreterFn, ProviderDescriptor, ProviderFunction, ProviderLoadError};
use rsscript::{
    EvalError, ExternalFunctionRegistry, NativeValue, PackageAnalysis, RegVmExecutable, VmLimits,
    analyze_package_dir, analyze_source, reg_vm_compile_package, reg_vm_compile_source,
};

#[derive(Default)]
pub struct Compiler;

impl Compiler {
    pub fn check(&self, file: &str, source: &str) -> Vec<Diagnostic> {
        analyze_source(file, source)
    }

    pub fn compile(&self, file: &str, source: &str) -> Result<CompiledPackage, CompileError> {
        let executable = reg_vm_compile_source(file, source).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
        })
    }

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

    pub fn load_verified(&self, bytecode: &[u8]) -> Result<CompiledPackage, CompileError> {
        let executable = RegVmExecutable::from_bytecode(bytecode).map_err(CompileError::from)?;
        Ok(CompiledPackage {
            executable,
            analysis: None,
        })
    }
}

pub struct CompiledPackage {
    executable: RegVmExecutable,
    analysis: Option<PackageAnalysis>,
}

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

#[derive(Default)]
pub struct ProviderRegistry {
    inner: ExternalFunctionRegistry,
}

impl ProviderRegistry {
    pub fn register(
        &mut self,
        descriptor: &ProviderDescriptor,
        functions: BTreeMap<rsscript::ExternalSymbol, ProviderFunction<NativeInterpreterFn>>,
    ) -> Result<(), ProviderLoadError> {
        self.inner.register_provider(descriptor, functions)
    }
}

#[derive(Debug, Clone)]
pub struct RunLimits {
    pub max_depth: usize,
    pub step_budget: Option<u64>,
    pub memory_budget: Option<usize>,
    pub cancellation: Option<Arc<AtomicBool>>,
    pub output_budget: Option<usize>,
    pub provider_call_budget: Option<u64>,
}

impl RunLimits {
    pub fn bounded() -> Self {
        VmLimits::safe_default().into()
    }
}

impl Default for RunLimits {
    fn default() -> Self {
        VmLimits::default().into()
    }
}

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

pub struct Runtime {
    providers: ProviderRegistry,
    limits: RunLimits,
}

impl Runtime {
    pub fn new(providers: ProviderRegistry, limits: RunLimits) -> Self {
        Self { providers, limits }
    }

    pub fn run(
        &self,
        package: &CompiledPackage,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<ExecutionReport, RuntimeError> {
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

impl Default for Runtime {
    fn default() -> Self {
        Self::new(ProviderRegistry::default(), RunLimits::default())
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError(pub String);

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

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RuntimeError {}

#[cfg(test)]
mod tests {
    use super::*;

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
}
