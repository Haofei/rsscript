#![forbid(unsafe_code)]
use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    ProviderError, ProviderFunction, WireCallTypeTable, WireInterpreterFn, WireValue,
};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

/// One command explicitly admitted by the embedding host.
#[derive(Clone, Debug)]
pub struct ConfiguredCommand {
    executable: PathBuf,
    fixed_args: Vec<String>,
    cwd: Option<PathBuf>,
    environment: BTreeMap<String, String>,
    max_script_args: usize,
}

impl ConfiguredCommand {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            fixed_args: Vec::new(),
            cwd: None,
            environment: BTreeMap::new(),
            max_script_args: 32,
        }
    }

    pub fn fixed_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.fixed_args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn environment(
        mut self,
        values: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.environment = values
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    pub fn max_script_args(mut self, max: usize) -> Self {
        self.max_script_args = max;
        self
    }
}

/// Process capability that maps script-visible command IDs to fixed host
/// executable configurations. Scripts can never supply an executable path,
/// ambient environment, or ambient working directory.
#[derive(Clone, Debug, Default)]
pub struct ConfiguredProcessProvider {
    commands: BTreeMap<String, ConfiguredCommand>,
}

impl ConfiguredProcessProvider {
    pub fn new(commands: impl IntoIterator<Item = (impl Into<String>, ConfiguredCommand)>) -> Self {
        Self {
            commands: commands
                .into_iter()
                .map(|(id, command)| (id.into(), command))
                .collect(),
        }
    }

    pub fn functions(&self) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
        functions_from_commands(self.commands.clone())
    }
}

fn functions_from_commands(
    commands: BTreeMap<String, ConfiguredCommand>,
) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
    let contract = descriptor();
    let function = contract.functions.into_iter().next().unwrap();
    let types = WireCallTypeTable::for_signature(&function.signature)
        .and_then(|types| types.with_record_layouts(contract.record_layouts))
        .expect("generated process descriptor has a valid wire layout");
    let output_type = types
        .type_id(&rsscript_abi_model::WireType::from(
            "host.process.ProcessOutput",
        ))
        .expect("process output record is present in the generated wire layout");
    let string_type = types
        .type_id(&rsscript_abi_model::WireType::String)
        .expect("process signature contains String");
    BTreeMap::from([(
        function.symbol,
        ProviderFunction {
            signature: function.signature,
            callable: WireInterpreterFn::new_contextual(move |context, values| {
                let [
                    WireValue::String { value: command_id },
                    WireValue::List {
                        element_type,
                        values: args,
                    },
                ] = values.as_slice()
                else {
                    return Err(ProviderError::invalid_argument("command ID must be String"));
                };
                if *element_type != string_type {
                    return Err(ProviderError::invalid_argument("args must be List<String>"));
                }
                let args = args
                    .iter()
                    .map(|value| match value {
                        WireValue::String { value } => Ok(value.clone()),
                        _ => Err(ProviderError::invalid_argument(
                            "args must contain String values",
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let configured = commands.get(command_id).ok_or_else(|| {
                    ProviderError::new(
                        rsscript_provider_api::ProviderErrorCode::PermissionDenied,
                        format!("process command ID `{command_id}` is not configured"),
                    )
                })?;
                if args.len() > configured.max_script_args {
                    return Err(ProviderError::invalid_argument(format!(
                        "process command accepts at most {} script arguments",
                        configured.max_script_args
                    )));
                }
                let mut command = Command::new(&configured.executable);
                command
                    .env_clear()
                    .envs(&configured.environment)
                    .args(&configured.fixed_args)
                    .args(args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                if let Some(cwd) = &configured.cwd {
                    command.current_dir(cwd);
                }
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
                Ok(WireValue::Record {
                    type_id: output_type,
                    fields: vec![
                        WireValue::Int {
                            value: status.code().unwrap_or(-1).into(),
                        },
                        WireValue::String {
                            value: String::from_utf8_lossy(&stdout).into_owned(),
                        },
                        WireValue::String {
                            value: String::from_utf8_lossy(&stderr).into_owned(),
                        },
                    ],
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
        let report = rsscript_provider_conformance::assert_wire_provider_conforms(
            descriptor(),
            ConfiguredProcessProvider::default().functions(),
        );
        assert_eq!(report.provider_id, "rsscript.process");
    }

    fn string_list(values: impl IntoIterator<Item = impl Into<String>>) -> WireValue {
        let string_type = WireCallTypeTable::for_signature(&descriptor().functions[0].signature)
            .unwrap()
            .type_id(&rsscript_abi_model::WireType::String)
            .unwrap();
        WireValue::List {
            element_type: string_type,
            values: values
                .into_iter()
                .map(|value| WireValue::String {
                    value: value.into(),
                })
                .collect(),
        }
    }

    #[cfg(unix)]
    fn test_provider(command_id: &str) -> ConfiguredProcessProvider {
        ConfiguredProcessProvider::new([(
            command_id,
            ConfiguredCommand::new("/bin/sh").max_script_args(2),
        )])
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_and_reaps_the_process_tree() {
        let function = test_provider("sleep")
            .functions()
            .into_values()
            .next()
            .unwrap();
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
                    WireValue::String {
                        value: "sleep".into(),
                    },
                    string_list(["-c", "sleep 10"]),
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
        let function = test_provider("print")
            .functions()
            .into_values()
            .next()
            .unwrap();
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
                    WireValue::String {
                        value: "print".into(),
                    },
                    string_list(["-c", "printf '%0200d' 0"]),
                ],
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::ResourceExhausted
        );
    }
}
