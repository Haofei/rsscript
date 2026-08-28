#![forbid(unsafe_code)]
use rand::RngCore;
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{ProviderError, ProviderFunction, WireInterpreterFn, WireValue};
use std::collections::BTreeMap;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

/// Canonical wire implementation; entropy has only scalar boundary values.
pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: WireInterpreterFn::new(|args| {
                let [WireValue::Int { value: length }] = args.as_slice() else {
                    return Err(ProviderError::invalid_argument(
                        "bytes expects exactly one Int length",
                    ));
                };
                let length = usize::try_from(*length)
                    .map_err(|_| ProviderError::invalid_argument("length must be non-negative"))?;
                if length > 16 * 1024 * 1024 {
                    return Err(ProviderError::resource_exhausted(
                        "entropy request exceeds 16 MiB",
                    ));
                }
                let mut bytes = vec![0; length];
                rand::thread_rng().fill_bytes(&mut bytes);
                Ok(WireValue::Bytes { value: bytes })
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
            rsscript_provider_conformance::assert_wire_provider_conforms(descriptor(), functions());
        assert_eq!(report.provider_id, "rsscript.entropy");
    }
}
