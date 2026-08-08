#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use rsscript_abi_model::{
    DataEffect, ExternalImport, ExternalSymbol, FunctionSignature, InvalidExternalSymbol,
    ParameterSignature, RUNTIME_ABI_VERSION, SignatureHash,
};
pub use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationId};
use serde::{Deserialize, Serialize};

/// Runtime value exchanged with trusted provider implementations. This safe
/// model is independent of any dynamic-library ABI; native adapters serialize it
/// at their own boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NativeValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Bytes(Vec<u8>),
    List(Vec<NativeValue>),
    Map(Vec<(NativeValue, NativeValue)>),
    Json(serde_json::Value),
    Struct {
        name: String,
        fields: BTreeMap<String, NativeValue>,
    },
    Variant {
        name: String,
        fields: BTreeMap<String, NativeValue>,
    },
    Native {
        type_name: String,
        id: i64,
    },
}

impl NativeValue {
    /// Return a deterministic estimate of the logical payload bytes crossing a
    /// Provider boundary. This intentionally excludes allocator capacity and
    /// transport framing, which vary by Provider implementation.
    pub fn estimated_payload_bytes(&self) -> usize {
        let mut total = 0usize;
        let mut values = vec![self];
        while let Some(value) = values.pop() {
            match value {
                Self::Unit => {}
                Self::Int(_) | Self::Float(_) => total = total.saturating_add(8),
                Self::Bool(_) => total = total.saturating_add(1),
                Self::Char(value) => total = total.saturating_add(value.len_utf8()),
                Self::String(value) => total = total.saturating_add(value.len()),
                Self::Bytes(value) => total = total.saturating_add(value.len()),
                Self::List(items) => values.extend(items),
                Self::Map(entries) => {
                    for (key, value) in entries {
                        values.push(key);
                        values.push(value);
                    }
                }
                Self::Json(value) => total = total.saturating_add(json_payload_bytes(value)),
                Self::Struct { name, fields } | Self::Variant { name, fields } => {
                    total = total.saturating_add(name.len());
                    for (name, value) in fields {
                        total = total.saturating_add(name.len());
                        values.push(value);
                    }
                }
                Self::Native { type_name, .. } => {
                    total = total.saturating_add(type_name.len()).saturating_add(8);
                }
            }
        }
        total
    }
}

fn json_payload_bytes(root: &serde_json::Value) -> usize {
    let mut total = 0usize;
    let mut values = vec![root];
    while let Some(value) = values.pop() {
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(_) => total = total.saturating_add(1),
            serde_json::Value::Number(_) => total = total.saturating_add(8),
            serde_json::Value::String(value) => total = total.saturating_add(value.len()),
            serde_json::Value::Array(items) => values.extend(items),
            serde_json::Value::Object(fields) => {
                for (name, value) in fields {
                    total = total.saturating_add(name.len());
                    values.push(value);
                }
            }
        }
    }
    total
}

pub fn estimated_payload_bytes(values: &[NativeValue]) -> usize {
    values.iter().fold(0usize, |total, value| {
        total.saturating_add(value.estimated_payload_bytes())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCode {
    InvalidArgument,
    NotFound,
    PermissionDenied,
    Cancelled,
    DeadlineExceeded,
    ResourceExhausted,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ProviderError {
    pub fn new(code: ProviderErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorCode::Internal, message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorCode::InvalidArgument, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorCode::Unavailable, message)
    }

    pub fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorCode::ResourceExhausted, message)
    }

    pub fn from_io(operation: &str, error: std::io::Error) -> Self {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ProviderErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => ProviderErrorCode::PermissionDenied,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                ProviderErrorCode::InvalidArgument
            }
            std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected => ProviderErrorCode::Unavailable,
            _ => ProviderErrorCode::Internal,
        };
        Self {
            code,
            message: format!("{operation}: {error}"),
            retryable: matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ),
            details: Some(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) })),
        }
    }
}

impl ProviderErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ResourceExhausted => "resource_exhausted",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl Error for ProviderError {}

/// Host-constructed, instance-local authority presented to Provider calls.
///
/// RSScript does not interpret these scopes as a language policy. A Provider
/// may use them to narrow an already configured host capability (for example,
/// selecting one of several rooted filesystem views).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderAuthority {
    scopes: BTreeSet<String>,
}

