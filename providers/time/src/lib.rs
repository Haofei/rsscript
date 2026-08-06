#![forbid(unsafe_code)]
use rsscript_abi_model::{ExternalSymbol, FunctionSignature, RUNTIME_ABI_VERSION};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderFunction, ProviderFunctionDescriptor,
};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.time.unix_ms").unwrap()
}
fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![],
        result: "Int".into(),
        asynchronous: false,
    }
}
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.time".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature: signature(),
            entry: "unix_ms".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: rsscript_provider_api::ResourceCleanupContract::None,
            error_mapping: rsscript_provider_api::ProviderErrorMapping::StructuredV1,
        }],
    }
}
pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    BTreeMap::from([(
        symbol(),
        ProviderFunction {
            signature: signature(),
            callable: NativeInterpreterFn::new(|args| {
                if !args.is_empty() {
                    return Err("unix_ms takes no arguments".into());
                }
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_millis();
                i64::try_from(millis)
                    .map(NativeValue::Int)
                    .map_err(|_| "clock value exceeds Int".into())
            }),
        },
    )])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links() {
        let mut r = rsscript_provider_api::ProviderRegistry::new(RUNTIME_ABI_VERSION);
        r.register_provider(&descriptor(), functions()).unwrap();
    }
}
