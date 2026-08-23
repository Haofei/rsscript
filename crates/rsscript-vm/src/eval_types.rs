use crate::diagnostic::Diagnostic;
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

pub use rsscript_abi_model::{
    ExternalImport, ExternalSymbol, FunctionSignature, SignatureHash, WireCallTypeTable, WireType,
    WireValue,
};
pub use rsscript_provider_api::{
    AsyncProviderCallContext, AsyncWireInterpreterFn, AsyncWireMutationInterpreterFn,
    BlockingBehavior, CancellationBehavior, HostCallContext, ProviderCallContext, ProviderCallMode,
    ProviderCallTrace, ProviderCallable, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, ProviderInvocationContract,
    ProviderLoadError, ProviderResource, ProviderResourceRegistry, ProviderResourceTable,
    ProviderTraceSink, ResolvedProviderFunction, ResourceCleanupContract, ResourceHandle,
    WireInterpreterFn, WireMutationInterpreterFn, WireMutationProviderFuture, WireMutationResult,
    WireProviderFuture,
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

/// A wire Provider future boundary that turns an unwind from an in-process host
/// callback into the same structured error path as other Provider failures.
///
/// This protects the reference VM when the host uses unwind panics. It is not
/// a process-isolation guarantee: abort panics and native faults still require
/// the isolated runner boundary. The VM converts its completed canonical value
/// only after this boundary.
struct PanicContainedWireProviderFuture {
    inner: WireProviderFuture,
}

/// The mutation equivalent of [`PanicContainedWireProviderFuture`].
struct PanicContainedWireMutationProviderFuture {
    inner: WireMutationProviderFuture,
}

impl PanicContainedWireProviderFuture {
    fn new(inner: WireProviderFuture) -> Self {
        Self { inner }
    }
}

impl Future for PanicContainedWireProviderFuture {
    type Output = Result<WireValue, ProviderError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(context))) {
            Ok(result) => result,
            Err(_) => Poll::Ready(Err(provider_panic_error())),
        }
    }
}

impl PanicContainedWireMutationProviderFuture {
    fn new(inner: WireMutationProviderFuture) -> Self {
        Self { inner }
    }
}

impl Future for PanicContainedWireMutationProviderFuture {
    type Output = Result<WireMutationResult, ProviderError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(context))) {
            Ok(result) => result,
            Err(_) => Poll::Ready(Err(provider_panic_error())),
        }
    }
}

fn provider_panic_error() -> ProviderError {
    ProviderError::internal("provider callable panicked")
}

/// Enforce the per-call payload ceiling at the runtime boundary, not merely in
/// individual Provider implementations. Providers may use the remaining
/// budgets to tune their work, but they cannot bypass the host's bound by
/// forgetting to check it themselves.
fn check_payload_budget(
    bytes: usize,
    limit: Option<usize>,
    direction: &str,
) -> Result<(), ProviderError> {
    if limit.is_some_and(|limit| bytes > limit) {
        return Err(ProviderError::resource_exhausted(format!(
            "provider {direction} payload exceeds remaining budget"
        )));
    }
    Ok(())
}

/// A shared permit held for the full lifetime of a non-reentrant Provider
/// invocation. Dropping a suspended async future releases its permit, so a
/// cancelled task cannot permanently lock out a Provider function.
struct NonReentrantCallPermit {
    active: Arc<AtomicBool>,
}

impl Drop for NonReentrantCallPermit {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

/// A linked provider callable. Registry resolution attaches the complete
/// provider contract so invocation cannot silently discard descriptor metadata.
#[derive(Clone)]
pub struct ExternalFunction {
    callable: ProviderCallable,
    contract: Option<ProviderInvocationContract>,
    host_context: Arc<HostCallContext>,
    active_non_reentrant_call: Arc<AtomicBool>,
}

impl ExternalFunction {
    pub fn contract(&self) -> Option<&ProviderInvocationContract> {
        self.contract.as_ref()
    }