impl ProviderAuthority {
    pub fn scoped(scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            scopes: scopes.into_iter().map(Into::into).collect(),
        }
    }

    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }

    pub fn scopes(&self) -> impl Iterator<Item = &str> {
        self.scopes.iter().map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderCallTrace {
    pub call_id: OperationId,
    pub provider_id: String,
    pub provider_version: String,
    pub symbol: String,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub elapsed: Duration,
    pub result: Result<(), ProviderErrorCode>,
}

pub trait ProviderTraceSink: Send + Sync {
    fn record(&self, trace: ProviderCallTrace);
}

pub struct ProviderCallContext<'a> {
    pub cancellation: Option<&'a CancellationToken>,
    pub deadline: Option<MonotonicDeadline>,
    pub remaining_byte_budget: Option<usize>,
    pub remaining_output_budget: Option<usize>,
    pub call_id: OperationId,
    pub provider_id: String,
    pub provider_version: String,
    pub symbol: String,
    pub authority: &'a ProviderAuthority,
    pub trace: Option<&'a dyn ProviderTraceSink>,
    pub resources: Option<&'a mut ProviderResourceTable>,
    /// Set only by a runtime lane that is prepared for a synchronous provider
    /// call to block its worker. Inline callers remain fail-closed by default.
    pub blocking_allowed: bool,
    /// Set only by an async-aware dispatcher. A sync call path must not invoke
    /// a descriptor that declares an async ABI.
    pub async_allowed: bool,
}

impl ProviderCallContext<'_> {
    pub fn check_cancelled(&self) -> Result<(), ProviderError> {
        if self
            .cancellation
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ProviderError::new(
                ProviderErrorCode::Cancelled,
                "provider call cancelled",
            ));
        }
        if self.deadline.is_some_and(MonotonicDeadline::is_expired) {
            return Err(ProviderError::new(
                ProviderErrorCode::DeadlineExceeded,
                "provider call deadline exceeded",
            ));
        }
        Ok(())
    }
}

impl Default for ProviderCallContext<'static> {
    fn default() -> Self {
        static EMPTY_AUTHORITY: std::sync::LazyLock<ProviderAuthority> =
            std::sync::LazyLock::new(ProviderAuthority::default);
        Self {
            cancellation: None,
            deadline: None,
            remaining_byte_budget: None,
            remaining_output_budget: None,
            call_id: OperationId(0),
            provider_id: String::new(),
            provider_version: String::new(),
            symbol: String::new(),
            authority: &EMPTY_AUTHORITY,
            trace: None,
            resources: None,
            blocking_allowed: false,
            async_allowed: false,
        }
    }
}

