#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

pub fn functions() -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
    let function = descriptor().functions.into_iter().next().unwrap();
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: NativeInterpreterFn::new_contextual(|context, mut values| {
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
                let mut child = rss_process_guard::spawn_guarded_child(
                    &mut command,
                    rss_process_guard::ProcessLimits::generated_program(),
                )
                .map_err(|error| ProviderError::from_io("start process", error))?;
                let stdout = child.child_mut().stdout.take().ok_or_else(|| {
                    ProviderError::internal("guarded child stdout pipe is unavailable")
                })?;
                let stderr = child.child_mut().stderr.take().ok_or_else(|| {
                    ProviderError::internal("guarded child stderr pipe is unavailable")
                })?;
                let stdout_reader = std::thread::spawn(move || read_pipe(stdout));
                let stderr_reader = std::thread::spawn(move || read_pipe(stderr));
                loop {
                    if let Err(cancelled) = context.check_cancelled() {
                        let _ = child.terminate();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(cancelled);
                    }
                    if child
                        .try_wait()
                        .map_err(|error| ProviderError::from_io("poll process", error))?
                        .is_some()
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let status = child
                    .wait()
                    .map_err(|error| ProviderError::from_io("wait for process", error))?;
                let stdout = join_pipe(stdout_reader, "stdout")?;
                let stderr = join_pipe(stderr_reader, "stderr")?;
                Ok(NativeValue::Struct {
                    name: "ProcessOutput".into(),
                    fields: BTreeMap::from([
                        (
                            "status".into(),
                            NativeValue::Int(status.code().unwrap_or(-1).into()),
                        ),
                        (
                            "stdout".into(),
                            NativeValue::String(String::from_utf8_lossy(&stdout).into_owned()),
                        ),
                        (
                            "stderr".into(),
                            NativeValue::String(String::from_utf8_lossy(&stderr).into_owned()),
                        ),
                    ]),
                })
            }),
        },
    )])
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_pipe(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    name: &str,
) -> Result<Vec<u8>, ProviderError> {
    reader
        .join()
        .map_err(|_| ProviderError::internal(format!("{name} reader panicked")))?
        .map_err(|error| ProviderError::from_io(&format!("read process {name}"), error))
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

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_and_reaps_the_process_tree() {
        let function = functions().into_values().next().unwrap();
        let cancellation = rsscript_provider_api::CancellationToken::new();
        let trigger = cancellation.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });
        let mut context = rsscript_provider_api::ProviderCallContext {
            cancellation: Some(&cancellation),
            blocking_allowed: true,
            ..rsscript_provider_api::ProviderCallContext::default()
        };
        let started = std::time::Instant::now();
        let error = function
            .callable
            .call_with_context(
                &mut context,
                vec![
                    NativeValue::String("sh".into()),
                    NativeValue::List(vec![
                        NativeValue::String("-c".into()),
                        NativeValue::String("sleep 10".into()),
                    ]),
                ],
            )
            .unwrap_err();
        canceller.join().unwrap();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::Cancelled
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
