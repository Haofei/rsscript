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
    ParameterSignature, RUNTIME_ABI_VERSION, SignatureHash, WireCallTypeTable,
    WireRecordFieldLayout, WireRecordLayout, WireResourceHandle, WireResourceTypeId, WireType,
    WireTypeId, WireTypeTableOverflow, WireValue, WireVariantId, WireVariantLayout,
};
pub use rsscript_operation::{CancellationToken, MonotonicDeadline, OperationId};
use serde::{Deserialize, Serialize};

/// Legacy dynamic runtime value exchanged with register-VM and native ABI
/// compatibility adapters. New Provider implementations use [`WireValue`].
///
/// This model is deliberately absent from the default Provider API so an
/// ordinary Provider cannot accidentally make free-form type names, JSON, or
/// native IDs part of its contract.
#[cfg(feature = "compatibility")]
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

#[cfg(feature = "compatibility")]
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

#[cfg(feature = "compatibility")]
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

#[cfg(feature = "compatibility")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<WireValue>,
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
            details: Some(WireValue::String {
                value: format!("{:?}", error.kind()),
            }),
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

/// Host-constructed, instance-local context presented to Provider calls.
///
/// RSScript does not interpret these labels as an authorization policy. A
/// Provider may use them to select one of its already configured host views
/// (for example, one of several rooted filesystem views).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCallContext {
    labels: BTreeSet<String>,
}

impl HostCallContext {
    pub fn with_labels(labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            labels: labels.into_iter().map(Into::into).collect(),
        }
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.labels.contains(label)
    }

    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.labels.iter().map(String::as_str)
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
    pub host_context: &'a HostCallContext,
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
        static EMPTY_HOST_CONTEXT: std::sync::LazyLock<HostCallContext> =
            std::sync::LazyLock::new(HostCallContext::default);
        Self {
            cancellation: None,
            deadline: None,
            remaining_byte_budget: None,
            remaining_output_budget: None,
            call_id: OperationId(0),
            provider_id: String::new(),
            provider_version: String::new(),
            symbol: String::new(),
            host_context: &EMPTY_HOST_CONTEXT,
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

    /// Convert a runtime-owned handle to the canonical ABI representation.
    /// The resource type is supplied by the generated adapter/descriptor,
    /// rather than being inferred from a legacy native type-name string.
    pub const fn to_wire(self, resource_type: WireResourceTypeId) -> WireResourceHandle {
        WireResourceHandle {
            resource_type,
            slot: self.slot,
            generation: self.generation,
        }
    }

    /// Recover the runtime table identity from a canonical wire handle. The
    /// caller validates `resource_type` against its descriptor before looking
    /// it up in a table; this conversion intentionally does not discard that
    /// contract check by accepting a raw string type name.
    pub const fn from_wire(handle: WireResourceHandle) -> Self {
        Self {
            slot: handle.slot,
            generation: handle.generation,
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
    pub host_context: Arc<HostCallContext>,
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

#[cfg(feature = "compatibility")]
pub type NativeHostFn = fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError>;

/// Canonical Provider wire callable for new Provider implementations.
///
/// This is deliberately separate from [`NativeInterpreterFn`]: the latter is
/// the compatibility adapter used by the existing register VM. New generated
/// Provider adapters can accept structural [`WireValue`]s without reintroducing
/// names, JSON, or native IDs into their public call contract.
pub type WireHostFn = fn(Vec<WireValue>) -> Result<WireValue, ProviderError>;

type ContextualWireProviderFn = dyn for<'a> Fn(&mut ProviderCallContext<'a>, Vec<WireValue>) -> Result<WireValue, ProviderError>
    + Send
    + Sync;

#[derive(Clone)]
pub struct WireInterpreterFn {
    inner: Arc<ContextualWireProviderFn>,
}

impl WireInterpreterFn {
    pub fn from_fn(function: WireHostFn) -> Self {
        Self::new(function)
    }

    pub fn new(
        function: impl Fn(Vec<WireValue>) -> Result<WireValue, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        Self::new_contextual(move |_, args| function(args))
    }

    pub fn new_contextual(
        function: impl for<'a> Fn(
            &mut ProviderCallContext<'a>,
            Vec<WireValue>,
        ) -> Result<WireValue, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(function),
        }
    }

    pub fn call_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<WireValue>,
    ) -> Result<WireValue, ProviderError> {
        context.check_cancelled()?;
        (self.inner)(context, args)
    }
}

