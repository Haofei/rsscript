#![forbid(unsafe_code)]

//! Reusable, side-effect-free preflight checks for RSScript Providers.
//!
//! The kit validates descriptor shape, registration, import resolution, and
//! the runtime-owned cancellation/deadline gate. Provider-specific behavior
//! still belongs in the Provider crate's own tests.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, CancellationToken, ExternalImport, ExternalSymbol,
    MonotonicDeadline, ProviderCallContext, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderFunction, ProviderLoadError, ProviderRegistry, RUNTIME_ABI_VERSION,
    ResourceCleanupContract, WireInterpreterFn,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConformanceReport {
    pub provider_id: String,
    pub functions_checked: usize,
    pub blocking_functions: usize,
    pub cancellable_functions: usize,
    pub resource_functions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConformanceError {
    EmptyDescriptor,
    EmptyEntry(ExternalSymbol),
    DuplicateEntry(String),
    DuplicateAbi(u32),
    EmptyParameterName(ExternalSymbol),
    DuplicateParameterName {
        symbol: ExternalSymbol,
        parameter: String,
    },
    Load(ProviderLoadError),
    CancellationPreflight {
        symbol: ExternalSymbol,
        observed: Option<ProviderErrorCode>,
    },
    DeadlinePreflight {
        symbol: ExternalSymbol,
        observed: Option<ProviderErrorCode>,
    },
}

impl fmt::Display for ProviderConformanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RSScript Provider conformance failed: {self:?}")
    }
}

impl Error for ProviderConformanceError {}

impl From<ProviderLoadError> for ProviderConformanceError {
    fn from(error: ProviderLoadError) -> Self {
        Self::Load(error)
    }
}

/// Check a canonical synchronous wire Provider against the same descriptor,
/// registry, cancellation, and deadline contracts required by RSScript.
pub fn check_wire_provider(
    descriptor: ProviderDescriptor,
    implementations: BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>>,
) -> Result<ProviderConformanceReport, ProviderConformanceError> {
    check_provider_inner(&descriptor, implementations, |callable, context| {
        callable.call_with_context(context, Vec::new()).map(|_| ())
    })?;
    Ok(conformance_report(&descriptor))
}

#[track_caller]
pub fn assert_wire_provider_conforms(
    descriptor: ProviderDescriptor,
    implementations: BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>>,
) -> ProviderConformanceReport {
    check_wire_provider(descriptor, implementations).unwrap_or_else(|error| panic!("{error}"))
}

fn check_provider_inner<T>(
    descriptor: &ProviderDescriptor,
    implementations: BTreeMap<ExternalSymbol, ProviderFunction<T>>,
    preflight: impl Fn(&T, &mut ProviderCallContext<'_>) -> Result<(), ProviderError>,
) -> Result<(), ProviderConformanceError> {
    check_descriptor_shape(descriptor)?;
    let mut registry = ProviderRegistry::new(RUNTIME_ABI_VERSION);
    registry.register_provider(descriptor, implementations)?;

    for function in &descriptor.functions {
        let import = ExternalImport {
            symbol: function.symbol.clone(),
            signature: function.signature.clone(),
            signature_hash: function.signature.hash(),
            abi_version: RUNTIME_ABI_VERSION,
        };
        let resolved = registry.resolve(&import)?;

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut cancelled_context = ProviderCallContext {
            cancellation: Some(&cancellation),
            blocking_allowed: true,
            async_allowed: true,
            ..ProviderCallContext::default()
        };
        let observed = preflight(&resolved.callable, &mut cancelled_context)
            .err()
            .map(|error| error.code);
        if observed != Some(ProviderErrorCode::Cancelled) {
            return Err(ProviderConformanceError::CancellationPreflight {
                symbol: function.symbol.clone(),
                observed,
            });
        }

        let mut expired_context = ProviderCallContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now()
                    .checked_sub(Duration::from_millis(1))
                    .expect("one millisecond before now is representable"),
            )),
            blocking_allowed: true,
            async_allowed: true,
            ..ProviderCallContext::default()
        };
        let observed = preflight(&resolved.callable, &mut expired_context)
            .err()
            .map(|error| error.code);
        if observed != Some(ProviderErrorCode::DeadlineExceeded) {
            return Err(ProviderConformanceError::DeadlinePreflight {
                symbol: function.symbol.clone(),
                observed,
            });
        }
    }
    Ok(())
}

