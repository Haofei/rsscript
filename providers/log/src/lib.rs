#![forbid(unsafe_code)]
use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, RUNTIME_ABI_VERSION,
};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderFunction, ProviderFunctionDescriptor,
};
use std::collections::BTreeMap;
use std::sync::Arc;
fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.log.emit").unwrap()
}
fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "message".into(),
            effect: DataEffect::Read,
            ty: "String".into(),
            retained: false,
        }],
        result: "Unit".into(),
        asynchronous: false,
    }
}
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.log".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature: signature(),
            entry: "emit".into(),
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
pub fn functions(
    sink: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static,
) -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let sink = Arc::new(sink);
    BTreeMap::from([(
        symbol(),
        ProviderFunction {
            signature: signature(),
            callable: NativeInterpreterFn::new(move |mut args| {
                let NativeValue::String(message) = args.remove(0) else {
                    return Err("message must be String".into());
                };
                sink(&message)?;
                Ok(NativeValue::Unit)
            }),
        },
    )])
}
pub fn stderr_functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    functions(|message| {
        eprintln!("{message}");
        Ok(())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links() {
        let mut r = rsscript_provider_api::ProviderRegistry::new(RUNTIME_ABI_VERSION);
        r.register_provider(&descriptor(), functions(|_| Ok(())))
            .unwrap();
    }
}
