#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    ProviderError, ProviderFunction, WireCallTypeTable, WireInterpreterFn, WireType, WireValue,
};
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

/// A captured, host-owned view of environment data.
///
/// Constructing the provider copies only the values the host deliberately
/// exposes. Provider calls never consult the ambient process environment, so
/// later process-wide mutations cannot widen script authority or make runs
/// nondeterministic.
#[derive(Clone, Debug, Default)]
pub struct CapturedEnvProvider {
    values: BTreeMap<String, String>,
}

impl CapturedEnvProvider {
    pub fn new(values: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }

    /// Capture an explicit allowlist from the process environment.
    ///
    /// Missing and non-Unicode values are omitted. The allowlist is evaluated
    /// once during host composition, never during a script call.
    pub fn capture_allowlist(names: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let values = names
            .into_iter()
            .filter_map(|name| {
                let name = name.as_ref().to_owned();
                std::env::var(&name).ok().map(|value| (name, value))
            })
            .collect();
        Self { values }
    }

    pub fn functions(&self) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
        functions_from_values(self.values.clone())
    }
}

fn functions_from_values(
    values: BTreeMap<String, String>,
) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
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
            callable: WireInterpreterFn::new(move |args| {
                let [WireValue::String { value: name }] = args.as_slice() else {
                    return Err(ProviderError::invalid_argument(
                        "get expects exactly one String name",
                    ));
                };
                Ok(match values.get(name) {
                    Some(value) => WireValue::Variant {
                        type_id: option_type,
                        variant_id: WireCallTypeTable::option_some_variant(),
                        payload: Some(Box::new(WireValue::String {
                            value: value.clone(),
                        })),
                    },
                    None => WireValue::Variant {
                        type_id: option_type,
                        variant_id: WireCallTypeTable::option_none_variant(),
                        payload: None,
                    },
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
            CapturedEnvProvider::default().functions(),
        );
        assert_eq!(report.provider_id, "rsscript.env");
    }

    #[test]
    fn only_captured_values_are_visible() {
        let provider = CapturedEnvProvider::new([("VISIBLE", "yes")]);
        let function = provider.functions().into_values().next().unwrap();
        let mut context = rsscript_provider_api::ProviderCallContext::default();
        let visible = function
            .callable
            .call_with_context(
                &mut context,
                vec![WireValue::String {
                    value: "VISIBLE".into(),
                }],
            )
            .unwrap();
        let hidden = function
            .callable
            .call_with_context(
                &mut context,
                vec![WireValue::String {
                    value: "PATH".into(),
                }],
            )
            .unwrap();
        assert!(matches!(
            visible,
            WireValue::Variant {
                payload: Some(_),
                ..
            }
        ));
        assert!(matches!(hidden, WireValue::Variant { payload: None, .. }));
    }
}
