#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};
use std::collections::BTreeMap;
use std::sync::Arc;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions(
    args: Vec<String>,
) -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let args = Arc::new(args);
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: NativeInterpreterFn::new(move |values| {
                if !values.is_empty() {
                    return Err(ProviderError::invalid_argument("args takes no arguments"));
                }
                Ok(NativeValue::List(
                    args.iter().cloned().map(NativeValue::String).collect(),
                ))
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
        r.register_provider(&descriptor(), functions(vec![]))
            .unwrap();
    }
}