impl ProviderCallContext<'_> {
    pub fn register_resource(
        &mut self,
        resource: impl ProviderResource + 'static,
    ) -> Result<ResourceHandle, ProviderError> {
        self.resources
            .as_deref_mut()
            .ok_or_else(|| ProviderError::internal("runtime resource table is unavailable"))?
            .register(Box::new(resource))
    }

    pub fn cleanup_resource(&mut self, handle: ResourceHandle) -> Result<(), ProviderError> {
        self.resources
            .as_deref_mut()
            .ok_or_else(|| ProviderError::internal("runtime resource table is unavailable"))?
            .cleanup(handle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceHandle {
    pub slot: u32,
    pub generation: u32,
}

impl ResourceHandle {
    pub fn to_native_id(self) -> i64 {
        i64::from_ne_bytes(
            ((u64::from(self.generation) << 32) | u64::from(self.slot)).to_ne_bytes(),
        )
    }

    pub fn from_native_id(id: i64) -> Self {
        let bits = u64::from_ne_bytes(id.to_ne_bytes());
        Self {
            slot: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

pub trait ProviderResource: Send {
    fn cleanup(&mut self) -> Result<(), ProviderError>;
}

struct ResourceSlot {
    generation: u32,
    resource: Option<Box<dyn ProviderResource>>,
}

pub struct ProviderResourceTable {
    slots: Vec<ResourceSlot>,
    limit: Option<usize>,
    live: usize,
    peak_live: usize,
    created: u64,
    cleaned: u64,
    cleanup_failures: u64,
}

impl ProviderResourceTable {
    pub fn new(limit: Option<usize>) -> Self {
        Self {
            slots: Vec::new(),
            limit,
            live: 0,
            peak_live: 0,
            created: 0,
            cleaned: 0,
            cleanup_failures: 0,
        }
    }

    pub fn set_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
    }

    pub fn register(
        &mut self,
        resource: Box<dyn ProviderResource>,
    ) -> Result<ResourceHandle, ProviderError> {
        if self.limit.is_some_and(|limit| self.live >= limit) {
            return Err(ProviderError::new(
                ProviderErrorCode::ResourceExhausted,
                "provider resource limit exceeded",
            ));
        }
        let slot = self
            .slots
            .iter()
            .position(|slot| slot.resource.is_none())
            .unwrap_or(self.slots.len());
        if slot == self.slots.len() {
            self.slots.push(ResourceSlot {
                generation: 0,
                resource: None,
            });
        }
        let entry = &mut self.slots[slot];
        entry.resource = Some(resource);
        self.live += 1;
        self.peak_live = self.peak_live.max(self.live);
        self.created += 1;
        Ok(ResourceHandle {
            slot: u32::try_from(slot).map_err(|_| {
                ProviderError::new(
                    ProviderErrorCode::ResourceExhausted,
                    "provider resource slot exceeds handle range",
                )
            })?,
            generation: entry.generation,
        })
    }

    pub fn cleanup(&mut self, handle: ResourceHandle) -> Result<(), ProviderError> {
        let slot = self
            .slots
            .get_mut(handle.slot as usize)
            .filter(|slot| slot.generation == handle.generation)
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCode::InvalidArgument,
                    "stale or invalid provider resource handle",
                )
            })?;
        let mut resource = slot.resource.take().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::InvalidArgument,
                "provider resource is already closed",
            )
        })?;
        let result = resource.cleanup();
        slot.generation = slot.generation.wrapping_add(1);
        self.live = self.live.saturating_sub(1);
        if result.is_ok() {
            self.cleaned += 1;
        } else {
            self.cleanup_failures += 1;
        }
        result
    }

    pub fn cleanup_all(&mut self) -> Vec<ProviderError> {
        let handles = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.resource.is_some())
            .map(|(index, slot)| ResourceHandle {
                slot: index as u32,
                generation: slot.generation,
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .filter_map(|handle| self.cleanup(handle).err())
            .collect()
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn peak_live(&self) -> usize {
        self.peak_live
    }

    pub fn created(&self) -> u64 {
        self.created
    }

    pub fn cleaned(&self) -> u64 {
        self.cleaned
    }

    pub fn cleanup_failures(&self) -> u64 {
        self.cleanup_failures
    }
}

impl Drop for ProviderResourceTable {
    fn drop(&mut self) {
        drop(self.cleanup_all());
    }
}

/// Cloneable runtime-owned resource registrar for Provider futures.
///
/// Async Provider calls may outlive the stack frame that started them, so they
/// cannot borrow the VM's table through `ProviderCallContext`. The registry
/// keeps the same generation-safe table behind a short critical section while
/// preserving one owner for final cleanup and telemetry.
#[derive(Clone)]
pub struct ProviderResourceRegistry {
    inner: Arc<Mutex<ProviderResourceTable>>,
}

impl ProviderResourceRegistry {
    pub fn new(limit: Option<usize>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProviderResourceTable::new(limit))),
        }
    }

    pub fn set_limit(&self, limit: Option<usize>) -> Result<(), ProviderError> {
        self.with_table(|table| {
            table.set_limit(limit);
            Ok(())
        })
    }

    pub fn register(
        &self,
        resource: impl ProviderResource + 'static,
    ) -> Result<ResourceHandle, ProviderError> {
        self.with_table(|table| table.register(Box::new(resource)))
    }

    pub fn cleanup(&self, handle: ResourceHandle) -> Result<(), ProviderError> {
        self.with_table(|table| table.cleanup(handle))
    }

    pub fn cleanup_all(&self) -> Result<Vec<ProviderError>, ProviderError> {
        self.with_table(|table| Ok(table.cleanup_all()))
    }

    pub fn snapshot(&self) -> Result<ProviderResourceUsage, ProviderError> {
        self.with_table(|table| {
            Ok(ProviderResourceUsage {
                live: table.live(),
                peak_live: table.peak_live(),
                created: table.created(),
                cleaned: table.cleaned(),
                cleanup_failures: table.cleanup_failures(),
            })
        })
    }

    pub fn with_table<T>(
        &self,
        action: impl FnOnce(&mut ProviderResourceTable) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let mut table = self
            .inner
            .lock()
            .map_err(|_| ProviderError::internal("provider resource table lock poisoned"))?;
        action(&mut table)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ProviderResourceUsage {
    pub live: usize,
    pub peak_live: usize,
    pub created: u64,
    pub cleaned: u64,
    pub cleanup_failures: u64,
}

/// Owned context passed to an asynchronous Provider callable.
#[derive(Clone)]
pub struct AsyncProviderCallContext {
    pub cancellation: Option<CancellationToken>,
    pub deadline: Option<MonotonicDeadline>,
    pub remaining_byte_budget: Option<usize>,
    pub remaining_output_budget: Option<usize>,
    pub call_id: OperationId,
    pub provider_id: String,
    pub provider_version: String,
    pub symbol: String,
    pub authority: Arc<ProviderAuthority>,
    pub trace: Option<Arc<dyn ProviderTraceSink>>,
    pub resources: Option<ProviderResourceRegistry>,
}

impl AsyncProviderCallContext {
    pub fn check_cancelled(&self) -> Result<(), ProviderError> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ProviderError::new(
                ProviderErrorCode::Cancelled,
                "provider call cancelled",
            ));
        }
        if self.deadline.is_some_and(MonotonicDeadline::is_expired) {
            return Err(ProviderError::new(
                ProviderErrorCode::DeadlineExceeded,
                "provider call deadline exceeded",
            ));
        }
        Ok(())
    }

    pub fn register_resource(
        &self,
        resource: impl ProviderResource + 'static,
    ) -> Result<ResourceHandle, ProviderError> {
        self.resources
            .as_ref()
            .ok_or_else(|| ProviderError::internal("runtime resource table is unavailable"))?
            .register(resource)
    }

    pub fn cleanup_resource(&self, handle: ResourceHandle) -> Result<(), ProviderError> {
        self.resources
            .as_ref()
            .ok_or_else(|| ProviderError::internal("runtime resource table is unavailable"))?
            .cleanup(handle)
    }
}