impl From<WireHostFn> for WireInterpreterFn {
    fn from(function: WireHostFn) -> Self {
        Self::from_fn(function)
    }
}

/// Canonical result of a Provider call with one or more `mut` parameters.
///
/// The values in `mutated` appear in the declaration order of the signature's
/// `mut` parameters. Keeping this distinct from a normal `WireValue` avoids
/// the legacy dynamic `List[result, mutated…]` envelope at the Provider ABI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireMutationResult {
    pub result: WireValue,
    pub mutated: Vec<WireValue>,
}

type ContextualWireMutationProviderFn = dyn for<'a> Fn(
        &mut ProviderCallContext<'a>,
        Vec<WireValue>,
    ) -> Result<WireMutationResult, ProviderError>
    + Send
    + Sync;

/// Canonical synchronous Provider callable for signatures with `mut`
/// parameters. New Provider implementations use this instead of constructing
/// a dynamic compatibility mutation envelope.
#[derive(Clone)]
pub struct WireMutationInterpreterFn {
    inner: Arc<ContextualWireMutationProviderFn>,
}

impl WireMutationInterpreterFn {
    pub fn new(
        function: impl Fn(Vec<WireValue>) -> Result<WireMutationResult, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::new_contextual(move |_, args| function(args))
    }

    pub fn new_contextual(
        function: impl for<'a> Fn(
            &mut ProviderCallContext<'a>,
            Vec<WireValue>,
        ) -> Result<WireMutationResult, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(function),
        }
    }

    pub fn call_with_context(
        &self,
        context: &mut ProviderCallContext<'_>,
        args: Vec<WireValue>,
    ) -> Result<WireMutationResult, ProviderError> {
        context.check_cancelled()?;
        (self.inner)(context, args)
    }
}

/// Whether a Provider operation may be replayed from a captured result.
///
/// Record/replay is opt-in diagnostic and test infrastructure. It neither
/// grants a Provider authority nor proves that an execution is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReplayability {
    Never,
    Deterministic,
}

/// The only request/response normalization accepted by the reference tape.
///
/// `WireValue` already carries canonical numeric type identity, so this avoids
/// reintroducing JSON or stringly typed normalizers into the replay contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReplayNormalization {
    CanonicalWireValueV1,
}

/// Controls whether a tape may retain call values.
///
/// Metadata-only recording intentionally cannot be replayed: a replayable
/// tape must have exact request and result values to fail closed on drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReplayRedaction {
    None,
    MetadataOnly,
}

/// Declares whether a call result depends on state outside its wire request.
///
/// The reference replayer only accepts `None`. Hosts may still record
/// metadata for a declared dependency, but must provide their own explicit
/// environment model before treating it as reproducible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProviderExternalState {
    None,
    Declared { description: String },
}

/// Retention rule for replay evidence.
///
/// The reference implementation stores values in memory only. It does not
/// serialize or persist a tape so that a caller cannot accidentally turn
/// potentially sensitive Provider inputs into an on-disk artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReplayPersistence {
    InMemoryOnly,
    HostManaged,
}

/// Explicit contract required before a canonical wire Provider can be wrapped
/// for deterministic record/replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReplayContract {
    pub replayability: ProviderReplayability,
    pub normalization: ProviderReplayNormalization,
    pub redaction: ProviderReplayRedaction,
    pub external_state: ProviderExternalState,
    pub persistence: ProviderReplayPersistence,
}

impl ProviderReplayContract {
    /// The strict, usable reference contract: deterministic calls with no
    /// external state, canonical values, and no persistence.
    pub const fn deterministic_in_memory() -> Self {
        Self {
            replayability: ProviderReplayability::Deterministic,
            normalization: ProviderReplayNormalization::CanonicalWireValueV1,
            redaction: ProviderReplayRedaction::None,
            external_state: ProviderExternalState::None,
            persistence: ProviderReplayPersistence::InMemoryOnly,
        }
    }

