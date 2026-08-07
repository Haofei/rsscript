#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};
use std::collections::BTreeMap;
use std::sync::Arc;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions(
    sink: impl Fn(&str) -> Result<(), ProviderError> + Send + Sync + 'static,
) -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let sink = Arc::new(sink);
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: NativeInterpreterFn::new(move |mut args| {
                let NativeValue::String(message) = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("message must be String"));
                };
                sink(&message)?;
                Ok(NativeValue::Unit)
            }),
        },
    )])
}
pub fn stderr_functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    functions(|message| {
        eprintln!("{message}");
        Ok(())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conforms_to_provider_contract() {
        let report = rsscript_provider_conformance::assert_provider_conforms(
            descriptor(),
            functions(|_| Ok(())),
        );
        assert_eq!(report.provider_id, "rsscript.log");
    }
}
