#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    ProviderError, ProviderFunction, WireCallTypeTable, WireInterpreterFn, WireType, WireValue,
};
use std::collections::BTreeMap;
use std::sync::Arc;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions(
    args: Vec<String>,
) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
    let args = Arc::new(args);
    let function = descriptor().functions.into_iter().next().unwrap();
    let types = WireCallTypeTable::for_signature(&function.signature)
        .expect("generated CLI interface must fit the wire type table");
    let WireType::List { element } = &function.signature.result else {
        unreachable!("generated CLI interface must return a list");
    };
    let element_type = types
        .type_id(element)
        .expect("generated CLI element type must be present in the wire type table");
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: WireInterpreterFn::new(move |values| {
                if !values.is_empty() {
                    return Err(ProviderError::invalid_argument("args takes no arguments"));
                }
                Ok(WireValue::List {
                    element_type,
                    values: args
                        .iter()
                        .cloned()
                        .map(|value| WireValue::String { value })
                        .collect(),
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
        let report = rsscript_provider_conformance::assert_wire_provider_conforms(
            descriptor(),
            functions(Vec::new()),
        );
        assert_eq!(report.provider_id, "rsscript.cli");
    }
}