    fn validate_for_value_replay(&self) -> Result<(), ProviderError> {
        if self.replayability != ProviderReplayability::Deterministic {
            return Err(ProviderError::invalid_argument(
                "Provider replay requires an explicit deterministic contract",
            ));
        }
        if self.redaction != ProviderReplayRedaction::None {
            return Err(ProviderError::invalid_argument(
                "metadata-redacted Provider recordings cannot be replayed",
            ));
        }
        if self.external_state != ProviderExternalState::None {
            return Err(ProviderError::invalid_argument(
                "Provider calls with declared external state cannot use the reference replayer",
            ));
        }
        if self.persistence != ProviderReplayPersistence::InMemoryOnly {
            return Err(ProviderError::invalid_argument(
                "the reference Provider replay tape is in-memory only",
            ));
        }
        Ok(())
    }
}

/// Select whether a replay wrapper invokes a real Provider or consumes an
/// already-recorded canonical call sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReplayMode {
    Record,
    Replay,
}

/// One canonical Provider request/result pair captured by an in-memory tape.
///
/// Tapes intentionally have no `Serialize` implementation. A host that needs
/// persistence must build a separately reviewed, redacted transport instead
/// of accidentally serializing raw Provider arguments and results.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderReplayEntry {
    pub sequence: u64,
    pub symbol: ExternalSymbol,
    pub signature_hash: SignatureHash,
    pub request: Vec<WireValue>,
    pub result: Result<WireValue, ProviderError>,
}

#[derive(Default)]
struct ProviderReplayTapeState {
    entries: Vec<ProviderReplayEntry>,
    replay_cursor: usize,
}

/// Cloneable, in-memory-only canonical Provider replay tape.
#[derive(Clone, Default)]
pub struct ProviderReplayTape {
    inner: Arc<Mutex<ProviderReplayTapeState>>,
}

impl ProviderReplayTape {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot entries for assertions or an explicitly host-owned export.
    pub fn entries(&self) -> Result<Vec<ProviderReplayEntry>, ProviderError> {
        self.with_state(|state| Ok(state.entries.clone()))
    }

    /// Start replaying this tape from its first call. Recording never advances
    /// the cursor, so the same tape can be recorded then replayed directly.
    pub fn rewind(&self) -> Result<(), ProviderError> {
        self.with_state(|state| {
            state.replay_cursor = 0;
            Ok(())
        })
    }

    fn record(
        &self,
        symbol: ExternalSymbol,
        signature_hash: SignatureHash,
        request: Vec<WireValue>,
        result: Result<WireValue, ProviderError>,
    ) -> Result<(), ProviderError> {
        self.with_state(|state| {
            let sequence = u64::try_from(state.entries.len()).map_err(|_| {
                ProviderError::resource_exhausted("Provider replay tape exceeds sequence range")
            })?;
            state.entries.push(ProviderReplayEntry {
                sequence,
                symbol,
                signature_hash,
                request,
                result,
            });
            Ok(())
        })
    }

    fn replay_next(
        &self,
        symbol: &ExternalSymbol,
        signature_hash: SignatureHash,
        request: &[WireValue],
    ) -> Result<Result<WireValue, ProviderError>, ProviderError> {
        self.with_state(|state| {
            let Some(entry) = state.entries.get(state.replay_cursor) else {
                return Err(ProviderError::unavailable(
                    "Provider replay tape has no remaining recorded call",
                ));
            };
            if entry.symbol != *symbol || entry.signature_hash != signature_hash {
                return Err(ProviderError::invalid_argument(
                    "Provider replay call does not match the recorded symbol or signature",
                ));
            }
            if entry.request != request {
                return Err(ProviderError::invalid_argument(
                    "Provider replay call arguments do not match the recorded canonical request",
                ));
            }
            state.replay_cursor = state.replay_cursor.saturating_add(1);
            Ok(entry.result.clone())
        })
    }

    fn with_state<T>(
        &self,
        action: impl FnOnce(&mut ProviderReplayTapeState) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ProviderError::internal("Provider replay tape lock poisoned"))?;
        action(&mut state)
    }
}

