#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction};
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

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
                let capture_limit = context
                    .remaining_byte_budget
                    .into_iter()
                    .chain(context.remaining_output_budget)
                    .chain([MAX_CAPTURE_BYTES])
                    .min()
                    .unwrap_or(MAX_CAPTURE_BYTES);
                let captured = Arc::new(AtomicUsize::new(0));
                let capture_exceeded = Arc::new(AtomicBool::new(false));
                let stdout_reader = spawn_pipe_reader(
                    stdout,
                    Arc::clone(&captured),
                    Arc::clone(&capture_exceeded),
                    capture_limit,
                );
                let stderr_reader = spawn_pipe_reader(
                    stderr,
                    Arc::clone(&captured),
                    Arc::clone(&capture_exceeded),
                    capture_limit,
                );
                loop {
                    if capture_exceeded.load(Ordering::Acquire) {
                        let _ = child.terminate();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(ProviderError::resource_exhausted(format!(
                            "process output exceeds {capture_limit} bytes"
                        )));
                    }
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

fn spawn_pipe_reader(
    pipe: impl Read + Send + 'static,
    captured: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    limit: usize,
) -> std::thread::JoinHandle<Result<Vec<u8>, ProviderError>> {
    std::thread::spawn(move || read_pipe_bounded(pipe, &captured, &exceeded, limit))
}

fn read_pipe_bounded(
    mut pipe: impl Read,
    captured: &AtomicUsize,
    exceeded: &AtomicBool,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = pipe
            .read(&mut chunk)
            .map_err(|error| ProviderError::from_io("read process pipe", error))?;
        if read == 0 {
            break;
        }
        let previous = captured.fetch_add(read, Ordering::AcqRel);
        if read > limit.saturating_sub(previous) {
            exceeded.store(true, Ordering::Release);
            return Err(ProviderError::resource_exhausted(format!(
                "process output exceeds {limit} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(bytes)
}

fn join_pipe(
    reader: std::thread::JoinHandle<Result<Vec<u8>, ProviderError>>,
    name: &str,
) -> Result<Vec<u8>, ProviderError> {
    reader
        .join()
        .map_err(|_| ProviderError::internal(format!("{name} reader panicked")))?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conforms_to_provider_contract() {
        let report =
            rsscript_provider_conformance::assert_provider_conforms(descriptor(), functions());
        assert_eq!(report.provider_id, "rsscript.process");
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

    #[cfg(unix)]
    #[test]
    fn combined_process_output_obeys_runtime_budget() {
        let function = functions().into_values().next().unwrap();
        let mut context = rsscript_provider_api::ProviderCallContext {
            remaining_byte_budget: Some(32),
            remaining_output_budget: Some(64),
            blocking_allowed: true,
            ..rsscript_provider_api::ProviderCallContext::default()
        };
        let error = function
            .callable
            .call_with_context(
                &mut context,
                vec![
                    NativeValue::String("sh".into()),
                    NativeValue::List(vec![
                        NativeValue::String("-c".into()),
                        NativeValue::String("printf '%0200d' 0".into()),
                    ]),
                ],
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::ResourceExhausted
        );
    }
}
