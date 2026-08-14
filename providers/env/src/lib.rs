#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    ProviderError, ProviderFunction, WireCallTypeTable, WireInterpreterFn, WireType, WireValue,
};
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    let types = WireCallTypeTable::for_signature(&function.signature)
        .expect("generated environment interface must fit the wire type table");
    let option_type = types
        .type_id(&function.signature.result)
        .expect("generated environment result must be present in the wire type table");
    let WireType::Option { .. } = &function.signature.result else {
        unreachable!("generated environment interface must return an option");
    };
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: WireInterpreterFn::new(move |mut args| {
                let WireValue::String { value: name } = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("name must be String"));
                };
                Ok(match std::env::var(name) {
                    Ok(value) => WireValue::Variant {
                        type_id: option_type,
                        variant_id: WireCallTypeTable::option_some_variant(),
                        payload: Some(Box::new(WireValue::String { value })),
                    },
                    Err(std::env::VarError::NotPresent) => WireValue::Variant {
                        type_id: option_type,
                        variant_id: WireCallTypeTable::option_none_variant(),
                        payload: None,
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
    fn conforms_to_provider_contract() {
        let report =
            rsscript_provider_conformance::assert_wire_provider_conforms(descriptor(), functions());
        assert_eq!(report.provider_id, "rsscript.env");
    }
}