/// Wrap a canonical synchronous Provider callable with explicit deterministic
/// record or replay behavior.
///
/// The wrapper records or checks the linked external symbol, semantic
/// signature hash, and exact canonical arguments. It never falls back to the
/// real Provider after a replay mismatch. Use only around a descriptor that
/// will subsequently be registered through the normal fail-closed registry.
pub fn replayable_wire_callable(
    descriptor: &ProviderFunctionDescriptor,
    callable: WireInterpreterFn,
    tape: ProviderReplayTape,
    mode: ProviderReplayMode,
    contract: ProviderReplayContract,
) -> Result<WireInterpreterFn, ProviderError> {
    contract.validate_for_value_replay()?;
    let symbol = descriptor.symbol.clone();
    let signature_hash = descriptor.signature.hash();
    Ok(match mode {
        ProviderReplayMode::Record => WireInterpreterFn::new_contextual(move |context, request| {
            let result = callable.call_with_context(context, request.clone());
            tape.record(
                symbol.clone(),
                signature_hash.clone(),
                request,
                result.clone(),
            )?;
            result
        }),
        ProviderReplayMode::Replay => WireInterpreterFn::new_contextual(move |_, request| {
            tape.replay_next(&symbol, signature_hash.clone(), &request)?
        }),
    })
}

/// Wrap a canonical asynchronous Provider callable with the same strict
/// in-memory record/replay contract as [`replayable_wire_callable`].
///
/// Replay does not invoke the original future and never falls back after a
/// mismatch. Runtime cancellation and deadline observation remain owned by
/// the normal asynchronous Provider dispatcher.
pub fn replayable_async_wire_callable(
    descriptor: &ProviderFunctionDescriptor,
    callable: AsyncWireInterpreterFn,
    tape: ProviderReplayTape,
    mode: ProviderReplayMode,
    contract: ProviderReplayContract,
) -> Result<AsyncWireInterpreterFn, ProviderError> {
    contract.validate_for_value_replay()?;
    let symbol = descriptor.symbol.clone();
    let signature_hash = descriptor.signature.hash();
    Ok(match mode {
        ProviderReplayMode::Record => AsyncWireInterpreterFn::new(move |context, request| {
            let future = callable.call(context, request.clone());
            let tape = tape.clone();
            let symbol = symbol.clone();
            let signature_hash = signature_hash.clone();
            async move {
                let result = future.await;
                tape.record(symbol, signature_hash, request, result.clone())?;
                result
            }
        }),
        ProviderReplayMode::Replay => AsyncWireInterpreterFn::new(move |_, request| {
            let tape = tape.clone();
            let symbol = symbol.clone();
            let signature_hash = signature_hash.clone();
            async move { tape.replay_next(&symbol, signature_hash, &request)? }
        }),
    })
}

/// Cloneable provider callable used by the runtime registry.
#[cfg(feature = "compatibility")]
type ContextualProviderFn = dyn for<'a> Fn(&mut ProviderCallContext<'a>, Vec<NativeValue>) -> Result<NativeValue, ProviderError>
    + Send
    + Sync;

#[cfg(feature = "compatibility")]
#[derive(Clone)]
pub struct NativeInterpreterFn {
    inner: Arc<ContextualProviderFn>,
}

#[cfg(feature = "compatibility")]
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

#[cfg(feature = "compatibility")]
impl From<NativeHostFn> for NativeInterpreterFn {
    fn from(function: NativeHostFn) -> Self {
        Self::from_fn(function)
    }
}

#[cfg(feature = "compatibility")]
pub type ProviderFuture =
    Pin<Box<dyn Future<Output = Result<NativeValue, ProviderError>> + Send + 'static>>;

/// Canonical asynchronous Provider wire callable result. It is separate from
/// [`ProviderFuture`] so new asynchronous Providers do not need to publish the
/// legacy dynamic value model merely to suspend.
pub type WireProviderFuture =
    Pin<Box<dyn Future<Output = Result<WireValue, ProviderError>> + Send + 'static>>;

/// Canonical asynchronous Provider mutation result. It keeps mutation
/// write-back structurally separate from an ordinary async wire result.
pub type WireMutationProviderFuture =
    Pin<Box<dyn Future<Output = Result<WireMutationResult, ProviderError>> + Send + 'static>>;

#[cfg(feature = "compatibility")]
type AsyncContextualProviderFn =
    dyn Fn(AsyncProviderCallContext, Vec<NativeValue>) -> ProviderFuture + Send + Sync;

#[cfg(feature = "compatibility")]
#[derive(Clone)]
pub struct AsyncInterpreterFn {
    inner: Arc<AsyncContextualProviderFn>,
}

