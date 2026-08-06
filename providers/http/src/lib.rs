#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: NativeInterpreterFn::new(|mut values| {
                let NativeValue::String(url) = values.remove(0) else {
                    return Err(ProviderError::invalid_argument("url must be String"));
                };
                let response = reqwest::blocking::get(url)
                    .map_err(|error| ProviderError::unavailable(format!("HTTP GET: {error}")))?;
                let status = i64::from(response.status().as_u16());
                let body = response
                    .text()
                    .map_err(|error| ProviderError::unavailable(format!("HTTP body: {error}")))?;
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
        let mut registry =
            rsscript_provider_api::ProviderRegistry::new(rsscript_abi_model::RUNTIME_ABI_VERSION);
        registry
            .register_provider(&descriptor(), functions())
            .unwrap();
    }
}
