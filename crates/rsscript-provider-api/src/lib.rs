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

#[derive(Debug, Clone, Copy)]
pub struct ProviderCallContext<'a> {
    pub cancellation: Option<&'a AtomicBool>,
    pub deadline: Option<Instant>,
    pub remaining_byte_budget: Option<usize>,
    pub remaining_output_budget: Option<usize>,
    pub call_id: u64,
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
        }
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
}
