use crate::diagnostic::Diagnostic;
use std::collections::BTreeMap;

pub use rsscript_abi_model::{ExternalImport, ExternalSymbol, FunctionSignature, SignatureHash};
pub use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallContext,
    ProviderCallMode, ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderErrorMapping,
    ProviderFunction, ProviderFunctionDescriptor, ProviderInvocationContract, ProviderLoadError,
    ProviderResource, ProviderResourceTable, ResolvedProviderFunction, ResourceCleanupContract,
    ResourceHandle,
};

/// A linked provider callable. Registry resolution attaches the complete
/// provider contract so invocation cannot silently discard descriptor metadata.
#[derive(Clone)]
pub struct ExternalFunction {
    callable: NativeInterpreterFn,
    contract: Option<ProviderInvocationContract>,
}

impl ExternalFunction {
    pub fn from_fn(function: rsscript_provider_api::NativeHostFn) -> Self {
        NativeInterpreterFn::from_fn(function).into()
    }

    pub fn new(
        function: impl Fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        NativeInterpreterFn::new(function).into()
    }

    pub fn new_contextual(
        function: impl for<'a> Fn(
            &mut ProviderCallContext<'a>,
            Vec<NativeValue>,
        ) -> Result<NativeValue, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        NativeInterpreterFn::new_contextual(function).into()
    }

    pub fn contract(&self) -> Option<&ProviderInvocationContract> {
        self.contract.as_ref()
    }

    pub fn call_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<NativeValue>,
    ) -> Result<NativeValue, ProviderError> {
        context.check_cancelled()?;
        if let Some(contract) = self.contract() {
            if contract.descriptor.blocking == BlockingBehavior::MayBlock
                && !context.blocking_allowed
            {
                return Err(ProviderError::unavailable(format!(
                    "blocking provider function `{}` requires a blocking execution lane",
                    contract.descriptor.symbol
                )));
            }
            if contract.descriptor.call_mode == ProviderCallMode::Async && !context.async_allowed {
                return Err(ProviderError::unavailable(format!(
                    "async provider function `{}` requires an async execution lane",
                    contract.descriptor.symbol
                )));
            }
        }
        let resources_before = context
            .resources
            .as_deref()
            .map(ProviderResourceTable::created);
        let result = self.callable.call_with_context(context, args);
        if self.contract().is_some_and(|contract| {
            matches!(
                contract.descriptor.cancellation,
                CancellationBehavior::Cooperative | CancellationBehavior::AbortSafe
            )
        }) {
            context.check_cancelled()?;
        }
        if result.is_ok()
            && self.contract().is_some_and(|contract| {
                contract.descriptor.resource_cleanup == ResourceCleanupContract::RuntimeRegistered
            })
            && resources_before
                == context
                    .resources
                    .as_deref()
                    .map(ProviderResourceTable::created)
        {
            return Err(ProviderError::internal(
                "runtime-registered provider call returned without registering a resource",
            ));
        }
        result
    }

    fn from_resolved(function: ResolvedProviderFunction<NativeInterpreterFn>) -> Self {
        Self {
            callable: function.callable,
            contract: Some(ProviderInvocationContract {
                provider_id: function.provider_id,
                provider_version: function.provider_version,
                descriptor: function.descriptor,
            }),
        }
    }
}

impl From<rsscript_provider_api::NativeHostFn> for ExternalFunction {
    fn from(function: rsscript_provider_api::NativeHostFn) -> Self {
        Self::from_fn(function)
    }
}

impl From<NativeInterpreterFn> for ExternalFunction {
    fn from(callable: NativeInterpreterFn) -> Self {
        Self {
            callable,
            contract: None,
        }
    }
}

impl From<ExternalFunction> for NativeInterpreterFn {
    fn from(function: ExternalFunction) -> Self {
        function.callable
    }
}

