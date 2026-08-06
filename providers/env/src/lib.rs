#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: NativeInterpreterFn::new(|mut args| {
                let NativeValue::String(name) = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("name must be String"));
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
                    Err(error) => {
                        return Err(ProviderError::invalid_argument(format!(
                            "environment value is not valid Unicode: {error}"
                        )));
                    }
                })
            }),
        },
    )])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links() {
        let mut r =
            rsscript_provider_api::ProviderRegistry::new(rsscript_abi_model::RUNTIME_ABI_VERSION);
        r.register_provider(&descriptor(), functions()).unwrap();
    }
}
