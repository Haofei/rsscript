use crate::diagnostic::Diagnostic;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use rsscript_abi_model::{ExternalImport, ExternalSymbol, FunctionSignature, SignatureHash};
pub use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderAuthority,
    ProviderCallContext, ProviderCallMode, ProviderCallTrace, ProviderDescriptor, ProviderError,
    ProviderErrorCode, ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor,
    ProviderInvocationContract, ProviderLoadError, ProviderResource, ProviderResourceTable,
    ProviderTraceSink, ResolvedProviderFunction, ResourceCleanupContract, ResourceHandle,
};

#[derive(Default)]
pub(crate) struct ProviderTraceCollector {
    traces: Mutex<Vec<ProviderCallTrace>>,
}

impl ProviderTraceSink for ProviderTraceCollector {
    fn record(&self, trace: ProviderCallTrace) {
        self.traces
            .lock()
            .expect("provider trace mutex poisoned")
            .push(trace);
    }
}

impl ProviderTraceCollector {
    pub(crate) fn snapshot(&self) -> Vec<ProviderCallTrace> {
        self.traces
            .lock()
            .expect("provider trace mutex poisoned")
            .clone()
    }
}

/// A linked provider callable. Registry resolution attaches the complete
/// provider contract so invocation cannot silently discard descriptor metadata.
#[derive(Clone)]
pub struct ExternalFunction {
    callable: NativeInterpreterFn,
    contract: Option<ProviderInvocationContract>,
    authority: Arc<ProviderAuthority>,
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

    pub fn authority(&self) -> &ProviderAuthority {
        &self.authority
    }

    pub fn call_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<NativeValue>,
    ) -> Result<NativeValue, ProviderError> {
        if let Some(contract) = self.contract() {
            context.provider_id.clone_from(&contract.provider_id);
            context
                .provider_version
                .clone_from(&contract.provider_version);
            context.symbol = contract.descriptor.symbol.as_str().to_string();
        }
        let started = Instant::now();
        let result = (|| {
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
                if contract.descriptor.call_mode == ProviderCallMode::Async
                    && !context.async_allowed
                {
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
                    contract.descriptor.resource_cleanup
                        == ResourceCleanupContract::RuntimeRegistered
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
        })();
        if let Some(trace) = context.trace {
            trace.record(ProviderCallTrace {
                call_id: context.call_id,
                provider_id: context.provider_id.clone(),
                provider_version: context.provider_version.clone(),
                symbol: context.symbol.clone(),
                elapsed: started.elapsed(),
                result: result.as_ref().map(|_| ()).map_err(|error| error.code),
            });
        }
        result
    }

    fn from_resolved(
        function: ResolvedProviderFunction<NativeInterpreterFn>,
        authority: Arc<ProviderAuthority>,
    ) -> Self {
        Self {
            callable: function.callable,
            contract: Some(ProviderInvocationContract {
                provider_id: function.provider_id,
                provider_version: function.provider_version,
                descriptor: function.descriptor,
            }),
            authority,
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
            authority: Arc::new(ProviderAuthority::default()),
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
    authority: Arc<ProviderAuthority>,
}

impl ExternalFunctionRegistry {
    pub fn new() -> Self {
        Self {
            registry: rsscript_provider_api::ProviderRegistry::new(
                rsscript_abi_model::RUNTIME_ABI_VERSION,
            ),
            authority: Arc::new(ProviderAuthority::default()),
        }
    }

    pub fn set_authority(&mut self, authority: ProviderAuthority) {
        self.authority = Arc::new(authority);
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
        let authority = self.authority;
        self.registry
            .into_resolved_functions()
            .map(move |(symbol, function)| {
                (
                    symbol.as_str().to_string(),
                    ExternalFunction::from_resolved(function, Arc::clone(&authority)),
                )
            })
    }

    pub fn bindings(&self) -> impl Iterator<Item = (String, ExternalFunction)> + '_ {
        let authority = Arc::clone(&self.authority);
        self.registry
            .resolved_functions()
            .map(move |(symbol, function)| {
                (
                    symbol.as_str().to_string(),
                    ExternalFunction::from_resolved(function.clone(), Arc::clone(&authority)),
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
    pub provider_call_traces: Vec<ProviderCallTrace>,
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