pub type NativeHostFn = fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError>;

/// Cloneable provider callable used by the runtime registry.
type ContextualProviderFn = dyn for<'a> Fn(&mut ProviderCallContext<'a>, Vec<NativeValue>) -> Result<NativeValue, ProviderError>
    + Send
    + Sync;

#[derive(Clone)]
pub struct NativeInterpreterFn {
    inner: Arc<ContextualProviderFn>,
}

impl NativeInterpreterFn {
    pub fn from_fn(function: NativeHostFn) -> Self {
        Self::new(function)
    }

    pub fn new(
        function: impl Fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::new_contextual(move |_, args| function(args))
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
        Self {
            inner: Arc::new(function),
        }
    }

    /// Invoke through an explicit runtime-owned context. Callers cannot bypass
    /// cancellation, deadline, authority, tracing, or resource registration by
    /// using a context-free convenience path.
    pub fn call_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<NativeValue>,
    ) -> Result<NativeValue, ProviderError> {
        context.check_cancelled()?;
        (self.inner)(context, args)
    }
}

impl From<NativeHostFn> for NativeInterpreterFn {
    fn from(function: NativeHostFn) -> Self {
        Self::from_fn(function)
    }
}

pub type ProviderFuture =
    Pin<Box<dyn Future<Output = Result<NativeValue, ProviderError>> + Send + 'static>>;

type AsyncContextualProviderFn =
    dyn Fn(AsyncProviderCallContext, Vec<NativeValue>) -> ProviderFuture + Send + Sync;

#[derive(Clone)]
pub struct AsyncInterpreterFn {
    inner: Arc<AsyncContextualProviderFn>,
}

