#![forbid(unsafe_code)]
use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, RUNTIME_ABI_VERSION,
};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderFunction, ProviderFunctionDescriptor,
};
use std::collections::BTreeMap;
pub fn descriptor() -> ProviderDescriptor {
    let signature = signature();
    ProviderDescriptor {
        provider_id: "rsscript.env".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature,
            entry: "get".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup_contract: "none".into(),
            error_mapping: "missing value maps to None".into(),
        }],
    }
}
pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    BTreeMap::from([(
        symbol(),
        ProviderFunction {
            signature: signature(),
            callable: NativeInterpreterFn::new(|mut args| {
                let NativeValue::String(name) = args.remove(0) else {
                    return Err("name must be String".into());
                };
                Ok(match std::env::var(name) {
                    Ok(value) => NativeValue::Variant {
                        name: "Some".into(),
                        fields: BTreeMap::from([("value".into(), NativeValue::String(value))]),
                    },
                    Err(std::env::VarError::NotPresent) => NativeValue::Variant {
                        name: "None".into(),
                        fields: BTreeMap::new(),
                    },
                    Err(error) => return Err(error.to_string()),
                })
            }),
        },
    )])
}
fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.env.get").unwrap()
}
fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "name".into(),
            effect: DataEffect::Read,
            type_name: "String".into(),
            retained: false,
        }],
        return_type: "Option<String>".into(),
        asynchronous: false,
    }
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
