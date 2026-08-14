#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{ProviderError, ProviderFunction, WireInterpreterFn, WireValue};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

/// Canonical wire implementation of the scalar clock Provider.
pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: WireInterpreterFn::new(|args| {
                if !args.is_empty() {
                    return Err(ProviderError::invalid_argument(
                        "unix_ms takes no arguments",
                    ));
                }
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| {
                        ProviderError::unavailable(format!("system clock before epoch: {error}"))
                    })?
                    .as_millis();
                i64::try_from(millis)
                    .map(|value| WireValue::Int { value })
                    .map_err(|_| ProviderError::resource_exhausted("clock value exceeds Int"))
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
        assert_eq!(report.provider_id, "rsscript.time");
    }

    #[test]
    fn clock_returns_a_canonical_wire_integer() {
        let function = functions()
            .into_values()
            .next()
            .expect("time Provider has one function");
        let mut context = rsscript_provider_api::ProviderCallContext::default();
        let value = function
            .callable
            .call_with_context(&mut context, Vec::new())
            .expect("clock call succeeds");
        assert!(matches!(value, WireValue::Int { value } if value >= 0));
    }
}