/// Runtime-owned symbol table for functions supplied by package providers.
/// Compilation and lowering only record the symbol name; provider selection is
/// deliberately deferred until execution.
pub struct ExternalFunctionRegistry {
    registry: rsscript_provider_api::ProviderRegistry<NativeInterpreterFn>,
}

impl ExternalFunctionRegistry {
    pub fn new() -> Self {
        Self {
            registry: rsscript_provider_api::ProviderRegistry::new(
                rsscript_abi_model::RUNTIME_ABI_VERSION,
            ),
        }
    }

    pub fn register_provider<T: Into<NativeInterpreterFn>>(
        &mut self,
        descriptor: &ProviderDescriptor,
        functions: BTreeMap<ExternalSymbol, ProviderFunction<T>>,
    ) -> Result<(), ProviderLoadError> {
        let functions = functions
            .into_iter()
            .map(|(symbol, function)| {
                (
                    symbol,
                    ProviderFunction {
                        signature: function.signature,
                        callable: function.callable.into(),
                    },
                )
            })
            .collect();
        self.registry.register_provider(descriptor, functions)
    }

    pub fn resolve(
        &self,
        import: &ExternalImport,
    ) -> Result<&ResolvedProviderFunction<NativeInterpreterFn>, ProviderLoadError> {
        self.registry.resolve(import)
    }

    pub fn into_bindings(self) -> impl Iterator<Item = (String, ExternalFunction)> {
        self.registry
            .into_resolved_functions()
            .map(|(symbol, function)| {
                (
                    symbol.as_str().to_string(),
                    ExternalFunction::from_resolved(function),
                )
            })
    }

    pub fn bindings(&self) -> impl Iterator<Item = (String, ExternalFunction)> + '_ {
        self.registry
            .resolved_functions()
            .map(|(symbol, function)| {
                (
                    symbol.as_str().to_string(),
                    ExternalFunction::from_resolved(function.clone()),
                )
            })
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
    AllocationBudgetExceeded,
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

#[cfg(test)]
mod provider_contract_tests {
    use super::*;

    fn registered_function(
        blocking: BlockingBehavior,
        cleanup: ResourceCleanupContract,
    ) -> ExternalFunction {
        let symbol = ExternalSymbol::new("host.test.run").unwrap();
        let signature = FunctionSignature {
            parameters: vec![],
            result: "Unit".into(),
            asynchronous: false,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.provider".to_string(),
            provider_version: "1.0.0".to_string(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "run".to_string(),
                call_mode: ProviderCallMode::Sync,
                blocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: cleanup,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        };
        let mut registry = ExternalFunctionRegistry::new();
        registry
            .register_provider(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature,
                        callable: NativeInterpreterFn::new(|_| Ok(NativeValue::Unit)),
                    },
                )]),
            )
            .unwrap();
        registry.into_bindings().next().unwrap().1
    }

    #[test]
    fn linked_function_enforces_blocking_lane_and_retains_identity() {
        let function =
            registered_function(BlockingBehavior::MayBlock, ResourceCleanupContract::None);
        assert_eq!(function.contract().unwrap().provider_id, "test.provider");
        let error = function
            .call_with_context(&mut ProviderCallContext::default(), vec![])
            .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::Unavailable);

        let mut context = ProviderCallContext {
            blocking_allowed: true,
            ..ProviderCallContext::default()
        };
        assert_eq!(
            function.call_with_context(&mut context, vec![]).unwrap(),
            NativeValue::Unit
        );
    }

    #[test]
    fn runtime_registered_cleanup_contract_is_enforced() {
        let function = registered_function(
            BlockingBehavior::NonBlocking,
            ResourceCleanupContract::RuntimeRegistered,
        );
        let mut resources = ProviderResourceTable::new(Some(4));
        let mut context = ProviderCallContext {
            resources: Some(&mut resources),
            ..ProviderCallContext::default()
        };
        let error = function
            .call_with_context(&mut context, vec![])
            .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::Internal);
        assert!(error.message.contains("without registering a resource"));
    }
}
