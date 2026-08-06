#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub use rsscript_abi_model::{
    DataEffect, ExternalImport, ExternalSymbol, FunctionSignature, InvalidExternalSymbol,
    ParameterSignature, RUNTIME_ABI_VERSION, SignatureHash,
};
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
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl Error for ProviderError {}

impl From<String> for ProviderError {
    fn from(message: String) -> Self {
        Self::internal(message)
    }
}

impl From<&str> for ProviderError {
    fn from(message: &str) -> Self {
        Self::internal(message)
    }
}

pub struct ProviderCallContext<'a> {
    pub cancellation: Option<&'a AtomicBool>,
    pub deadline: Option<Instant>,
    pub remaining_byte_budget: Option<usize>,
    pub remaining_output_budget: Option<usize>,
    pub call_id: u64,
    pub resources: Option<&'a mut ProviderResourceTable>,
}

impl ProviderCallContext<'_> {
    pub fn check_cancelled(&self) -> Result<(), ProviderError> {
        if self
            .cancellation
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(ProviderError::new(
                ProviderErrorCode::Cancelled,
                "provider call cancelled",
            ));
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
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
        Self {
            cancellation: None,
            deadline: None,
            remaining_byte_budget: None,
            remaining_output_budget: None,
            call_id: 0,
            resources: None,
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
    created: u64,
    cleaned: u64,
}

impl ProviderResourceTable {
    pub fn new(limit: Option<usize>) -> Self {
        Self {
            slots: Vec::new(),
            limit,
            live: 0,
            created: 0,
            cleaned: 0,
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
        self.cleaned += 1;
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

    pub fn created(&self) -> u64 {
        self.created
    }

    pub fn cleaned(&self) -> u64 {
        self.cleaned
    }
}

impl Drop for ProviderResourceTable {
    fn drop(&mut self) {
        drop(self.cleanup_all());
    }
}

pub type NativeHostFn = fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError>;

/// Cloneable provider callable used by the runtime registry.
#[derive(Clone)]
pub struct NativeInterpreterFn {
    inner: Arc<
        dyn for<'a> Fn(
                &mut ProviderCallContext<'a>,
                Vec<NativeValue>,
            ) -> Result<NativeValue, ProviderError>
            + Send
            + Sync,
    >,
}

impl NativeInterpreterFn {
    pub fn from_fn<E>(function: fn(Vec<NativeValue>) -> Result<NativeValue, E>) -> Self
    where
        E: Into<ProviderError> + 'static,
    {
        Self::new(move |args| function(args).map_err(Into::into))
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

    pub fn call(&self, args: Vec<NativeValue>) -> Result<NativeValue, ProviderError> {
        self.call_with_context(&mut ProviderCallContext::default(), args)
    }

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

pub struct ProviderFunction<T> {
    pub signature: FunctionSignature,
    pub callable: T,
}

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
        if function.descriptor.signature.hash() != import.signature_hash {
            return Err(ProviderLoadError::ImportSignatureMismatch(
                import.symbol.clone(),
            ));
        }
        Ok(function)
    }

    pub fn into_functions(self) -> impl Iterator<Item = (ExternalSymbol, T)> {
        self.functions
            .into_iter()
            .map(|(symbol, function)| (symbol, function.callable))
    }

    pub fn functions(&self) -> impl Iterator<Item = (&ExternalSymbol, &T)> {
        self.functions
            .iter()
            .map(|(symbol, function)| (symbol, &function.callable))
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

impl fmt::Display for ProviderLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RSScript provider load failed: {self:?}")
    }
}

impl Error for ProviderLoadError {}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_abi_model::{DataEffect, ParameterSignature};
    use std::sync::atomic::AtomicBool;

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
            signature_hash: signature(DataEffect::Take).hash(),
            abi_version: 1,
        };
        assert!(matches!(
            registry.resolve(&import),
            Err(ProviderLoadError::ImportSignatureMismatch(_))
        ));
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
        let cancelled = AtomicBool::new(true);
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
        assert_eq!(table.created(), 2);
        assert_eq!(table.cleaned(), 2);
    }
}
