#![forbid(unsafe_code)]
use rand::RngCore;
use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, RUNTIME_ABI_VERSION,
};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderFunction, ProviderFunctionDescriptor,
};
use std::collections::BTreeMap;
fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.entropy.bytes").unwrap()
}
fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "length".into(),
            effect: DataEffect::Read,
            ty: "Int".into(),
            retained: false,
        }],
        result: "Bytes".into(),
        asynchronous: false,
    }
}
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.entropy".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature: signature(),
            entry: "bytes".into(),
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
            callable: NativeInterpreterFn::new(|mut args| {
                let NativeValue::Int(length) = args.remove(0) else {
                    return Err("length must be Int".into());
                };
                let length = usize::try_from(length).map_err(|_| "length must be non-negative")?;
                if length > 16 * 1024 * 1024 {
                    return Err("entropy request exceeds 16 MiB".into());
                }
                let mut bytes = vec![0; length];
                rand::thread_rng().fill_bytes(&mut bytes);
                Ok(NativeValue::Bytes(bytes))
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
