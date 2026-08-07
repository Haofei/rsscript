#![forbid(unsafe_code)]
use rand::RngCore;
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
                let NativeValue::Int(length) = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("length must be Int"));
                };
                let length = usize::try_from(length)
                    .map_err(|_| ProviderError::invalid_argument("length must be non-negative"))?;
                if length > 16 * 1024 * 1024 {
                    return Err(ProviderError::resource_exhausted(
                        "entropy request exceeds 16 MiB",
                    ));
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
    fn conforms_to_provider_contract() {
        let report =
            rsscript_provider_conformance::assert_provider_conforms(descriptor(), functions());
        assert_eq!(report.provider_id, "rsscript.entropy");
    }
}
