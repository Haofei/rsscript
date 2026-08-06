use crate::diagnostic::Diagnostic;
use std::collections::BTreeMap;

pub use rsscript_abi_model::{ExternalImport, ExternalSymbol, FunctionSignature, SignatureHash};
pub use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallContext,
    ProviderCallMode, ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderErrorMapping,
    ProviderFunction, ProviderFunctionDescriptor, ProviderLoadError, ProviderResource,
    ProviderResourceTable, ResourceCleanupContract, ResourceHandle,
};
pub type ExternalFunction = NativeInterpreterFn;

/// Runtime-owned symbol table for functions supplied by package providers.
/// Compilation and lowering only record the symbol name; provider selection is
/// deliberately deferred until execution.
pub struct ExternalFunctionRegistry {
    registry: rsscript_provider_api::ProviderRegistry<ExternalFunction>,
}

impl ExternalFunctionRegistry {
    pub fn new() -> Self {
        Self {
            registry: rsscript_provider_api::ProviderRegistry::new(
                rsscript_abi_model::RUNTIME_ABI_VERSION,
            ),
        }
    }

    pub fn register_provider(
        &mut self,
        descriptor: &ProviderDescriptor,
        functions: BTreeMap<ExternalSymbol, ProviderFunction<ExternalFunction>>,
    ) -> Result<(), ProviderLoadError> {
        self.registry.register_provider(descriptor, functions)
    }

    pub fn resolve(&self, import: &ExternalImport) -> Result<&ExternalFunction, ProviderLoadError> {
        self.registry
            .resolve(import)
            .map(|function| &function.callable)
    }

    pub fn into_bindings(self) -> impl Iterator<Item = (String, ExternalFunction)> {
        self.registry
            .into_functions()
            .map(|(symbol, function)| (symbol.as_str().to_string(), function))
    }

    pub fn bindings(&self) -> impl Iterator<Item = (String, ExternalFunction)> + '_ {
        self.registry
            .functions()
            .map(|(symbol, function)| (symbol.as_str().to_string(), function.clone()))
    }
}

impl Default for ExternalFunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalOutput {
    pub usage: ExecutionUsage,
    pub value: String,
    pub display_value: String,
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutionUsage {
    pub steps_consumed: u64,
    pub allocation_bytes_consumed: usize,
    pub output_bytes: usize,
    pub intrinsic_calls: u64,
    pub provider_calls: u64,
    pub resources_created: u64,
    pub resources_cleaned: u64,
    pub resources_live_at_return: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    Diagnostics(Vec<Diagnostic>),
    Execution {
        kind: ExecutionFailureKind,
        message: String,
    },
    Provider(ProviderError),
    Runtime(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionFailureKind {
    Cancelled,
    DeadlineExceeded,
    StepBudgetExceeded,
    MemoryLimitExceeded,
    OutputLimitExceeded,
    IntrinsicBudgetExceeded,
    ProviderBudgetExceeded,
    ResourceLimitExceeded,
}

impl EvalError {
    pub fn execution(kind: ExecutionFailureKind, message: impl Into<String>) -> Self {
        Self::Execution {
            kind,
            message: message.into(),
        }
    }

    pub fn into_message(self) -> String {
        match self {
            Self::Diagnostics(diagnostics) => {
                format!(
                    "execution rejected with {} diagnostic(s)",
                    diagnostics.len()
                )
            }
            Self::Execution { message, .. } | Self::Runtime(message) => message,
            Self::Provider(error) => error.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageBucket {
    pub all: Vec<String>,
    pub supported: Vec<String>,
    pub missing: Vec<String>,
}

impl CoverageBucket {
    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn supported_count(&self) -> usize {
        self.supported.len()
    }

    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }
}