#[cfg(feature = "compatibility")]
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

type AsyncContextualWireProviderFn =
    dyn Fn(AsyncProviderCallContext, Vec<WireValue>) -> WireProviderFuture + Send + Sync;

/// Canonical asynchronous Provider callable for descriptor-linked wire values.
#[derive(Clone)]
pub struct AsyncWireInterpreterFn {
    inner: Arc<AsyncContextualWireProviderFn>,
}

impl AsyncWireInterpreterFn {
    pub fn new<F, Fut>(function: F) -> Self
    where
        F: Fn(AsyncProviderCallContext, Vec<WireValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<WireValue, ProviderError>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |context, args| Box::pin(function(context, args))),
        }
    }

    pub fn call(
        &self,
        context: AsyncProviderCallContext,
        args: Vec<WireValue>,
    ) -> WireProviderFuture {
        (self.inner)(context, args)
    }
}

type AsyncContextualWireMutationProviderFn =
    dyn Fn(AsyncProviderCallContext, Vec<WireValue>) -> WireMutationProviderFuture + Send + Sync;

/// Canonical asynchronous Provider callable for signatures with `mut`
/// parameters. The completed value contains explicit canonical write-backs.
#[derive(Clone)]
pub struct AsyncWireMutationInterpreterFn {
    inner: Arc<AsyncContextualWireMutationProviderFn>,
}

impl AsyncWireMutationInterpreterFn {
    pub fn new<F, Fut>(function: F) -> Self
    where
        F: Fn(AsyncProviderCallContext, Vec<WireValue>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<WireMutationResult, ProviderError>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |context, args| Box::pin(function(context, args))),
        }
    }

    pub fn call(
        &self,
        context: AsyncProviderCallContext,
        args: Vec<WireValue>,
    ) -> WireMutationProviderFuture {
        (self.inner)(context, args)
    }
}

#[derive(Clone)]
pub enum ProviderCallable {
    #[cfg(feature = "compatibility")]
    Sync(NativeInterpreterFn),
    /// Canonical typed wire callable for descriptor-linked synchronous calls.
    WireSync(WireInterpreterFn),
    /// Canonical typed wire callable for synchronous signatures with `mut`
    /// parameters. Its result carries explicit write-back values.
    WireSyncMut(WireMutationInterpreterFn),
    #[cfg(feature = "compatibility")]
    Async(AsyncInterpreterFn),
    /// Canonical typed wire callable for descriptor-linked asynchronous calls.
    WireAsync(AsyncWireInterpreterFn),
    /// Canonical typed wire callable for asynchronous signatures with `mut`
    /// parameters and explicit write-back values.
    WireAsyncMut(AsyncWireMutationInterpreterFn),
}

impl ProviderCallable {
    pub const fn call_mode(&self) -> ProviderCallMode {
        match self {
            #[cfg(feature = "compatibility")]
            Self::Sync(_) => ProviderCallMode::Sync,
            Self::WireSync(_) => ProviderCallMode::Sync,
            Self::WireSyncMut(_) => ProviderCallMode::Sync,
            #[cfg(feature = "compatibility")]
            Self::Async(_) => ProviderCallMode::Async,
            Self::WireAsync(_) => ProviderCallMode::Async,
            Self::WireAsyncMut(_) => ProviderCallMode::Async,
        }
    }
}

#[cfg(feature = "compatibility")]
impl From<NativeInterpreterFn> for ProviderCallable {
    fn from(value: NativeInterpreterFn) -> Self {
        Self::Sync(value)
    }
}

#[cfg(feature = "compatibility")]
impl From<AsyncInterpreterFn> for ProviderCallable {
    fn from(value: AsyncInterpreterFn) -> Self {
        Self::Async(value)
    }
}

impl From<AsyncWireInterpreterFn> for ProviderCallable {
    fn from(value: AsyncWireInterpreterFn) -> Self {
        Self::WireAsync(value)
    }
}

impl From<AsyncWireMutationInterpreterFn> for ProviderCallable {
    fn from(value: AsyncWireMutationInterpreterFn) -> Self {
        Self::WireAsyncMut(value)
    }
}

impl From<WireInterpreterFn> for ProviderCallable {
    fn from(value: WireInterpreterFn) -> Self {
        Self::WireSync(value)
    }
}

