#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, RUNTIME_ABI_VERSION,
};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderFunction, ProviderFunctionDescriptor,
};

fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.http.get").unwrap()
}

fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "url".into(),
            effect: DataEffect::Read,
            type_name: "String".into(),
            retained: false,
        }],
        return_type: "HttpResponse".into(),
        asynchronous: false,
    }
}

pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.http".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature: signature(),
            entry: "get".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::MayBlock,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup_contract: "response body is consumed before return".into(),
            error_mapping: "transport and body errors".into(),
        }],
    }
}

pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    BTreeMap::from([(
        symbol(),
        ProviderFunction {
            signature: signature(),
            callable: NativeInterpreterFn::new(|mut values| {
                let NativeValue::String(url) = values.remove(0) else {
                    return Err("url must be String".into());
                };
                let response = reqwest::blocking::get(url).map_err(|error| error.to_string())?;
                let status = i64::from(response.status().as_u16());
                let body = response.text().map_err(|error| error.to_string())?;
                Ok(NativeValue::Struct {
                    name: "HttpResponse".into(),
                    fields: BTreeMap::from([
                        ("status".into(), NativeValue::Int(status)),
                        ("body".into(), NativeValue::String(body)),
                    ]),
                })
            }),
        },
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_and_implementation_link_without_network_access() {
        let mut registry = rsscript_provider_api::ProviderRegistry::new(RUNTIME_ABI_VERSION);
        registry
            .register_provider(&descriptor(), functions())
            .unwrap();
    }
}
