#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
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
        let mut r =
            rsscript_provider_api::ProviderRegistry::new(rsscript_abi_model::RUNTIME_ABI_VERSION);
        r.register_provider(&descriptor(), functions()).unwrap();
    }
}