impl From<WireMutationInterpreterFn> for ProviderCallable {
    fn from(value: WireMutationInterpreterFn) -> Self {
        Self::WireSyncMut(value)
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
    /// Named record layouts supplied by the interface descriptor. Wire calls
    /// use them to decode positional records without reintroducing dynamic
    /// field or type identity into [`WireValue`].
    #[serde(default)]
    pub record_layouts: Vec<WireRecordLayout>,
    /// Public sum layouts supplied by the interface descriptor. The caller
    /// resolves cases through declaration-order IDs, never Provider text.
    #[serde(default)]
    pub variant_layouts: Vec<WireVariantLayout>,
    pub functions: Vec<ProviderFunctionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInvocationContract {
    pub provider_id: String,
    pub provider_version: String,
    pub record_layouts: Vec<WireRecordLayout>,
    pub variant_layouts: Vec<WireVariantLayout>,
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
    pub record_layouts: Vec<WireRecordLayout>,
    pub variant_layouts: Vec<WireVariantLayout>,
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
        validate_record_layouts(descriptor)?;
        validate_variant_layouts(descriptor)?;

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
                    record_layouts: descriptor.record_layouts.clone(),
                    variant_layouts: descriptor.variant_layouts.clone(),
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

fn validate_record_layouts(descriptor: &ProviderDescriptor) -> Result<(), ProviderLoadError> {
    let mut records = BTreeSet::new();
    for record in &descriptor.record_layouts {
        if !matches!(record.ty, WireType::Named { .. }) {
            return Err(ProviderLoadError::InvalidRecordLayout(record.ty.clone()));
        }
        if !records.insert(record.ty.clone()) {
            return Err(ProviderLoadError::DuplicateRecordLayout(record.ty.clone()));
        }
        let mut fields = BTreeSet::new();
        for field in &record.fields {
            if field.name.trim().is_empty() || !fields.insert(field.name.clone()) {
                return Err(ProviderLoadError::InvalidRecordField {
                    record: record.ty.clone(),
                    field: field.name.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_variant_layouts(descriptor: &ProviderDescriptor) -> Result<(), ProviderLoadError> {
    let mut types = BTreeSet::new();
    for layout in &descriptor.variant_layouts {
        if !matches!(layout.ty, WireType::Named { .. }) || !types.insert(layout.ty.clone()) {
            return Err(ProviderLoadError::InvalidVariantLayout(layout.ty.clone()));
        }
        let mut variants = BTreeSet::new();
        for variant in &layout.variants {
            if variant.name.trim().is_empty() || !variants.insert(variant.name.clone()) {
                return Err(ProviderLoadError::InvalidVariantLayout(layout.ty.clone()));
            }
            let mut fields = BTreeSet::new();
            if variant
                .fields
                .iter()
                .any(|field| field.name.trim().is_empty() || !fields.insert(field.name.clone()))
            {
                return Err(ProviderLoadError::InvalidVariantLayout(layout.ty.clone()));
            }
        }
    }
    Ok(())
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
    InvalidRecordLayout(WireType),
    DuplicateRecordLayout(WireType),
    InvalidRecordField {
        record: WireType,
        field: String,
    },
    InvalidVariantLayout(WireType),
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
    InvalidRecordLayout,
    DuplicateRecordLayout,
    InvalidRecordField,
    InvalidVariantLayout,
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
            Self::InvalidRecordLayout(_) => ProviderLoadErrorCode::InvalidRecordLayout,
            Self::DuplicateRecordLayout(_) => ProviderLoadErrorCode::DuplicateRecordLayout,
            Self::InvalidRecordField { .. } => ProviderLoadErrorCode::InvalidRecordField,
            Self::InvalidVariantLayout(_) => ProviderLoadErrorCode::InvalidVariantLayout,
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

    #[cfg(feature = "compatibility")]
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

    #[test]
    fn provider_error_details_use_the_canonical_wire_value_model() {
        let error = ProviderError::from_io(
            "read fixture",
            std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        );
        assert_eq!(
            error.details,
            Some(WireValue::String {
                value: "NotFound".to_owned(),
            })
        );
        let encoded = serde_json::to_value(&error).expect("Provider error serializes");
        assert_eq!(encoded["details"]["kind"], "string");
        assert_eq!(encoded["details"]["value"], "NotFound");
    }
    use proptest::prelude::*;
    use rsscript_abi_model::{DataEffect, ParameterSignature};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};

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
            record_layouts: Vec::new(),
            variant_layouts: Vec::new(),
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

    fn async_context() -> AsyncProviderCallContext {
        AsyncProviderCallContext {
            cancellation: None,
            deadline: None,
            remaining_byte_budget: None,
            remaining_output_budget: None,
            call_id: OperationId(0),
            provider_id: String::new(),
            provider_version: String::new(),
            symbol: String::new(),
            host_context: Arc::new(HostCallContext::default()),
            trace: None,
            resources: None,
        }
    }

    fn poll_wire_future(future: &mut WireProviderFuture) -> Poll<Result<WireValue, ProviderError>> {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        future.as_mut().poll(&mut context)
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
    fn malformed_record_layouts_fail_before_provider_registration() {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let declared = signature(DataEffect::Read);
        let mut descriptor = descriptor(&symbol, declared.clone());
        let record = WireRecordLayout {
            ty: WireType::from("host.test.Response"),
            fields: vec![WireRecordFieldLayout {
                name: "value".into(),
                ty: WireType::Int {
                    bits: 64,
                    signed: true,
                },
            }],
        };
        descriptor.record_layouts = vec![record.clone(), record];
        let error = ProviderRegistry::new(1)
            .register_provider(
                &descriptor,
                BTreeMap::from([(
                    symbol,
                    ProviderFunction {
                        signature: declared,
                        callable: 7,
                    },
                )]),
            )
            .expect_err("duplicate record layouts must fail before a provider is registered");
        assert!(matches!(error, ProviderLoadError::DuplicateRecordLayout(_)));
        assert_eq!(error.code(), ProviderLoadErrorCode::DuplicateRecordLayout);
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

    #[cfg(feature = "compatibility")]
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
    fn wire_callable_uses_the_same_runtime_cancellation_gate() {
        let called = Arc::new(AtomicBool::new(false));
        let called_by_provider = Arc::clone(&called);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let callable = WireInterpreterFn::new_contextual(move |_, _| {
            called_by_provider.store(true, Ordering::Relaxed);
            Ok(WireValue::Unit)
        });
        let mut context = ProviderCallContext {
            cancellation: Some(&cancelled),
            ..ProviderCallContext::default()
        };
        let error = callable
            .call_with_context(&mut context, vec![])
            .expect_err("cancelled wire call must not enter Provider code");
        assert_eq!(error.code, ProviderErrorCode::Cancelled);
        assert!(!called.load(Ordering::Relaxed));
    }

    #[test]
    fn deterministic_wire_replay_uses_exact_canonical_calls_without_reinvoking_provider() {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let descriptor = descriptor(&symbol, signature(DataEffect::Read));
        let function = descriptor.functions.first().expect("one function");
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let calls_by_provider = Arc::clone(&calls);
        let tape = ProviderReplayTape::new();
        let recorded = replayable_wire_callable(
            function,
            WireInterpreterFn::new(move |values| {
                calls_by_provider.fetch_add(1, Ordering::SeqCst);
                Ok(values.into_iter().next().expect("one argument"))
            }),
            tape.clone(),
            ProviderReplayMode::Record,
            ProviderReplayContract::deterministic_in_memory(),
        )
        .expect("deterministic contract is accepted");
        let request = vec![WireValue::Int { value: 41 }];
        assert_eq!(
            recorded
                .call_with_context(&mut ProviderCallContext::default(), request.clone())
                .unwrap(),
            WireValue::Int { value: 41 }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(tape.entries().unwrap().len(), 1);

        let replayed = replayable_wire_callable(
            function,
            WireInterpreterFn::new(|_| -> Result<WireValue, ProviderError> {
                panic!("a replayed call must not invoke the real Provider")
            }),
            tape,
            ProviderReplayMode::Replay,
            ProviderReplayContract::deterministic_in_memory(),
        )
        .expect("replay contract is accepted");
        assert_eq!(
            replayed
                .call_with_context(&mut ProviderCallContext::default(), request)
                .unwrap(),
            WireValue::Int { value: 41 }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn deterministic_wire_replay_fails_closed_for_drift_and_non_replayable_contracts() {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let descriptor = descriptor(&symbol, signature(DataEffect::Read));
        let function = descriptor.functions.first().expect("one function");
        let tape = ProviderReplayTape::new();
        let recorded = replayable_wire_callable(
            function,
            WireInterpreterFn::new(|values| {
                Ok(values.into_iter().next().unwrap_or(WireValue::Unit))
            }),
            tape.clone(),
            ProviderReplayMode::Record,
            ProviderReplayContract::deterministic_in_memory(),
        )
        .unwrap();
        recorded
            .call_with_context(
                &mut ProviderCallContext::default(),
                vec![WireValue::Int { value: 7 }],
            )
            .unwrap();
        let replayed = replayable_wire_callable(
            function,
            WireInterpreterFn::new(|_| Ok(WireValue::Unit)),
            tape.clone(),
            ProviderReplayMode::Replay,
            ProviderReplayContract::deterministic_in_memory(),
        )
        .unwrap();
        let mismatch = replayed
            .call_with_context(
                &mut ProviderCallContext::default(),
                vec![WireValue::Int { value: 8 }],
            )
            .expect_err("argument drift must not fall through to the Provider");
        assert_eq!(mismatch.code, ProviderErrorCode::InvalidArgument);
        // A rejected mismatch cannot consume the recorded call.
        assert_eq!(
            replayed
                .call_with_context(
                    &mut ProviderCallContext::default(),
                    vec![WireValue::Int { value: 7 }],
                )
                .unwrap(),
            WireValue::Int { value: 7 }
        );

        let non_replayable = ProviderReplayContract {
            replayability: ProviderReplayability::Never,
            ..ProviderReplayContract::deterministic_in_memory()
        };
        let error = match replayable_wire_callable(
            function,
            WireInterpreterFn::new(|_| Ok(WireValue::Unit)),
            tape,
            ProviderReplayMode::Record,
            non_replayable,
        ) {
            Ok(_) => panic!("providers must explicitly opt into deterministic replay"),
            Err(error) => error,
        };
        assert_eq!(error.code, ProviderErrorCode::InvalidArgument);
    }

    #[test]
    fn deterministic_async_wire_replay_skips_the_real_future() {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let mut descriptor = descriptor(&symbol, signature(DataEffect::Read));
        descriptor.functions[0].call_mode = ProviderCallMode::Async;
        descriptor.functions[0].signature.asynchronous = true;
        let function = descriptor.functions.first().expect("one function");
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let calls_by_provider = Arc::clone(&calls);
        let tape = ProviderReplayTape::new();
        let recorded = replayable_async_wire_callable(
            function,
            AsyncWireInterpreterFn::new(move |_, values| {
                let calls = Arc::clone(&calls_by_provider);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(values.into_iter().next().expect("one argument"))
                }
            }),
            tape.clone(),
            ProviderReplayMode::Record,
            ProviderReplayContract::deterministic_in_memory(),
        )
        .unwrap();
        let mut recorded_future =
            recorded.call(async_context(), vec![WireValue::Int { value: 17 }]);
        assert_eq!(
            poll_wire_future(&mut recorded_future),
            Poll::Ready(Ok(WireValue::Int { value: 17 }))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let replayed = replayable_async_wire_callable(
            function,
            AsyncWireInterpreterFn::new(|_, _| async move {
                panic!("a replayed async call must not invoke the real Provider")
            }),
            tape,
            ProviderReplayMode::Replay,
            ProviderReplayContract::deterministic_in_memory(),
        )
        .unwrap();
        let mut replayed_future =
            replayed.call(async_context(), vec![WireValue::Int { value: 17 }]);
        assert_eq!(
            poll_wire_future(&mut replayed_future),
            Poll::Ready(Ok(WireValue::Int { value: 17 }))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
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
    fn resource_handles_cross_the_wire_without_legacy_type_strings() {
        let handle = ResourceHandle {
            slot: 12,
            generation: 34,
        };
        let wire = handle.to_wire(WireResourceTypeId::new(56));
        assert_eq!(wire.resource_type, WireResourceTypeId::new(56));
        assert_eq!(wire.slot, 12);
        assert_eq!(wire.generation, 34);
        assert_eq!(ResourceHandle::from_wire(wire), handle);
        assert!(
            serde_json::to_string(&wire)
                .expect("wire handle serializes")
                .contains("resource_type")
        );
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