    /// The descriptor-scoped type table used by the VM's direct wire boundary.
    /// Only linked Provider functions expose it, so execution cannot invent
    /// record, variant, or resource identities from display strings.
    pub(crate) fn wire_types(&self) -> Result<WireCallTypeTable, ProviderError> {
        let contract = self.contract().ok_or_else(|| {
            ProviderError::unavailable("Provider function must be linked before execution")
        })?;
        wire_type_table(
            &contract.descriptor.signature,
            &contract.record_layouts,
            &contract.variant_layouts,
        )
    }

    pub fn host_context(&self) -> &HostCallContext {
        &self.host_context
    }

    pub(crate) fn host_context_arc(&self) -> Arc<HostCallContext> {
        Arc::clone(&self.host_context)
    }

    pub fn call_mode(&self) -> ProviderCallMode {
        self.callable.call_mode()
    }

    /// Whether this linked function can use the canonical asynchronous wire
    /// dispatcher. The register VM keeps the scheduler boundary explicit, but
    /// the Provider callable itself receives and returns only `WireValue`.
    pub(crate) const fn is_wire_async(&self) -> bool {
        matches!(self.callable, ProviderCallable::WireAsync(_))
    }

    /// Whether this linked function uses the canonical asynchronous mutation
    /// dispatcher with explicit canonical write-back values.
    pub(crate) const fn is_wire_async_mut(&self) -> bool {
        matches!(self.callable, ProviderCallable::WireAsyncMut(_))
    }

