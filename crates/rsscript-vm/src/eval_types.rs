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
    AsyncInterpreterFn, AsyncProviderCallContext, AsyncWireInterpreterFn, BlockingBehavior,
    CancellationBehavior, HostCallContext, NativeInterpreterFn, NativeValue, ProviderCallContext,
    ProviderCallMode, ProviderCallTrace, ProviderCallable, ProviderDescriptor, ProviderError,
    ProviderErrorCode, ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor,
    ProviderFuture, ProviderInvocationContract, ProviderLoadError, ProviderResource,
    ProviderResourceRegistry, ProviderResourceTable, ProviderTraceSink, ResolvedProviderFunction,
    ResourceCleanupContract, ResourceHandle, WireInterpreterFn, WireProviderFuture,
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

/// A Provider future boundary that turns an unwind from an in-process host
/// callback into the same structured error path as other Provider failures.
///
/// This protects the reference VM when the host uses unwind panics. It is not
/// a process-isolation guarantee: abort panics and native faults still require
/// the isolated runner boundary.
struct PanicContainedProviderFuture {
    inner: ProviderFuture,
}

/// The wire equivalent of [`PanicContainedProviderFuture`]. The VM converts
/// its completed canonical value only after this boundary, so a Provider panic
/// cannot skip the normal structured error path merely because it is async.
struct PanicContainedWireProviderFuture {
    inner: WireProviderFuture,
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

impl PanicContainedProviderFuture {
    fn new(inner: ProviderFuture) -> Self {
        Self { inner }
    }
}

impl Future for PanicContainedProviderFuture {
    type Output = Result<NativeValue, ProviderError>;

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

/// Convert the subset with an unambiguous legacy VM representation to the
/// canonical Provider wire model. The type table is derived from the linked
/// function signature, so aggregate identities are shared with the Provider
/// without fabricating an Artifact-wide record layout. Named records use the
/// descriptor-supplied layouts; JSON and resources remain fail-closed until
/// their Artifact-wide lifecycle/extension adapters are available.
fn native_to_wire(
    value: NativeValue,
    expected: &WireType,
    types: &WireCallTypeTable,
) -> Result<WireValue, ProviderError> {
    match (value, expected) {
        (NativeValue::Unit, WireType::Unit) => Ok(WireValue::Unit),
        (NativeValue::Bool(value), WireType::Bool) => Ok(WireValue::Bool { value }),
        (NativeValue::Int(value), WireType::Int { .. }) => Ok(WireValue::Int { value }),
        (NativeValue::Float(value), WireType::Float { .. }) => Ok(WireValue::Float { value }),
        (NativeValue::String(value), WireType::String) => Ok(WireValue::String { value }),
        (NativeValue::Char(value), WireType::Char) => Ok(WireValue::Char { value }),
        (NativeValue::Bytes(value), WireType::Bytes) => Ok(WireValue::Bytes { value }),
        (NativeValue::List(values), WireType::List { element }) => {
            let element_type = type_id(types, element)?;
            let values = values
                .into_iter()
                .map(|value| native_to_wire(value, element, types))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(WireValue::List {
                element_type,
                values,
            })
        }
        (NativeValue::Map(entries), WireType::Map { key, value }) => {
            let key_type = type_id(types, key)?;
            let value_type = type_id(types, value)?;
            let entries = entries
                .into_iter()
                .map(|(entry_key, entry_value)| {
                    Ok((
                        native_to_wire(entry_key, key, types)?,
                        native_to_wire(entry_value, value, types)?,
                    ))
                })
                .collect::<Result<Vec<_>, ProviderError>>()?;
            Ok(WireValue::Map {
                key_type,
                value_type,
                entries,
            })
        }
        (NativeValue::List(values), WireType::Tuple { elements }) => {
            if values.len() != elements.len() {
                return Err(ProviderError::invalid_argument(
                    "provider tuple argument length does not match its linked signature",
                ));
            }
            Ok(WireValue::Tuple {
                values: values
                    .into_iter()
                    .zip(elements)
                    .map(|(value, element)| native_to_wire(value, element, types))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        (NativeValue::Struct { name, fields }, named @ WireType::Named { .. }) => {
            let layout = types.record_layout(named).ok_or_else(|| {
                ProviderError::unavailable(
                    "provider named record is missing from its linked interface layout",
                )
            })?;
            if !native_record_name_matches(&name, named) || fields.len() != layout.fields.len() {
                return Err(ProviderError::invalid_argument(
                    "provider named record argument does not match its linked interface layout",
                ));
            }
            let fields = layout
                .fields
                .iter()
                .map(|field| {
                    fields
                        .get(&field.name)
                        .cloned()
                        .ok_or_else(|| {
                            ProviderError::invalid_argument(
                                "provider named record argument is missing a linked field",
                            )
                        })
                        .and_then(|value| native_to_wire(value, &field.ty, types))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(WireValue::Record {
                type_id: type_id(types, named)?,
                fields,
            })
        }
        (NativeValue::Variant { name, fields }, option @ WireType::Option { value: element })
            if name == "Some" && fields.len() == 1 =>
        {
            let value = fields.get("value").ok_or_else(|| {
                ProviderError::invalid_argument("provider option value must have a `value` field")
            })?;
            Ok(WireValue::Variant {
                type_id: type_id(types, option)?,
                variant_id: WireCallTypeTable::option_some_variant(),
                payload: Some(Box::new(native_to_wire(value.clone(), element, types)?)),
            })
        }
        (NativeValue::Variant { name, fields }, option @ WireType::Option { .. })
            if name == "None" && fields.is_empty() =>
        {
            Ok(WireValue::Variant {
                type_id: type_id(types, option)?,
                variant_id: WireCallTypeTable::option_none_variant(),
                payload: None,
            })
        }
        (NativeValue::Variant { name, fields }, result @ WireType::Result { ok, .. })
            if name == "Ok" && fields.len() == 1 =>
        {
            let value = fields.get("value").ok_or_else(|| {
                ProviderError::invalid_argument("provider result value must have a `value` field")
            })?;
            Ok(WireValue::Variant {
                type_id: type_id(types, result)?,
                variant_id: WireCallTypeTable::result_ok_variant(),
                payload: Some(Box::new(native_to_wire(value.clone(), ok, types)?)),
            })
        }
        (NativeValue::Variant { name, fields }, result @ WireType::Result { error, .. })
            if name == "Err" && fields.len() == 1 =>
        {
            let value = fields.get("value").ok_or_else(|| {
                ProviderError::invalid_argument("provider result value must have a `value` field")
            })?;
            Ok(WireValue::Variant {
                type_id: type_id(types, result)?,
                variant_id: WireCallTypeTable::result_err_variant(),
                payload: Some(Box::new(native_to_wire(value.clone(), error, types)?)),
            })
        }
        (
            value,
            WireType::Qualified {
                value: expected, ..
            },
        ) => native_to_wire(value, expected, types),
        (
            _,
            WireType::Unit
            | WireType::Bool
            | WireType::Int { .. }
            | WireType::Float { .. }
            | WireType::String
            | WireType::Char
            | WireType::Bytes,
        ) => Err(ProviderError::invalid_argument(
            "provider wire argument does not match its linked scalar signature",
        )),
        _ => Err(ProviderError::unavailable(
            "provider wire value requires an Artifact type-table adapter",
        )),
    }
}

fn wire_to_native(
    value: WireValue,
    expected: &WireType,
    types: &WireCallTypeTable,
) -> Result<NativeValue, ProviderError> {
    match (value, expected) {
        (WireValue::Unit, WireType::Unit) => Ok(NativeValue::Unit),
        (WireValue::Bool { value }, WireType::Bool) => Ok(NativeValue::Bool(value)),
        (WireValue::Int { value }, WireType::Int { .. }) => Ok(NativeValue::Int(value)),
        (WireValue::Float { value }, WireType::Float { .. }) => Ok(NativeValue::Float(value)),
        (WireValue::String { value }, WireType::String) => Ok(NativeValue::String(value)),
        (WireValue::Char { value }, WireType::Char) => Ok(NativeValue::Char(value)),
        (WireValue::Bytes { value }, WireType::Bytes) => Ok(NativeValue::Bytes(value)),
        (
            WireValue::List {
                element_type,
                values,
            },
            WireType::List { element },
        ) if element_type == type_id(types, element)? => values
            .into_iter()
            .map(|value| wire_to_native(value, element, types))
            .collect::<Result<Vec<_>, _>>()
            .map(NativeValue::List),
        (
            WireValue::Map {
                key_type,
                value_type,
                entries,
            },
            WireType::Map { key, value },
        ) if key_type == type_id(types, key)? && value_type == type_id(types, value)? => entries
            .into_iter()
            .map(|(entry_key, entry_value)| {
                Ok((
                    wire_to_native(entry_key, key, types)?,
                    wire_to_native(entry_value, value, types)?,
                ))
            })
            .collect::<Result<Vec<_>, ProviderError>>()
            .map(NativeValue::Map),
        (WireValue::Tuple { values }, WireType::Tuple { elements }) => {
            if values.len() != elements.len() {
                return Err(ProviderError::invalid_argument(
                    "provider wire tuple result length does not match its linked signature",
                ));
            }
            values
                .into_iter()
                .zip(elements)
                .map(|(value, element)| wire_to_native(value, element, types))
                .collect::<Result<Vec<_>, _>>()
                .map(NativeValue::List)
        }
        (
            WireValue::Variant {
                type_id: actual_type,
                variant_id,
                payload: Some(payload),
            },
            option @ WireType::Option { value: element },
        ) if actual_type == type_id(types, option)?
            && variant_id == WireCallTypeTable::option_some_variant() =>
        {
            Ok(NativeValue::Variant {
                name: "Some".to_string(),
                fields: BTreeMap::from([(
                    "value".to_string(),
                    wire_to_native(*payload, element, types)?,
                )]),
            })
        }
        (
            WireValue::Record {
                type_id: actual_type,
                fields,
            },
            named @ WireType::Named { .. },
        ) if actual_type == type_id(types, named)? => {
            let layout = types.record_layout(named).ok_or_else(|| {
                ProviderError::unavailable(
                    "provider named record is missing from its linked interface layout",
                )
            })?;
            if fields.len() != layout.fields.len() {
                return Err(ProviderError::invalid_argument(
                    "provider wire record result length does not match its linked interface layout",
                ));
            }
            let fields = layout
                .fields
                .iter()
                .zip(fields)
                .map(|(field, value)| {
                    wire_to_native(value, &field.ty, types).map(|value| (field.name.clone(), value))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            Ok(NativeValue::Struct {
                name: native_record_name(named),
                fields,
            })
        }
        (
            WireValue::Variant {
                type_id: actual_type,
                variant_id,
                payload: None,
            },
            option @ WireType::Option { .. },
        ) if actual_type == type_id(types, option)?
            && variant_id == WireCallTypeTable::option_none_variant() =>
        {
            Ok(NativeValue::Variant {
                name: "None".to_string(),
                fields: BTreeMap::new(),
            })
        }
        (
            WireValue::Variant {
                type_id: actual_type,
                variant_id,
                payload: Some(payload),
            },
            result @ WireType::Result { ok, .. },
        ) if actual_type == type_id(types, result)?
            && variant_id == WireCallTypeTable::result_ok_variant() =>
        {
            Ok(NativeValue::Variant {
                name: "Ok".to_string(),
                fields: BTreeMap::from([(
                    "value".to_string(),
                    wire_to_native(*payload, ok, types)?,
                )]),
            })
        }
        (
            WireValue::Variant {
                type_id: actual_type,
                variant_id,
                payload: Some(payload),
            },
            result @ WireType::Result { error, .. },
        ) if actual_type == type_id(types, result)?
            && variant_id == WireCallTypeTable::result_err_variant() =>
        {
            Ok(NativeValue::Variant {
                name: "Err".to_string(),
                fields: BTreeMap::from([(
                    "value".to_string(),
                    wire_to_native(*payload, error, types)?,
                )]),
            })
        }
        (
            value,
            WireType::Qualified {
                value: expected, ..
            },
        ) => wire_to_native(value, expected, types),
        (
            _,
            WireType::Unit
            | WireType::Bool
            | WireType::Int { .. }
            | WireType::Float { .. }
            | WireType::String
            | WireType::Char
            | WireType::Bytes,
        ) => Err(ProviderError::invalid_argument(
            "provider wire result does not match its linked scalar signature",
        )),
        _ => Err(ProviderError::unavailable(
            "provider wire value requires an Artifact type-table adapter",
        )),
    }
}

fn type_id(
    types: &WireCallTypeTable,
    expected: &WireType,
) -> Result<rsscript_abi_model::WireTypeId, ProviderError> {
    types.type_id(expected).ok_or_else(|| {
        ProviderError::internal("linked provider signature is missing a wire type identity")
    })
}

fn native_record_name_matches(name: &str, ty: &WireType) -> bool {
    let canonical = native_record_name(ty);
    canonical == name || matches!(ty, WireType::Named { name: local, .. } if local == name)
}

fn native_record_name(ty: &WireType) -> String {
    let WireType::Named { package, name, .. } = ty else {
        return String::new();
    };
    package
        .as_ref()
        .map_or_else(|| name.clone(), |package| format!("{package}.{name}"))
}

/// A shared permit held for the full lifetime of a non-reentrant Provider
/// invocation. Dropping a suspended async future releases its permit, so a
/// cancelled task cannot permanently lock out a Provider function.
struct NonReentrantCallPermit {
    active: Arc<AtomicBool>,
}

#[derive(Clone)]
enum AsyncProviderCallable {
    Native(AsyncInterpreterFn),
    Wire(AsyncWireInterpreterFn),
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

    pub fn new_async<F, Fut>(function: F) -> Self
    where
        F: Fn(AsyncProviderCallContext, Vec<NativeValue>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<NativeValue, ProviderError>> + Send + 'static,
    {
        AsyncInterpreterFn::new(function).into()
    }

    pub fn contract(&self) -> Option<&ProviderInvocationContract> {
        self.contract.as_ref()
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

    pub fn call_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<NativeValue>,
    ) -> Result<NativeValue, ProviderError> {
        let _non_reentrant_permit = self.acquire_non_reentrant_permit()?;
        if let Some(contract) = self.contract() {
            context.provider_id.clone_from(&contract.provider_id);
            context
                .provider_version
                .clone_from(&contract.provider_version);
            context.symbol = contract.descriptor.symbol.as_str().to_string();
        }
        let request_bytes = rsscript_provider_api::estimated_payload_bytes(&args);
        let started = Instant::now();
        let result = (|| {
            context.check_cancelled()?;
            check_payload_budget(request_bytes, context.remaining_byte_budget, "request")?;
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
            let result = catch_unwind(AssertUnwindSafe(|| match &self.callable {
                ProviderCallable::Sync(callable) => callable.call_with_context(context, args),
                ProviderCallable::WireSync(callable) => {
                    let contract = self.contract().ok_or_else(|| {
                        ProviderError::unavailable(
                            "wire provider function requires a linked descriptor",
                        )
                    })?;
                    let signature = contract.descriptor.signature.clone();
                    if signature.parameters.len() != args.len() {
                        return Err(ProviderError::invalid_argument(
                            "provider wire argument count does not match its linked signature",
                        ));
                    }
                    let types = wire_type_table(&signature, &contract.record_layouts)?;
                    let wire_args = args
                        .into_iter()
                        .zip(&signature.parameters)
                        .map(|(value, parameter)| native_to_wire(value, &parameter.ty, &types))
                        .collect::<Result<Vec<_>, _>>()?;
                    let result = callable.call_with_context(context, wire_args)?;
                    wire_to_native(result, &signature.result, &types)
                }
                ProviderCallable::Async(_) => Err(ProviderError::unavailable(
                    "async provider function requires the VM async dispatcher",
                )),
                ProviderCallable::WireAsync(_) => Err(ProviderError::unavailable(
                    "async wire provider function requires the VM async dispatcher",
                )),
            }))
            .unwrap_or_else(|_| Err(provider_panic_error()));
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
                    .map_or(0, |details| details.to_string().len()),
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

    pub fn start_async(
        &self,
        mut context: AsyncProviderCallContext,
        args: Vec<NativeValue>,
    ) -> ProviderFuture {
        let non_reentrant_permit = match self.acquire_non_reentrant_permit() {
            Ok(permit) => permit,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let Some(contract) = self.contract.clone() else {
            return Box::pin(async {
                Err(ProviderError::unavailable(
                    "async provider function requires a linked descriptor",
                ))
            });
        };
        context.provider_id.clone_from(&contract.provider_id);
        context
            .provider_version
            .clone_from(&contract.provider_version);
        context.symbol = contract.descriptor.symbol.as_str().to_string();
        let callable = match &self.callable {
            ProviderCallable::Async(callable) => AsyncProviderCallable::Native(callable.clone()),
            ProviderCallable::WireAsync(callable) => AsyncProviderCallable::Wire(callable.clone()),
            ProviderCallable::Sync(_) | ProviderCallable::WireSync(_) => {
                return Box::pin(async {
                    Err(ProviderError::unavailable(
                        "sync provider function cannot enter the async dispatcher",
                    ))
                });
            }
        };
        let request_bytes = rsscript_provider_api::estimated_payload_bytes(&args);
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
                        "async callable has a synchronous Provider descriptor",
                    ));
                }
                if contract.descriptor.blocking == BlockingBehavior::MayBlock {
                    return Err(ProviderError::unavailable(
                        "blocking work must not run inside an async Provider future",
                    ));
                }
                let result = match callable {
                    AsyncProviderCallable::Native(callable) => {
                        match catch_unwind(AssertUnwindSafe(|| callable.call(context, args))) {
                            Ok(future) => PanicContainedProviderFuture::new(future).await,
                            Err(_) => Err(provider_panic_error()),
                        }
                    }
                    AsyncProviderCallable::Wire(callable) => {
                        let signature = contract.descriptor.signature.clone();
                        if signature.parameters.len() != args.len() {
                            return Err(ProviderError::invalid_argument(
                                "provider wire argument count does not match its linked signature",
                            ));
                        }
                        let types = wire_type_table(&signature, &contract.record_layouts)?;
                        let wire_args = args
                            .into_iter()
                            .zip(&signature.parameters)
                            .map(|(value, parameter)| native_to_wire(value, &parameter.ty, &types))
                            .collect::<Result<Vec<_>, _>>()?;
                        let result = match catch_unwind(AssertUnwindSafe(|| {
                            callable.call(context, wire_args)
                        })) {
                            Ok(future) => PanicContainedWireProviderFuture::new(future).await,
                            Err(_) => Err(provider_panic_error()),
                        };
                        result.and_then(|value| wire_to_native(value, &signature.result, &types))
                    }
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
                        .map_or(0, |details| details.to_string().len()),
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
) -> Result<WireCallTypeTable, ProviderError> {
    WireCallTypeTable::for_signature(signature)
        .and_then(|table| table.with_record_layouts(record_layouts.to_vec()))
        .map_err(|error| {
            ProviderError::internal(format!(
                "linked provider signature cannot form a wire type table: {error}"
            ))
        })
}

impl From<rsscript_provider_api::NativeHostFn> for ExternalFunction {
    fn from(function: rsscript_provider_api::NativeHostFn) -> Self {
        Self::from_fn(function)
    }
}

impl From<NativeInterpreterFn> for ExternalFunction {
    fn from(callable: NativeInterpreterFn) -> Self {
        Self {
            callable: callable.into(),
            contract: None,
            host_context: Arc::new(HostCallContext::default()),
            active_non_reentrant_call: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl From<AsyncInterpreterFn> for ExternalFunction {
    fn from(callable: AsyncInterpreterFn) -> Self {
        Self {
            callable: callable.into(),
            contract: None,
            host_context: Arc::new(HostCallContext::default()),
            active_non_reentrant_call: Arc::new(AtomicBool::new(false)),
        }
    }
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

impl From<ExternalFunction> for NativeInterpreterFn {
    fn from(function: ExternalFunction) -> Self {
        match function.callable {
            ProviderCallable::Sync(callable) => callable,
            ProviderCallable::WireSync(_) => NativeInterpreterFn::new(|_| {
                Err(ProviderError::unavailable(
                    "wire Provider callable cannot be converted to the legacy sync adapter",
                ))
            }),
            ProviderCallable::Async(_) => NativeInterpreterFn::new(|_| {
                Err(ProviderError::unavailable(
                    "async Provider callable cannot be converted to a sync callable",
                ))
            }),
            ProviderCallable::WireAsync(_) => NativeInterpreterFn::new(|_| {
                Err(ProviderError::unavailable(
                    "async wire Provider callable cannot be converted to a sync callable",
                ))
            }),
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
    pub native_value: Option<NativeValue>,
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
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
    pub provider_call_traces: Vec<ProviderCallTrace>,
    pub failure: Option<EvalError>,
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

#[cfg(test)]
mod provider_contract_tests {
    use super::*;
    use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationId};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    fn registered_function(
        blocking: BlockingBehavior,
        cleanup: ResourceCleanupContract,
    ) -> ExternalFunction {
        registered_sync_function(
            blocking,
            cleanup,
            NativeInterpreterFn::new(|_| Ok(NativeValue::Unit)),
        )
    }

    fn registered_sync_function(
        blocking: BlockingBehavior,
        cleanup: ResourceCleanupContract,
        callable: NativeInterpreterFn,
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
            record_layouts: Vec::new(),
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
                        callable,
                    },
                )]),
            )
            .unwrap();
        registry.into_bindings().next().unwrap().1
    }

    fn registered_scalar_wire_function(callable: WireInterpreterFn) -> ExternalFunction {
        let symbol = ExternalSymbol::new("host.test.increment").unwrap();
        let signature = FunctionSignature {
            parameters: vec![rsscript_abi_model::ParameterSignature {
                name: "value".to_string(),
                effect: rsscript_abi_model::DataEffect::Read,
                ty: "Int".into(),
                retained: false,
            }],
            result: "Int".into(),
            asynchronous: false,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.provider".to_string(),
            provider_version: "1.0.0".to_string(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            record_layouts: Vec::new(),
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "increment".to_string(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
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
                        callable: ProviderCallable::from(callable),
                    },
                )]),
            )
            .unwrap();
        registry.into_bindings().next().unwrap().1
    }

    #[test]
    fn linked_scalar_wire_provider_avoids_the_native_callable_adapter() {
        let function = registered_scalar_wire_function(WireInterpreterFn::new(|args| {
            assert_eq!(args, vec![WireValue::Int { value: 41 }]);
            Ok(WireValue::Int { value: 42 })
        }));
        let mut context = ProviderCallContext::default();
        let result = function
            .call_with_context(&mut context, vec![NativeValue::Int(41)])
            .expect("linked scalar wire call");
        assert_eq!(result, NativeValue::Int(42));
    }

    #[test]
    fn linked_wire_provider_decodes_record_results_from_descriptor_layout() {
        let symbol = ExternalSymbol::new("host.test.record").unwrap();
        let record = WireType::from("host.test.Result");
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: record.clone(),
            asynchronous: false,
        };
        let layouts = vec![rsscript_abi_model::WireRecordLayout {
            ty: record.clone(),
            fields: vec![rsscript_abi_model::WireRecordFieldLayout {
                name: "value".into(),
                ty: WireType::Int {
                    bits: 64,
                    signed: true,
                },
            }],
        }];
        let type_id = WireCallTypeTable::for_signature(&signature)
            .unwrap()
            .with_record_layouts(layouts.clone())
            .unwrap()
            .type_id(&record)
            .unwrap();
        let descriptor = ProviderDescriptor {
            provider_id: "test.provider".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            record_layouts: layouts,
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "record".into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
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
                        callable: WireInterpreterFn::new(move |_| {
                            Ok(WireValue::Record {
                                type_id,
                                fields: vec![WireValue::Int { value: 42 }],
                            })
                        }),
                    },
                )]),
            )
            .unwrap();
        let function = registry.into_bindings().next().unwrap().1;
        let mut context = ProviderCallContext::default();
        assert_eq!(
            function
                .call_with_context(&mut context, Vec::new())
                .unwrap(),
            NativeValue::Struct {
                name: "host.test.Result".into(),
                fields: BTreeMap::from([("value".into(), NativeValue::Int(42))]),
            }
        );
    }

    #[test]
    fn descriptor_type_table_adapts_list_values() {
        let list = WireType::List {
            element: Box::new(WireType::Int {
                bits: 64,
                signed: true,
            }),
        };
        let signature = FunctionSignature {
            parameters: vec![],
            result: list.clone(),
            asynchronous: false,
        };
        let types = WireCallTypeTable::for_signature(&signature).unwrap();
        let wire = native_to_wire(
            NativeValue::List(vec![NativeValue::Int(1), NativeValue::Int(2)]),
            &list,
            &types,
        )
        .expect("list has a descriptor-derived element identity");
        assert_eq!(
            wire,
            WireValue::List {
                element_type: types
                    .type_id(match &list {
                        WireType::List { element } => element,
                        _ => unreachable!(),
                    })
                    .unwrap(),
                values: vec![WireValue::Int { value: 1 }, WireValue::Int { value: 2 }],
            }
        );
        assert_eq!(
            wire_to_native(wire, &list, &types).unwrap(),
            NativeValue::List(vec![NativeValue::Int(1), NativeValue::Int(2)])
        );
    }

    #[test]
    fn descriptor_type_table_adapts_char_and_map_values() {
        let map = WireType::Map {
            key: Box::new(WireType::String),
            value: Box::new(WireType::Char),
        };
        let signature = FunctionSignature {
            parameters: vec![rsscript_abi_model::ParameterSignature {
                name: "entries".into(),
                effect: rsscript_abi_model::DataEffect::Read,
                ty: map.clone(),
                retained: false,
            }],
            result: WireType::Char,
            asynchronous: false,
        };
        let types = WireCallTypeTable::for_signature(&signature).unwrap();
        let value = NativeValue::Map(vec![
            (NativeValue::String("left".into()), NativeValue::Char('a')),
            (NativeValue::String("right".into()), NativeValue::Char('z')),
        ]);
        let wire = native_to_wire(value.clone(), &map, &types)
            .expect("descriptor-derived map identities bridge the legacy adapter");
        assert_eq!(
            wire,
            WireValue::Map {
                key_type: types.type_id(&WireType::String).unwrap(),
                value_type: types.type_id(&WireType::Char).unwrap(),
                entries: vec![
                    (
                        WireValue::String {
                            value: "left".into()
                        },
                        WireValue::Char { value: 'a' },
                    ),
                    (
                        WireValue::String {
                            value: "right".into()
                        },
                        WireValue::Char { value: 'z' },
                    ),
                ],
            }
        );
        assert_eq!(
            wire_to_native(wire, &map, &types).unwrap(),
            value,
            "map adapter preserves declaration-order entries and Char values"
        );
    }

    #[test]
    fn descriptor_type_table_adapts_option_values() {
        let option = WireType::Option {
            value: Box::new(WireType::String),
        };
        let signature = FunctionSignature {
            parameters: vec![],
            result: option.clone(),
            asynchronous: false,
        };
        let types = WireCallTypeTable::for_signature(&signature).unwrap();
        let some = NativeValue::Variant {
            name: "Some".to_string(),
            fields: BTreeMap::from([(
                "value".to_string(),
                NativeValue::String("value".to_string()),
            )]),
        };
        let wire = native_to_wire(some.clone(), &option, &types).unwrap();
        assert_eq!(wire_to_native(wire, &option, &types).unwrap(), some);
        let none = NativeValue::Variant {
            name: "None".to_string(),
            fields: BTreeMap::new(),
        };
        let wire = native_to_wire(none.clone(), &option, &types).unwrap();
        assert_eq!(wire_to_native(wire, &option, &types).unwrap(), none);
    }

    #[test]
    fn descriptor_type_table_adapts_tuple_and_result_values() {
        let tuple = WireType::Tuple {
            elements: vec![
                WireType::String,
                WireType::Int {
                    bits: 64,
                    signed: true,
                },
            ],
        };
        let tuple_signature = FunctionSignature {
            parameters: vec![],
            result: tuple.clone(),
            asynchronous: false,
        };
        let tuple_types = WireCallTypeTable::for_signature(&tuple_signature).unwrap();
        let tuple_value = NativeValue::List(vec![
            NativeValue::String("left".to_string()),
            NativeValue::Int(2),
        ]);
        let wire = native_to_wire(tuple_value.clone(), &tuple, &tuple_types).unwrap();
        assert_eq!(
            wire_to_native(wire, &tuple, &tuple_types).unwrap(),
            tuple_value
        );

        let result = WireType::Result {
            ok: Box::new(WireType::String),
            error: Box::new(WireType::Int {
                bits: 64,
                signed: true,
            }),
        };
        let result_signature = FunctionSignature {
            parameters: vec![],
            result: result.clone(),
            asynchronous: false,
        };
        let result_types = WireCallTypeTable::for_signature(&result_signature).unwrap();
        for value in [
            NativeValue::Variant {
                name: "Ok".to_string(),
                fields: BTreeMap::from([(
                    "value".to_string(),
                    NativeValue::String("done".to_string()),
                )]),
            },
            NativeValue::Variant {
                name: "Err".to_string(),
                fields: BTreeMap::from([("value".to_string(), NativeValue::Int(7))]),
            },
        ] {
            let wire = native_to_wire(value.clone(), &result, &result_types).unwrap();
            assert_eq!(wire_to_native(wire, &result, &result_types).unwrap(), value);
        }
    }

    #[test]
    fn descriptor_type_table_rejects_named_values_without_an_artifact_layout() {
        let named = WireType::Named {
            package: Some("host.example".to_string()),
            name: "Record".to_string(),
            arguments: vec![],
        };
        let signature = FunctionSignature {
            parameters: vec![],
            result: named.clone(),
            asynchronous: false,
        };
        let types = WireCallTypeTable::for_signature(&signature).unwrap();
        let error = native_to_wire(
            NativeValue::Struct {
                name: "Record".to_string(),
                fields: BTreeMap::new(),
            },
            &named,
            &types,
        )
        .expect_err("named records still require an Artifact type-table adapter");
        assert_eq!(error.code, ProviderErrorCode::Unavailable);
    }

    fn registered_async_function(callable: AsyncInterpreterFn) -> ExternalFunction {
        registered_async_function_with_reentrancy(callable, true)
    }

    fn registered_async_function_with_reentrancy(
        callable: AsyncInterpreterFn,
        reentrant: bool,
    ) -> ExternalFunction {
        let symbol = ExternalSymbol::new("host.test.async_run").unwrap();
        let signature = FunctionSignature {
            parameters: vec![],
            result: "Unit".into(),
            asynchronous: true,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.provider".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            record_layouts: Vec::new(),
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "async_run".into(),
                call_mode: ProviderCallMode::Async,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant,
                resource_cleanup: ResourceCleanupContract::None,
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
                        callable,
                    },
                )]),
            )
            .unwrap();
        registry.into_bindings().next().unwrap().1
    }

    fn registered_async_wire_function(callable: AsyncWireInterpreterFn) -> ExternalFunction {
        let symbol = ExternalSymbol::new("host.test.async_increment").unwrap();
        let signature = FunctionSignature {
            parameters: vec![rsscript_abi_model::ParameterSignature {
                name: "value".to_string(),
                effect: rsscript_abi_model::DataEffect::Read,
                ty: "Int".into(),
                retained: false,
            }],
            result: "Int".into(),
            asynchronous: true,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.provider".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            record_layouts: Vec::new(),
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "async_increment".into(),
                call_mode: ProviderCallMode::Async,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
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
                        callable,
                    },
                )]),
            )
            .unwrap();
        registry.into_bindings().next().unwrap().1
    }

    fn async_context(
        cancellation: Option<CancellationToken>,
        deadline: Option<MonotonicDeadline>,
    ) -> AsyncProviderCallContext {
        AsyncProviderCallContext {
            cancellation,
            deadline,
            remaining_byte_budget: None,
            remaining_output_budget: None,
            call_id: OperationId(7),
            provider_id: String::new(),
            provider_version: String::new(),
            symbol: "host.test.async_run".into(),
            host_context: Arc::new(HostCallContext::default()),
            trace: None,
            resources: None,
        }
    }

    fn poll_once(future: &mut ProviderFuture) -> Poll<Result<NativeValue, ProviderError>> {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        future.as_mut().poll(&mut context)
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
    fn dispatchers_enforce_request_and_response_payload_limits() {
        let called = Arc::new(AtomicBool::new(false));
        let called_by_provider = Arc::clone(&called);
        let function = registered_sync_function(
            BlockingBehavior::NonBlocking,
            ResourceCleanupContract::None,
            NativeInterpreterFn::new(move |_| {
                called_by_provider.store(true, Ordering::SeqCst);
                Ok(NativeValue::String("response".into()))
            }),
        );
        let mut request_context = ProviderCallContext {
            remaining_byte_budget: Some(3),
            ..ProviderCallContext::default()
        };
        let request_error = function
            .call_with_context(
                &mut request_context,
                vec![NativeValue::String("request".into())],
            )
            .expect_err("oversized request must fail before Provider code runs");
        assert_eq!(request_error.code, ProviderErrorCode::ResourceExhausted);
        assert!(!called.load(Ordering::SeqCst));

        let mut response_context = ProviderCallContext {
            remaining_byte_budget: Some(64),
            remaining_output_budget: Some(3),
            ..ProviderCallContext::default()
        };
        let response_error = function
            .call_with_context(&mut response_context, vec![])
            .expect_err("oversized response must fail at the dispatcher boundary");
        assert_eq!(response_error.code, ProviderErrorCode::ResourceExhausted);
        assert!(called.load(Ordering::SeqCst));
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

    #[test]
    fn callable_mode_mismatch_fails_during_provider_registration() {
        let symbol = ExternalSymbol::new("host.test.async_run").unwrap();
        let signature = FunctionSignature {
            parameters: vec![],
            result: "Unit".into(),
            asynchronous: true,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test.provider".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION],
            record_layouts: Vec::new(),
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "async_run".into(),
                call_mode: ProviderCallMode::Async,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        };
        let error = ExternalFunctionRegistry::new()
            .register_provider(
                &descriptor,
                BTreeMap::from([(
                    symbol.clone(),
                    ProviderFunction {
                        signature,
                        callable: NativeInterpreterFn::new(|_| Ok(NativeValue::Unit)),
                    },
                )]),
            )
            .unwrap_err();
        assert_eq!(error, ProviderLoadError::CallModeMismatch(symbol));
    }

    #[test]
    fn sync_dispatcher_contains_provider_panics() {
        let function = registered_sync_function(
            BlockingBehavior::NonBlocking,
            ResourceCleanupContract::None,
            NativeInterpreterFn::new(|_| -> Result<NativeValue, ProviderError> {
                panic!("test Provider panic");
            }),
        );
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            function.call_with_context(&mut ProviderCallContext::default(), vec![])
        }));
        let error = result
            .expect("dispatcher must contain a Provider unwind")
            .expect_err("contained panic must become a Provider error");
        assert_eq!(error.code, ProviderErrorCode::Internal);
        assert_eq!(error.message, "provider callable panicked");
    }

    #[test]
    fn async_dispatcher_observes_cancellation_while_provider_is_suspended() {
        let started = Arc::new(AtomicBool::new(false));
        let started_by_provider = Arc::clone(&started);
        let function = registered_async_function(AsyncInterpreterFn::new(move |_, _| {
            let started = Arc::clone(&started_by_provider);
            async move {
                std::future::poll_fn(move |_| {
                    if !started.swap(true, Ordering::SeqCst) {
                        Poll::Pending
                    } else {
                        Poll::Ready(Ok(NativeValue::Unit))
                    }
                })
                .await
            }
        }));
        let cancellation = CancellationToken::new();
        let mut future =
            function.start_async(async_context(Some(cancellation.clone()), None), vec![]);

        assert!(matches!(poll_once(&mut future), Poll::Pending));
        assert!(started.load(Ordering::SeqCst));
        cancellation.cancel();

        let error = match poll_once(&mut future) {
            Poll::Ready(Err(error)) => error,
            result => panic!("expected cancellation after pending provider future, got {result:?}"),
        };
        assert_eq!(error.code, ProviderErrorCode::Cancelled);
    }

    #[test]
    fn async_wire_dispatcher_adapts_the_linked_signature() {
        let function =
            registered_async_wire_function(AsyncWireInterpreterFn::new(|_, args| async move {
                assert_eq!(args, vec![WireValue::Int { value: 41 }]);
                Ok(WireValue::Int { value: 42 })
            }));
        let mut future =
            function.start_async(async_context(None, None), vec![NativeValue::Int(41)]);
        assert_eq!(
            poll_once(&mut future),
            Poll::Ready(Ok(NativeValue::Int(42)))
        );
    }

    #[test]
    fn async_dispatcher_enforces_response_payload_limits() {
        let function = registered_async_function(AsyncInterpreterFn::new(|_, _| async move {
            Ok(NativeValue::String("response".into()))
        }));
        let mut context = async_context(None, None);
        context.remaining_byte_budget = Some(64);
        context.remaining_output_budget = Some(3);
        let mut future = function.start_async(context, vec![]);

        let error = match poll_once(&mut future) {
            Poll::Ready(Err(error)) => error,
            result => panic!("expected payload limit failure, got {result:?}"),
        };
        assert_eq!(error.code, ProviderErrorCode::ResourceExhausted);
    }

    #[test]
    fn async_dispatcher_enforces_non_reentrant_provider_contracts() {
        let function = registered_async_function_with_reentrancy(
            AsyncInterpreterFn::new(|_, _| async move {
                std::future::pending::<Result<NativeValue, ProviderError>>().await
            }),
            false,
        );
        let mut first = function.start_async(async_context(None, None), vec![]);
        assert!(matches!(poll_once(&mut first), Poll::Pending));

        let mut second = function.start_async(async_context(None, None), vec![]);
        let error = match poll_once(&mut second) {
            Poll::Ready(Err(error)) => error,
            result => panic!("expected non-reentrant Provider rejection, got {result:?}"),
        };
        assert_eq!(error.code, ProviderErrorCode::Unavailable);
        drop(first);

        let mut after_drop = function.start_async(async_context(None, None), vec![]);
        assert!(matches!(poll_once(&mut after_drop), Poll::Pending));
    }

    #[test]
    fn async_dispatcher_observes_deadline_expiry_while_provider_is_suspended() {
        let function = registered_async_function(AsyncInterpreterFn::new(|_, _| async move {
            let mut pending = true;
            std::future::poll_fn(move |_| {
                if pending {
                    pending = false;
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(NativeValue::Unit))
                }
            })
            .await
        }));
        let deadline = MonotonicDeadline::after(Duration::from_millis(1));
        let mut future = function.start_async(async_context(None, Some(deadline)), vec![]);

        assert!(matches!(poll_once(&mut future), Poll::Pending));
        std::thread::sleep(Duration::from_millis(5));

        let error = match poll_once(&mut future) {
            Poll::Ready(Err(error)) => error,
            result => panic!("expected deadline after pending provider future, got {result:?}"),
        };
        assert_eq!(error.code, ProviderErrorCode::DeadlineExceeded);
    }

    #[test]
    fn async_dispatcher_contains_provider_future_panics() {
        let function = registered_async_function(AsyncInterpreterFn::new(|_, _| async move {
            panic!("test Provider future panic");
            #[allow(unreachable_code)]
            Ok(NativeValue::Unit)
        }));
        let mut future = function.start_async(async_context(None, None), vec![]);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| poll_once(&mut future)));
        let error = match result.expect("dispatcher must contain a Provider future unwind") {
            Poll::Ready(Err(error)) => error,
            outcome => panic!("expected contained Provider panic, got {outcome:?}"),
        };
        assert_eq!(error.code, ProviderErrorCode::Internal);
        assert_eq!(error.message, "provider callable panicked");
    }
}