fn check_descriptor_shape(descriptor: &ProviderDescriptor) -> Result<(), ProviderConformanceError> {
    if descriptor.functions.is_empty() {
        return Err(ProviderConformanceError::EmptyDescriptor);
    }
    let mut abi_versions = BTreeSet::new();
    for abi in &descriptor.supported_abi {
        if !abi_versions.insert(*abi) {
            return Err(ProviderConformanceError::DuplicateAbi(*abi));
        }
    }
    let mut entries = BTreeSet::new();
    for function in &descriptor.functions {
        if function.entry.trim().is_empty() {
            return Err(ProviderConformanceError::EmptyEntry(
                function.symbol.clone(),
            ));
        }
        if !entries.insert(function.entry.clone()) {
            return Err(ProviderConformanceError::DuplicateEntry(
                function.entry.clone(),
            ));
        }
        let mut parameters = BTreeSet::new();
        for parameter in &function.signature.parameters {
            if parameter.name.trim().is_empty() {
                return Err(ProviderConformanceError::EmptyParameterName(
                    function.symbol.clone(),
                ));
            }
            if !parameters.insert(parameter.name.clone()) {
                return Err(ProviderConformanceError::DuplicateParameterName {
                    symbol: function.symbol.clone(),
                    parameter: parameter.name.clone(),
                });
            }
        }
    }
    Ok(())
}

fn conformance_report(descriptor: &ProviderDescriptor) -> ProviderConformanceReport {
    ProviderConformanceReport {
        provider_id: descriptor.provider_id.clone(),
        functions_checked: descriptor.functions.len(),
        blocking_functions: descriptor
            .functions
            .iter()
            .filter(|function| function.blocking == BlockingBehavior::MayBlock)
            .count(),
        cancellable_functions: descriptor
            .functions
            .iter()
            .filter(|function| function.cancellation != CancellationBehavior::NotApplicable)
            .count(),
        resource_functions: descriptor
            .functions
            .iter()
            .filter(|function| function.resource_cleanup != ResourceCleanupContract::None)
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_provider_api::{
        DataEffect, FunctionSignature, ParameterSignature, ProviderCallMode, ProviderErrorMapping,
        ProviderFunctionDescriptor, WireInterpreterFn,
    };

    fn fixture() -> (
        ProviderDescriptor,
        BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>>,
    ) {
        let symbol = ExternalSymbol::new("host.test.identity").unwrap();
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "value".into(),
                effect: DataEffect::Read,
                ty: "Int".into(),
                retained: false,
            }],
            result: "Int".into(),
            asynchronous: false,
        };
        let descriptor = ProviderDescriptor {
            provider_id: "test".into(),
            provider_version: "1.0.0".into(),
            supported_abi: vec![RUNTIME_ABI_VERSION],
            record_layouts: Vec::new(),
            variant_layouts: Vec::new(),
            functions: vec![ProviderFunctionDescriptor {
                symbol: symbol.clone(),
                signature: signature.clone(),
                entry: "identity".into(),
                call_mode: ProviderCallMode::Sync,
                blocking: BlockingBehavior::NonBlocking,
                cancellation: CancellationBehavior::Cooperative,
                thread_safe: true,
                reentrant: true,
                resource_cleanup: ResourceCleanupContract::None,
                error_mapping: ProviderErrorMapping::StructuredV1,
            }],
        };
        let implementations = BTreeMap::from([(
            symbol,
            ProviderFunction {
                signature,
                callable: WireInterpreterFn::new(|mut arguments| Ok(arguments.remove(0))),
            },
        )]);
        (descriptor, implementations)
    }

    #[test]
    fn canonical_wire_provider_uses_the_same_preflight_contract() {
        let (descriptor, implementations) = fixture();
        let report = check_wire_provider(descriptor, implementations).unwrap();
        assert_eq!(report.provider_id, "test");
        assert_eq!(report.functions_checked, 1);
        assert_eq!(report.cancellable_functions, 1);
    }

    #[test]
    fn duplicate_entries_fail_closed() {
        let (mut descriptor, implementations) = fixture();
        descriptor.functions.push(descriptor.functions[0].clone());
        let error = check_wire_provider(descriptor, implementations).unwrap_err();
        assert_eq!(
            error,
            ProviderConformanceError::DuplicateEntry("identity".into())
        );
    }

    #[test]
    fn missing_implementation_is_reported_by_the_linker() {
        let (descriptor, _) = fixture();
        let error = check_wire_provider(descriptor, BTreeMap::new()).unwrap_err();
        assert!(matches!(
            error,
            ProviderConformanceError::Load(ProviderLoadError::MissingImplementation(_))
        ));
    }
}
