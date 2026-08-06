#![forbid(unsafe_code)]
use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, RUNTIME_ABI_VERSION,
};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderError, ProviderFunction, ProviderFunctionDescriptor,
};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
fn symbol() -> ExternalSymbol {
    ExternalSymbol::new("host.process.run").unwrap()
}
fn signature() -> FunctionSignature {
    FunctionSignature {
        parameters: vec![
            ParameterSignature {
                name: "program".into(),
                effect: DataEffect::Read,
                ty: "String".into(),
                retained: false,
            },
            ParameterSignature {
                name: "args".into(),
                effect: DataEffect::Read,
                ty: "List<String>".into(),
                retained: false,
            },
        ],
        result: "ProcessOutput".into(),
        asynchronous: false,
    }
}
pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.process".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol(),
            signature: signature(),
            entry: "run".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::MayBlock,
            cancellation: CancellationBehavior::AbortSafe,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: rsscript_provider_api::ResourceCleanupContract::ProviderManaged,
            error_mapping: rsscript_provider_api::ProviderErrorMapping::StructuredV1,
        }],
    }
}
pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    BTreeMap::from([(
        symbol(),
        ProviderFunction {
            signature: signature(),
            callable: NativeInterpreterFn::new(|mut values| {
                let NativeValue::String(program) = values.remove(0) else {
                    return Err(ProviderError::invalid_argument("program must be String"));
                };
                let NativeValue::List(args) = values.remove(0) else {
                    return Err(ProviderError::invalid_argument("args must be List<String>"));
                };
                let args = args
                    .into_iter()
                    .map(|value| match value {
                        NativeValue::String(value) => Ok(value),
                        _ => Err(ProviderError::invalid_argument(
                            "args must contain String values",
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut command = Command::new(program);
                command
                    .args(args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let (child, guard) = rss_process_guard::spawn_guarded(
                    &mut command,
                    rss_process_guard::ProcessLimits::generated_program(),
                )
                .map_err(|error| ProviderError::from_io("start process", error))?;
                let output = child.wait_with_output().map_err(|e| {
                    let _ = guard.terminate();
                    ProviderError::from_io("wait for process", e)
                })?;
                Ok(NativeValue::Struct {
                    name: "ProcessOutput".into(),
                    fields: BTreeMap::from([
                        (
                            "status".into(),
                            NativeValue::Int(output.status.code().unwrap_or(-1).into()),
                        ),
                        (
                            "stdout".into(),
                            NativeValue::String(
                                String::from_utf8_lossy(&output.stdout).into_owned(),
                            ),
                        ),
                        (
                            "stderr".into(),
                            NativeValue::String(
                                String::from_utf8_lossy(&output.stderr).into_owned(),
                            ),
                        ),
                    ]),
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
        let mut r = rsscript_provider_api::ProviderRegistry::new(RUNTIME_ABI_VERSION);
        r.register_provider(&descriptor(), functions()).unwrap();
    }
}