    fn acquire_non_reentrant_permit(
        &self,
    ) -> Result<Option<NonReentrantCallPermit>, ProviderError> {
        if self
            .contract()
            .is_none_or(|contract| contract.descriptor.reentrant)
        {
            return Ok(None);
        }
        self.active_non_reentrant_call
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                ProviderError::unavailable(
                    "non-reentrant provider function already has an active call",
                )
            })?;
        Ok(Some(NonReentrantCallPermit {
            active: Arc::clone(&self.active_non_reentrant_call),
        }))
    }

    /// Invoke a descriptor-linked synchronous wire Provider without routing
    /// the Provider call itself through `NativeValue`. The register VM uses
    /// this for non-`mut` external calls; legacy callables and mutation
    /// envelopes retain their compatibility dispatcher until their VM value
    /// representation is migrated.
    pub(crate) fn call_wire_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<WireValue>,
    ) -> Result<WireValue, ProviderError> {
        let _non_reentrant_permit = self.acquire_non_reentrant_permit()?;
        let contract = self.contract().ok_or_else(|| {
            ProviderError::unavailable("wire provider function requires a linked descriptor")
        })?;
        let ProviderCallable::WireSync(callable) = &self.callable else {
            return Err(ProviderError::unavailable(
                "provider function does not use the synchronous wire dispatcher",
            ));
        };
        context.provider_id.clone_from(&contract.provider_id);
        context
            .provider_version
            .clone_from(&contract.provider_version);
        context.symbol = contract.descriptor.symbol.as_str().to_string();
        let request_bytes = args
            .iter()
            .map(WireValue::estimated_payload_bytes)
            .sum::<usize>();
        let started = Instant::now();
        let result = (|| {
            context.check_cancelled()?;
            check_payload_budget(request_bytes, context.remaining_byte_budget, "request")?;
            if contract.descriptor.blocking == BlockingBehavior::MayBlock
                && !context.blocking_allowed
            {
                return Err(ProviderError::unavailable(format!(
                    "blocking provider function `{}` requires a blocking execution lane",
                    contract.descriptor.symbol
                )));
            }
            if contract.descriptor.call_mode != ProviderCallMode::Sync {
                return Err(ProviderError::unavailable(
                    "synchronous wire dispatcher received an async Provider descriptor",
                ));
            }
            let resources_before = context
                .resources
                .as_deref()
                .map(ProviderResourceTable::created);
            let result = catch_unwind(AssertUnwindSafe(|| {
                callable.call_with_context(context, args)
            }))
            .unwrap_or_else(|_| Err(provider_panic_error()));
            if matches!(
                contract.descriptor.cancellation,
                CancellationBehavior::Cooperative | CancellationBehavior::AbortSafe
            ) {
                context.check_cancelled()?;
            }
            if result.is_ok()
                && contract.descriptor.resource_cleanup
                    == ResourceCleanupContract::RuntimeRegistered
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
            let value = result?;
            let response_bytes = value.estimated_payload_bytes();
            check_payload_budget(response_bytes, context.remaining_byte_budget, "response")?;
            check_payload_budget(
                response_bytes,
                context.remaining_output_budget,
                "response output",
            )?;
            Ok(value)
        })();
        let response_bytes = match &result {
            Ok(value) => value.estimated_payload_bytes(),
            Err(error) => error.message.len().saturating_add(
                error
                    .details
                    .as_ref()
                    .map_or(0, WireValue::estimated_payload_bytes),
            ),
        };
        if let Some(trace) = context.trace {
            trace.record(ProviderCallTrace {
                call_id: context.call_id,
                provider_id: context.provider_id.clone(),
                provider_version: context.provider_version.clone(),
                symbol: context.symbol.clone(),
                request_bytes,
                response_bytes,
                elapsed: started.elapsed(),
                result: result.as_ref().map(|_| ()).map_err(|error| error.code),
            });
        }
        result
    }

    /// Invoke a descriptor-linked synchronous wire Provider with explicit
    /// mutation write-back values. The Provider ABI stays canonical; only the
    /// old register VM decodes the checked result after this call returns.
    pub(crate) fn call_wire_mut_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<WireValue>,
    ) -> Result<WireMutationResult, ProviderError> {
        let _non_reentrant_permit = self.acquire_non_reentrant_permit()?;
        let contract = self.contract().ok_or_else(|| {
            ProviderError::unavailable("wire provider function requires a linked descriptor")
        })?;
        let ProviderCallable::WireSyncMut(callable) = &self.callable else {
            return Err(ProviderError::unavailable(
                "provider function does not use the synchronous wire mutation dispatcher",
            ));
        };
        context.provider_id.clone_from(&contract.provider_id);
        context
            .provider_version
            .clone_from(&contract.provider_version);
        context.symbol = contract.descriptor.symbol.as_str().to_string();
        let request_bytes = args
            .iter()
            .map(WireValue::estimated_payload_bytes)
            .sum::<usize>();
        let started = Instant::now();
        let result = (|| {
            context.check_cancelled()?;
            check_payload_budget(request_bytes, context.remaining_byte_budget, "request")?;
            if contract.descriptor.blocking == BlockingBehavior::MayBlock
                && !context.blocking_allowed
            {
                return Err(ProviderError::unavailable(format!(
                    "blocking provider function `{}` requires a blocking execution lane",
                    contract.descriptor.symbol
                )));
            }
            if contract.descriptor.call_mode != ProviderCallMode::Sync {
                return Err(ProviderError::unavailable(
                    "synchronous wire mutation dispatcher received an async Provider descriptor",
                ));
            }
            let expected_mutations = contract
                .descriptor
                .signature
                .parameters
                .iter()
                .filter(|parameter| parameter.effect == rsscript_abi_model::DataEffect::Mut)
                .count();
            if expected_mutations == 0 {
                return Err(ProviderError::invalid_argument(
                    "wire mutation callable is linked to a signature without mut parameters",
                ));
            }
            let resources_before = context
                .resources
                .as_deref()
                .map(ProviderResourceTable::created);
            let result = catch_unwind(AssertUnwindSafe(|| {
                callable.call_with_context(context, args)
            }))
            .unwrap_or_else(|_| Err(provider_panic_error()));
            if matches!(
                contract.descriptor.cancellation,
                CancellationBehavior::Cooperative | CancellationBehavior::AbortSafe
            ) {
                context.check_cancelled()?;
            }
            if result.is_ok()
                && contract.descriptor.resource_cleanup
                    == ResourceCleanupContract::RuntimeRegistered
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
            let value = result?;
            if value.mutated.len() != expected_mutations {
                return Err(ProviderError::invalid_argument(
                    "wire mutation result does not contain every linked mut parameter",
                ));
            }
            let response_bytes = value
                .mutated
                .iter()
                .map(WireValue::estimated_payload_bytes)
                .fold(
                    value.result.estimated_payload_bytes(),
                    usize::saturating_add,
                );
            check_payload_budget(response_bytes, context.remaining_byte_budget, "response")?;
            check_payload_budget(
                response_bytes,
                context.remaining_output_budget,
                "response output",
            )?;
            Ok(value)
        })();
        let response_bytes = match &result {
            Ok(value) => value
                .mutated
                .iter()
                .map(WireValue::estimated_payload_bytes)
                .fold(
                    value.result.estimated_payload_bytes(),
                    usize::saturating_add,
                ),
            Err(error) => error.message.len().saturating_add(
                error
                    .details
                    .as_ref()
                    .map_or(0, WireValue::estimated_payload_bytes),
            ),
        };
        if let Some(trace) = context.trace {
            trace.record(ProviderCallTrace {
                call_id: context.call_id,
                provider_id: context.provider_id.clone(),
                provider_version: context.provider_version.clone(),
                symbol: context.symbol.clone(),
                request_bytes,
                response_bytes,
                elapsed: started.elapsed(),
                result: result.as_ref().map(|_| ()).map_err(|error| error.code),
            });
        }
        result
    }

    /// Start a descriptor-linked asynchronous wire Provider without adapting
    /// its arguments or result through `NativeValue`. This is intentionally a
    /// separate scheduler entry point: the legacy async dispatcher remains
    /// available only for compatibility callables and mutation envelopes.
    #[allow(unreachable_patterns)] // Legacy Provider variants are feature-gated upstream.
    pub(crate) fn start_wire_async(
        &self,
        mut context: AsyncProviderCallContext,
        args: Vec<WireValue>,
    ) -> WireProviderFuture {
        let non_reentrant_permit = match self.acquire_non_reentrant_permit() {
            Ok(permit) => permit,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let Some(contract) = self.contract.clone() else {
            return Box::pin(async {
                Err(ProviderError::unavailable(
                    "async wire provider function requires a linked descriptor",
                ))
            });
        };
        context.provider_id.clone_from(&contract.provider_id);
        context
            .provider_version
            .clone_from(&contract.provider_version);
        context.symbol = contract.descriptor.symbol.as_str().to_string();
        let callable = match &self.callable {
            ProviderCallable::WireAsync(callable) => callable.clone(),
            _ => {
                return Box::pin(async {
                    Err(ProviderError::unavailable(
                        "non-wire async Provider cannot enter the wire async dispatcher",
                    ))
                });
            }
        };
        let request_bytes = args
            .iter()
            .map(WireValue::estimated_payload_bytes)
            .sum::<usize>();
        let trace = context.trace.clone();
        let trace_context = context.clone();
        let resources_before = context
            .resources
            .as_ref()
            .and_then(|resources| resources.snapshot().ok())
            .map(|usage| usage.created);
        let started = Instant::now();
        Box::pin(async move {
            let _non_reentrant_permit = non_reentrant_permit;
            let result = async {
                context.check_cancelled()?;
                check_payload_budget(request_bytes, context.remaining_byte_budget, "request")?;
                if contract.descriptor.call_mode != ProviderCallMode::Async {
                    return Err(ProviderError::unavailable(
                        "async wire callable has a synchronous Provider descriptor",
                    ));
                }
                if contract.descriptor.blocking == BlockingBehavior::MayBlock {
                    return Err(ProviderError::unavailable(
                        "blocking work must not run inside an async Provider future",
                    ));
                }
                let result = match catch_unwind(AssertUnwindSafe(|| callable.call(context, args))) {
                    Ok(future) => PanicContainedWireProviderFuture::new(future).await,
                    Err(_) => Err(provider_panic_error()),
                };
                if matches!(
                    contract.descriptor.cancellation,
                    CancellationBehavior::Cooperative | CancellationBehavior::AbortSafe
                ) {
                    trace_context.check_cancelled()?;
                }
                if result.is_ok()
                    && contract.descriptor.resource_cleanup
                        == ResourceCleanupContract::RuntimeRegistered
                    && resources_before
                        == trace_context
                            .resources
                            .as_ref()
                            .and_then(|resources| resources.snapshot().ok())
                            .map(|usage| usage.created)
                {
                    return Err(ProviderError::internal(
                        "runtime-registered provider call returned without registering a resource",
                    ));
                }
                let value = result?;
                let response_bytes = value.estimated_payload_bytes();
                check_payload_budget(
                    response_bytes,
                    trace_context.remaining_byte_budget,
                    "response",
                )?;
                check_payload_budget(
                    response_bytes,
                    trace_context.remaining_output_budget,
                    "response output",
                )?;
                Ok(value)
            }
            .await;
            let response_bytes = match &result {
                Ok(value) => value.estimated_payload_bytes(),
                Err(error) => error.message.len().saturating_add(
                    error
                        .details
                        .as_ref()
                        .map_or(0, WireValue::estimated_payload_bytes),
                ),
            };
            if let Some(trace) = trace {
                trace.record(ProviderCallTrace {
                    call_id: trace_context.call_id,
                    provider_id: trace_context.provider_id.clone(),
                    provider_version: trace_context.provider_version.clone(),
                    symbol: trace_context.symbol.clone(),
                    request_bytes,
                    response_bytes,
                    elapsed: started.elapsed(),
                    result: result.as_ref().map(|_| ()).map_err(|error| error.code),
                });
            }
            result
        })
    }

    /// Start an asynchronous canonical wire mutation Provider. The scheduler
    /// receives explicit wire write-backs and only the register VM's legacy
    /// boundary later materializes its temporary mutation envelope.
    #[allow(unreachable_patterns)] // Legacy Provider variants are feature-gated upstream.
    pub(crate) fn start_wire_mut_async(
        &self,
        mut context: AsyncProviderCallContext,
        args: Vec<WireValue>,
    ) -> WireMutationProviderFuture {
        let non_reentrant_permit = match self.acquire_non_reentrant_permit() {
            Ok(permit) => permit,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let Some(contract) = self.contract.clone() else {
            return Box::pin(async {
                Err(ProviderError::unavailable(
                    "async wire mutation provider function requires a linked descriptor",
                ))
            });
        };
        context.provider_id.clone_from(&contract.provider_id);
        context
            .provider_version
            .clone_from(&contract.provider_version);
        context.symbol = contract.descriptor.symbol.as_str().to_string();
        let callable = match &self.callable {
            ProviderCallable::WireAsyncMut(callable) => callable.clone(),
            _ => {
                return Box::pin(async {
                    Err(ProviderError::unavailable(
                        "non-wire-mutation async Provider cannot enter the wire mutation dispatcher",
                    ))
                });
            }
        };
        let expected_mutations = contract
            .descriptor
            .signature
            .parameters
            .iter()
            .filter(|parameter| parameter.effect == rsscript_abi_model::DataEffect::Mut)
            .count();
        if expected_mutations == 0 {
            return Box::pin(async {
                Err(ProviderError::invalid_argument(
                    "wire mutation callable is linked to a signature without mut parameters",
                ))
            });
        }
        let request_bytes = args
            .iter()
            .map(WireValue::estimated_payload_bytes)
            .sum::<usize>();
        let trace = context.trace.clone();
        let trace_context = context.clone();
        let resources_before = context
            .resources
            .as_ref()
            .and_then(|resources| resources.snapshot().ok())
            .map(|usage| usage.created);
        let started = Instant::now();
        Box::pin(async move {
            let _non_reentrant_permit = non_reentrant_permit;
            let result = async {
                context.check_cancelled()?;
                check_payload_budget(request_bytes, context.remaining_byte_budget, "request")?;
                if contract.descriptor.call_mode != ProviderCallMode::Async {
                    return Err(ProviderError::unavailable(
                        "async wire mutation callable has a synchronous Provider descriptor",
                    ));
                }
                if contract.descriptor.blocking == BlockingBehavior::MayBlock {
                    return Err(ProviderError::unavailable(
                        "blocking work must not run inside an async Provider future",
                    ));
                }
                let result = match catch_unwind(AssertUnwindSafe(|| callable.call(context, args))) {
                    Ok(future) => PanicContainedWireMutationProviderFuture::new(future).await,
                    Err(_) => Err(provider_panic_error()),
                };
                if matches!(
                    contract.descriptor.cancellation,
                    CancellationBehavior::Cooperative | CancellationBehavior::AbortSafe
                ) {
                    trace_context.check_cancelled()?;
                }
                if result.is_ok()
                    && contract.descriptor.resource_cleanup
                        == ResourceCleanupContract::RuntimeRegistered
                    && resources_before
                        == trace_context
                            .resources
                            .as_ref()
                            .and_then(|resources| resources.snapshot().ok())
                            .map(|usage| usage.created)
                {
                    return Err(ProviderError::internal(
                        "runtime-registered provider call returned without registering a resource",
                    ));
                }
                let value = result?;
                if value.mutated.len() != expected_mutations {
                    return Err(ProviderError::invalid_argument(
                        "wire mutation result does not contain every linked mut parameter",
                    ));
                }
                let response_bytes = value
                    .mutated
                    .iter()
                    .map(WireValue::estimated_payload_bytes)
                    .fold(
                        value.result.estimated_payload_bytes(),
                        usize::saturating_add,
                    );
                check_payload_budget(
                    response_bytes,
                    trace_context.remaining_byte_budget,
                    "response",
                )?;
                check_payload_budget(
                    response_bytes,
                    trace_context.remaining_output_budget,
                    "response output",
                )?;
                Ok(value)
            }
            .await;
            let response_bytes = match &result {
                Ok(value) => value
                    .mutated
                    .iter()
                    .map(WireValue::estimated_payload_bytes)
                    .fold(
                        value.result.estimated_payload_bytes(),
                        usize::saturating_add,
                    ),
                Err(error) => error.message.len().saturating_add(
                    error
                        .details
                        .as_ref()
                        .map_or(0, WireValue::estimated_payload_bytes),
                ),
            };
            if let Some(trace) = trace {
                trace.record(ProviderCallTrace {
                    call_id: trace_context.call_id,
                    provider_id: trace_context.provider_id.clone(),
                    provider_version: trace_context.provider_version.clone(),
                    symbol: trace_context.symbol.clone(),
                    request_bytes,
                    response_bytes,
                    elapsed: started.elapsed(),
                    result: result.as_ref().map(|_| ()).map_err(|error| error.code),
                });
            }
            result
        })
    }

    fn from_resolved(
        function: ResolvedProviderFunction<ProviderCallable>,
        host_context: Arc<HostCallContext>,
    ) -> Self {
        Self {
            callable: function.callable,
            contract: Some(ProviderInvocationContract {
                provider_id: function.provider_id,
                provider_version: function.provider_version,
                record_layouts: function.record_layouts,
                variant_layouts: function.variant_layouts,
                descriptor: function.descriptor,
            }),
            host_context,
            active_non_reentrant_call: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn wire_type_table(
    signature: &FunctionSignature,
    record_layouts: &[rsscript_abi_model::WireRecordLayout],
    variant_layouts: &[rsscript_abi_model::WireVariantLayout],
) -> Result<WireCallTypeTable, ProviderError> {
    WireCallTypeTable::for_signature(signature)
        .and_then(|table| table.with_record_layouts(record_layouts.to_vec()))
        .and_then(|table| table.with_variant_layouts(variant_layouts.to_vec()))
        .map_err(|error| {
            ProviderError::internal(format!(
                "linked provider signature cannot form a wire type table: {error}"
            ))
        })
}

impl From<AsyncWireInterpreterFn> for ExternalFunction {
    fn from(callable: AsyncWireInterpreterFn) -> Self {
        Self {
            callable: callable.into(),
            contract: None,
            host_context: Arc::new(HostCallContext::default()),
            active_non_reentrant_call: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Runtime-owned symbol table for functions supplied by package providers.
/// Compilation and lowering only record the symbol name; provider selection is
/// deliberately deferred until execution.
pub struct ExternalFunctionRegistry {
    registry: rsscript_provider_api::ProviderRegistry<ProviderCallable>,
    host_context: Arc<HostCallContext>,
}

impl ExternalFunctionRegistry {
    pub fn new() -> Self {
        Self {
            registry: rsscript_provider_api::ProviderRegistry::new(
                rsscript_abi_model::RUNTIME_ABI_VERSION,
            ),
            host_context: Arc::new(HostCallContext::default()),
        }
    }

    pub fn set_host_call_context(&mut self, host_context: HostCallContext) {
        self.host_context = Arc::new(host_context);
    }

    pub fn register_provider<T: Into<ProviderCallable>>(
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
            .collect::<BTreeMap<_, _>>();
        for declared in &descriptor.functions {
            if let Some(implementation) = functions.get(&declared.symbol)
                && implementation.callable.call_mode() != declared.call_mode
            {
                return Err(ProviderLoadError::CallModeMismatch(declared.symbol.clone()));
            }
        }
        self.registry.register_provider(descriptor, functions)
    }

    pub fn resolve(
        &self,
        import: &ExternalImport,
    ) -> Result<&ResolvedProviderFunction<ProviderCallable>, ProviderLoadError> {
        self.registry.resolve(import)
    }

    pub fn into_bindings(self) -> impl Iterator<Item = (String, ExternalFunction)> {
        let host_context = self.host_context;
        self.registry
            .into_resolved_functions()
            .map(move |(symbol, function)| {
                (
                    symbol.as_str().to_string(),
                    ExternalFunction::from_resolved(function, Arc::clone(&host_context)),
                )
            })
    }

    pub fn bindings(&self) -> impl Iterator<Item = (String, ExternalFunction)> + '_ {
        let host_context = Arc::clone(&self.host_context);
        self.registry
            .resolved_functions()
            .map(move |(symbol, function)| {
                (
                    symbol.as_str().to_string(),
                    ExternalFunction::from_resolved(function.clone(), Arc::clone(&host_context)),
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
    pub stdout: String,
    pub stderr: String,
    pub provider_call_traces: Vec<ProviderCallTrace>,
}

/// VM result that preserves audit evidence even when execution terminates with
/// an error. Public embedding façades should prefer this over a bare
/// `Result<EvalOutput, EvalError>` when reporting bounded execution.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalExecutionReport {
    pub usage: ExecutionUsage,
    pub value: Option<String>,
    pub display_value: Option<String>,
    /// Canonical result value for the declared program result type when the
    /// legacy v1 Artifact carries enough layout information to derive it.
    /// Absent values are deliberately not replaced with dynamic identifiers.
    pub wire_value: Option<WireValue>,
    pub stdout: String,
    pub stderr: String,
    pub provider_call_traces: Vec<ProviderCallTrace>,
    /// Actual engine evidence for this run. A native request may still report
    /// zero native entries when eligibility, profitability, or armed limits
    /// kept all work on the interpreter.
    pub engine: ExecutionEngineTelemetry,
    pub failure: Option<EvalError>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionEngineTelemetry {
    #[default]
    Interpreter,
    Native {
        considered: u64,
        compiled: u64,
        native_calls: u64,
        native_bails: u64,
        osr_entries: u64,
        /// Weighted native-lowerable work that remained interpreted.
        interpreted_native_work: u64,
        /// Dynamic normal-boundary observations grouped by stable reason.
        native_barrier_counts: std::collections::BTreeMap<String, u64>,
        /// Machine code currently resident in the JIT modules. The current
        /// compile-once-publish policy makes this equal to published code.
        resident_code_bytes: u64,
        /// Machine code reachable through a VM dispatch cache.
        published_code_bytes: u64,
        /// Resident machine code rejected after finalization. This remains zero
        /// under compile-once-publish and guards future admission changes.
        rejected_resident_bytes: u64,
        /// Executable address space reserved by all JIT arenas for this run.
        reserved_arena_bytes: u64,
        compile_nanos: u128,
        run_nanos: u128,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionUsage {
    pub steps_consumed: u64,
    pub allocation_bytes_consumed: usize,
    pub live_memory_bytes_at_return: usize,
    pub peak_live_memory_bytes: usize,
    pub output_bytes: usize,
    pub intrinsic_calls: u64,
    pub provider_calls: u64,
    pub resources_created: u64,
    pub resources_cleaned: u64,
    pub resource_cleanup_failures: u64,
    pub resources_peak_live: usize,
    pub resources_live_at_return: usize,
    pub tasks_created: u64,
    pub tasks_completed: u64,
    pub tasks_cancelled: u64,
    pub tasks_peak_live: usize,
    pub tasks_live_at_return: usize,
}

#[derive(Debug, Clone, PartialEq)]
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
    LiveMemoryLimitExceeded,
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