impl AsyncInterpreterFn {
    pub fn new<F, Fut>(function: F) -> Self
    where
        F: Fn(AsyncProviderCallContext, Vec<NativeValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<NativeValue, ProviderError>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |context, args| Box::pin(function(context, args))),
        }
    }

    pub fn call(
        &self,
        context: AsyncProviderCallContext,
        args: Vec<NativeValue>,
    ) -> ProviderFuture {
        (self.inner)(context, args)
    }
}

#[derive(Clone)]
pub enum ProviderCallable {
    Sync(NativeInterpreterFn),
    Async(AsyncInterpreterFn),
}

impl ProviderCallable {
    pub const fn call_mode(&self) -> ProviderCallMode {
        match self {
            Self::Sync(_) => ProviderCallMode::Sync,
            Self::Async(_) => ProviderCallMode::Async,
        }
    }
}

impl From<NativeInterpreterFn> for ProviderCallable {
    fn from(value: NativeInterpreterFn) -> Self {
        Self::Sync(value)
    }
}

impl From<AsyncInterpreterFn> for ProviderCallable {
    fn from(value: AsyncInterpreterFn) -> Self {
        Self::Async(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCallMode {
    Sync,
    Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockingBehavior {
    NonBlocking,
    MayBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationBehavior {
    NotApplicable,
    Cooperative,
    AbortSafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceCleanupContract {
    None,
    ProviderManaged,
    RuntimeRegistered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorMapping {
    StructuredV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFunctionDescriptor {
    pub symbol: ExternalSymbol,
    pub signature: FunctionSignature,
    pub entry: String,
    pub call_mode: ProviderCallMode,
    pub blocking: BlockingBehavior,
    pub cancellation: CancellationBehavior,
    pub thread_safe: bool,
    pub reentrant: bool,
    pub resource_cleanup: ResourceCleanupContract,
    pub error_mapping: ProviderErrorMapping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub provider_id: String,
    pub provider_version: String,
    pub supported_abi: Vec<u32>,
    pub functions: Vec<ProviderFunctionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInvocationContract {
    pub provider_id: String,
    pub provider_version: String,
    pub descriptor: ProviderFunctionDescriptor,
}

pub struct ProviderFunction<T> {
    pub signature: FunctionSignature,
    pub callable: T,
}

#[derive(Clone)]
pub struct ResolvedProviderFunction<T> {
    pub provider_id: String,
    pub provider_version: String,
    pub descriptor: ProviderFunctionDescriptor,
    pub callable: T,
}

pub struct ProviderRegistry<T> {
    runtime_abi: u32,
    functions: BTreeMap<ExternalSymbol, ResolvedProviderFunction<T>>,
}

impl<T> ProviderRegistry<T> {
    pub fn new(runtime_abi: u32) -> Self {
        Self {
            runtime_abi,
            functions: BTreeMap::new(),
        }
    }

    pub fn register_provider(
        &mut self,
        descriptor: &ProviderDescriptor,
        mut implementations: BTreeMap<ExternalSymbol, ProviderFunction<T>>,
    ) -> Result<(), ProviderLoadError> {
        if descriptor.provider_id.trim().is_empty() || descriptor.provider_version.trim().is_empty()
        {
            return Err(ProviderLoadError::InvalidProviderIdentity);
        }
        if !descriptor.supported_abi.contains(&self.runtime_abi) {
            return Err(ProviderLoadError::UnsupportedAbi {
                provider: descriptor.provider_id.clone(),
                runtime_abi: self.runtime_abi,
            });
        }

        let mut declared = BTreeSet::new();
        for function in &descriptor.functions {
            if !declared.insert(function.symbol.clone()) {
                return Err(ProviderLoadError::DuplicateDescriptorSymbol(
                    function.symbol.clone(),
                ));
            }
            if self.functions.contains_key(&function.symbol) {
                return Err(ProviderLoadError::DuplicateRegisteredSymbol(
                    function.symbol.clone(),
                ));
            }
            let implementation = implementations
                .remove(&function.symbol)
                .ok_or_else(|| ProviderLoadError::MissingImplementation(function.symbol.clone()))?;
            if implementation.signature.hash() != function.signature.hash() {
                return Err(ProviderLoadError::DescriptorSignatureMismatch(
                    function.symbol.clone(),
                ));
            }
            if (function.call_mode == ProviderCallMode::Async) != function.signature.asynchronous {
                return Err(ProviderLoadError::CallModeMismatch(function.symbol.clone()));
            }
            self.functions.insert(
                function.symbol.clone(),
                ResolvedProviderFunction {
                    provider_id: descriptor.provider_id.clone(),
                    provider_version: descriptor.provider_version.clone(),
                    descriptor: function.clone(),
                    callable: implementation.callable,
                },
            );
        }
        if let Some(symbol) = implementations.into_keys().next() {
            return Err(ProviderLoadError::UndeclaredImplementation(symbol));
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        import: &ExternalImport,
    ) -> Result<&ResolvedProviderFunction<T>, ProviderLoadError> {
        if import.abi_version != self.runtime_abi {
            return Err(ProviderLoadError::ImportAbiMismatch {
                symbol: import.symbol.clone(),
                import_abi: import.abi_version,
                runtime_abi: self.runtime_abi,
            });
        }
        let function = self
            .functions
            .get(&import.symbol)
            .ok_or_else(|| ProviderLoadError::UnresolvedImport(import.symbol.clone()))?;
        if function.descriptor.signature != import.signature
            || function.descriptor.signature.hash() != import.signature_hash
        {
            return Err(ProviderLoadError::ImportSignatureMismatch(
                import.symbol.clone(),
            ));
        }
        Ok(function)
    }

    pub fn into_resolved_functions(
        self,
    ) -> impl Iterator<Item = (ExternalSymbol, ResolvedProviderFunction<T>)> {
        self.functions.into_iter()
    }

    pub fn resolved_functions(
        &self,
    ) -> impl Iterator<Item = (&ExternalSymbol, &ResolvedProviderFunction<T>)> {
        self.functions.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderLoadError {
    InvalidProviderIdentity,
    UnsupportedAbi {
        provider: String,
        runtime_abi: u32,
    },
    DuplicateDescriptorSymbol(ExternalSymbol),
    DuplicateRegisteredSymbol(ExternalSymbol),
    MissingImplementation(ExternalSymbol),
    UndeclaredImplementation(ExternalSymbol),
    DescriptorSignatureMismatch(ExternalSymbol),
    UnresolvedImport(ExternalSymbol),
    ImportAbiMismatch {
        symbol: ExternalSymbol,
        import_abi: u32,
        runtime_abi: u32,
    },
    ImportSignatureMismatch(ExternalSymbol),
    CallModeMismatch(ExternalSymbol),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLoadErrorCode {
    InvalidProviderIdentity,
    UnsupportedAbi,
    DuplicateDescriptorSymbol,
    DuplicateRegisteredSymbol,
    MissingImplementation,
    UndeclaredImplementation,
    DescriptorSignatureMismatch,
    UnresolvedImport,
    ImportAbiMismatch,
    ImportSignatureMismatch,
    CallModeMismatch,
}

impl ProviderLoadError {
    pub fn code(&self) -> ProviderLoadErrorCode {
        match self {
            Self::InvalidProviderIdentity => ProviderLoadErrorCode::InvalidProviderIdentity,
            Self::UnsupportedAbi { .. } => ProviderLoadErrorCode::UnsupportedAbi,
            Self::DuplicateDescriptorSymbol(_) => ProviderLoadErrorCode::DuplicateDescriptorSymbol,
            Self::DuplicateRegisteredSymbol(_) => ProviderLoadErrorCode::DuplicateRegisteredSymbol,
            Self::MissingImplementation(_) => ProviderLoadErrorCode::MissingImplementation,
            Self::UndeclaredImplementation(_) => ProviderLoadErrorCode::UndeclaredImplementation,
            Self::DescriptorSignatureMismatch(_) => {
                ProviderLoadErrorCode::DescriptorSignatureMismatch
            }
            Self::UnresolvedImport(_) => ProviderLoadErrorCode::UnresolvedImport,
            Self::ImportAbiMismatch { .. } => ProviderLoadErrorCode::ImportAbiMismatch,
            Self::ImportSignatureMismatch(_) => ProviderLoadErrorCode::ImportSignatureMismatch,
            Self::CallModeMismatch(_) => ProviderLoadErrorCode::CallModeMismatch,
        }
    }
}

impl fmt::Display for ProviderLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RSScript provider load failed: {self:?}")
    }
}

impl Error for ProviderLoadError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_estimate_is_structural_and_deterministic() {
        let value = NativeValue::Struct {
            name: "Reply".into(),
            fields: BTreeMap::from([
                ("body".into(), NativeValue::Bytes(vec![1, 2, 3, 4])),
                ("ok".into(), NativeValue::Bool(true)),
            ]),
        };
        assert_eq!(value.estimated_payload_bytes(), 5 + 4 + 4 + 2 + 1);
        assert_eq!(estimated_payload_bytes(&[value.clone(), value]), 32);
    }
    use proptest::prelude::*;
    use rsscript_abi_model::{DataEffect, ParameterSignature};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn signature(effect: DataEffect) -> FunctionSignature {
        FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "value".to_string(),
                effect,
                ty: "Int".into(),
                retained: false,
            }],
            result: "Int".into(),
            asynchronous: false,
        }
    }

    fn descriptor(symbol: &ExternalSymbol, signature: FunctionSignature) -> ProviderDescriptor {
        ProviderDescriptor {
            provider_id: "test".to_string(),
            provider_version: "1.0.0".to_string(),
            supported_abi: vec![1],
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature,
                entry: "identity".to_string(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::NotApplicable,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        }
    }

    #[test]
    fn import_signature_mismatch_fails_before_resolution() {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let declared = signature(DataEffect::Read);
        let mut registry = ProviderRegistry::new(1);
        registry
            .register_provider(
                &descriptor(&symbol, declared.clone()),
                BTreeMap::from([(
                    symbol.clone(),
                    ProviderFunction {
                        signature: declared,
                        callable: 7,
                    },
                )]),
            )
            .unwrap();

        let import = ExternalImport {
            symbol,
            signature: signature(DataEffect::Take),
            signature_hash: signature(DataEffect::Take).hash(),
            abi_version: 1,
        };
        let Err(error) = registry.resolve(&import) else {
            panic!("mismatched import must fail")
        };
        assert!(matches!(
            error,
            ProviderLoadError::ImportSignatureMismatch(_)
        ));
        assert_eq!(error.code(), ProviderLoadErrorCode::ImportSignatureMismatch);
        assert_eq!(
            ProviderErrorCode::ResourceExhausted.as_str(),
            "resource_exhausted"
        );
    }

    #[test]
    fn resolved_function_retains_provider_and_behavior_metadata() {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let declared = signature(DataEffect::Read);
        let mut registry = ProviderRegistry::new(1);
        registry
            .register_provider(
                &descriptor(&symbol, declared.clone()),
                BTreeMap::from([(
                    symbol.clone(),
                    ProviderFunction {
                        signature: declared.clone(),
                        callable: 7,
                    },
                )]),
            )
            .unwrap();
        let resolved = registry
            .resolve(&ExternalImport {
                symbol,
                signature: declared.clone(),
                signature_hash: declared.hash(),
                abi_version: 1,
            })
            .unwrap();
        assert_eq!(resolved.provider_id, "test");
        assert_eq!(resolved.provider_version, "1.0.0");
        assert_eq!(
            resolved.descriptor.resource_cleanup,
            ResourceCleanupContract::None
        );
        assert_eq!(resolved.callable, 7);
    }

    #[test]
    fn contextual_callable_observes_cancellation_before_provider_code() {
        let called = Arc::new(AtomicBool::new(false));
        let called_by_provider = Arc::clone(&called);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let callable = NativeInterpreterFn::new_contextual(move |_, _| {
            called_by_provider.store(true, Ordering::Relaxed);
            Ok(NativeValue::Unit)
        });
        let mut context = ProviderCallContext {
            cancellation: Some(&cancelled),
            ..ProviderCallContext::default()
        };
        let error = callable
            .call_with_context(&mut context, vec![])
            .expect_err("cancelled call must not enter Provider code");
        assert_eq!(error.code, ProviderErrorCode::Cancelled);
        assert!(!called.load(Ordering::Relaxed));
    }

    #[test]
    fn resource_handles_are_generation_safe_and_cleanup_is_exactly_once() {
        struct CountedResource(Arc<std::sync::atomic::AtomicU64>);
        impl ProviderResource for CountedResource {
            fn cleanup(&mut self) -> Result<(), ProviderError> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
        }

        let cleanups = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut table = ProviderResourceTable::new(Some(1));
        let first = table
            .register(Box::new(CountedResource(Arc::clone(&cleanups))))
            .unwrap();
        assert!(matches!(
            table.register(Box::new(CountedResource(Arc::clone(&cleanups)))),
            Err(ProviderError {
                code: ProviderErrorCode::ResourceExhausted,
                ..
            })
        ));
        table.cleanup(first).unwrap();
        let second = table
            .register(Box::new(CountedResource(Arc::clone(&cleanups))))
            .unwrap();
        assert_eq!(first.slot, second.slot);
        assert_ne!(first.generation, second.generation);
        assert!(
            table.cleanup(first).is_err(),
            "stale handle must fail closed"
        );
        assert!(table.cleanup_all().is_empty());
        assert_eq!(cleanups.load(Ordering::Relaxed), 2);
        assert_eq!(table.live(), 0);
        assert_eq!(table.peak_live(), 1);
        assert_eq!(table.created(), 2);
        assert_eq!(table.cleaned(), 2);
        assert_eq!(table.cleanup_failures(), 0);
    }

    #[test]
    fn failed_resource_cleanup_is_not_reported_as_successful() {
        struct FailingResource;
        impl ProviderResource for FailingResource {
            fn cleanup(&mut self) -> Result<(), ProviderError> {
                Err(ProviderError::internal("cleanup failed"))
            }
        }

        let mut table = ProviderResourceTable::new(Some(1));
        let handle = table.register(Box::new(FailingResource)).unwrap();
        let error = table.cleanup(handle).expect_err("cleanup must fail");
        assert_eq!(error.code, ProviderErrorCode::Internal);
        assert_eq!(table.live(), 0);
        assert_eq!(table.created(), 1);
        assert_eq!(table.cleaned(), 0);
        assert_eq!(table.cleanup_failures(), 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn resource_table_preserves_generation_and_exact_cleanup_invariants(
            operations in prop::collection::vec(any::<u8>(), 1..256)
        ) {
            struct CountedResource(Arc<std::sync::atomic::AtomicU64>);
            impl ProviderResource for CountedResource {
                fn cleanup(&mut self) -> Result<(), ProviderError> {
                    self.0.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            }

            let cleanups = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let mut table = ProviderResourceTable::new(Some(8));
            let mut live = Vec::new();
            let mut stale = Vec::new();
            let mut created = 0u64;
            let mut peak_live = 0usize;

            for operation in operations {
                match operation % 4 {
                    0 | 1 => {
                        let result = table.register(Box::new(CountedResource(Arc::clone(&cleanups))));
                        if live.len() < 8 {
                            let handle = result.expect("registration below the limit must succeed");
                            prop_assert!(!live.contains(&handle));
                            live.push(handle);
                            peak_live = peak_live.max(live.len());
                            created += 1;
                        } else {
                            let exhausted = matches!(
                                result,
                                Err(ProviderError { code: ProviderErrorCode::ResourceExhausted, .. })
                            );
                            prop_assert!(exhausted);
                        }
                    }
                    2 if !live.is_empty() => {
                        let index = usize::from(operation) % live.len();
                        let handle = live.swap_remove(index);
                        table.cleanup(handle).expect("live handle cleanup must succeed");
                        stale.push(handle);
                    }
                    _ => {
                        prop_assert!(table.cleanup_all().is_empty());
                        stale.append(&mut live);
                    }
                }

                if !stale.is_empty() {
                    let handle = stale[usize::from(operation) % stale.len()];
                    prop_assert!(table.cleanup(handle).is_err(), "stale handle must fail closed");
                }
                prop_assert_eq!(table.live(), live.len());
                prop_assert_eq!(table.peak_live(), peak_live);
                prop_assert_eq!(table.created(), created);
                prop_assert_eq!(table.cleaned(), cleanups.load(Ordering::Relaxed));
                prop_assert_eq!(table.cleanup_failures(), 0);
            }

            drop(table);
            prop_assert_eq!(cleanups.load(Ordering::Relaxed), created);
        }
    }
}
