#![forbid(unsafe_code)]
use rsscript_abi_model::{ExternalSymbol, FunctionSignature, RUNTIME_ABI_VERSION};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderFunction, ProviderFunctionDescriptor,
};
use std::collections::BTreeMap;
use std::sync::Arc;
fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.cli.args").unwrap()
}
fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![],
        return_type: "List<String>".into(),
        asynchronous: false,
    }
}
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.cli".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature: signature(),
            entry: "args".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup_contract: "none".into(),
            error_mapping: "none".into(),
        }],
    }
}
pub fn functions(
    args: Vec<String>,
) -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let args = Arc::new(args);
    BTreeMap::from([(
        symbol(),
        ProviderFunction {
            signature: signature(),
            callable: NativeInterpreterFn::new(move |values| {
                if !values.is_empty() {
                    return Err("args takes no arguments".into());
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
        let mut r = rsscript_provider_api::ProviderRegistry::new(RUNTIME_ABI_VERSION);
        r.register_provider(&descriptor(), functions(vec![]))
            .unwrap();
    }
}
