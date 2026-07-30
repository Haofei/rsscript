use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ChildStderr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use rss_process_guard::{
    ProcessLimits, WorkerIsolationBackend, WorkerSandbox, spawn_isolated_worker,
};
use rss_worker_protocol::{
    EvalBackend, EvalRequest, MetalMatmulRequest, MetalRun1dRequest, NativeArtifact,
    NativeCallRequest, NativeValue as ProtocolNativeValue, ProgramBundle, ProgramSource, Request,
    RequestOperation, Response, ResponseOutcome, ResponseValue, WorkerError, encode_request,
    read_response,
};

use crate::execution_policy::IsolatedExecutionAuthorization;
use crate::{DeploymentProfile, EvalError, EvalOutput, ExecutionCapability, NativeValue};

const DEFAULT_WALL_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_STDERR_MAX_BYTES: usize = 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Explicit configuration for one verified, isolated execution worker.
#[derive(Clone, Debug)]
pub struct IsolatedWorkerConfig {
    worker_path: PathBuf,
    isolation_backend: WorkerIsolationBackend,
    wall_timeout: Duration,
    process_limits: ProcessLimits,
    stderr_max_bytes: usize,
}

impl IsolatedWorkerConfig {
    pub fn new(
        worker_path: impl Into<PathBuf>,
        isolation_backend: WorkerIsolationBackend,
    ) -> Result<Self, IsolatedExecutionError> {
        let worker_path = worker_path.into();
        if !worker_path.is_absolute() {
            return Err(IsolatedExecutionError::InvalidConfiguration(
                "isolated worker executable must be an absolute path".to_string(),
            ));
        }
        Ok(Self {
            worker_path,
            isolation_backend,
            wall_timeout: DEFAULT_WALL_TIMEOUT,
            process_limits: ProcessLimits::generated_program(),
            stderr_max_bytes: DEFAULT_STDERR_MAX_BYTES,
        })
    }

    pub fn with_wall_timeout(
        mut self,
        wall_timeout: Duration,
    ) -> Result<Self, IsolatedExecutionError> {
        if wall_timeout.is_zero() {
            return Err(IsolatedExecutionError::InvalidConfiguration(
                "isolated worker wall timeout must be nonzero".to_string(),
            ));
        }
        self.wall_timeout = wall_timeout;
        Ok(self)
    }

    pub fn worker_path(&self) -> &Path {
        self.worker_path.as_path()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolatedProgram {
    entry: String,
    sources: Vec<IsolatedProgramSource>,
    interfaces: Vec<IsolatedProgramSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolatedProgramSource {
    path: String,
    source: String,
}

impl IsolatedProgram {
    pub fn new(
        entry: impl Into<String>,
        sources: impl IntoIterator<Item = IsolatedProgramSource>,
    ) -> Self {
        Self {
            entry: entry.into(),
            sources: sources.into_iter().collect(),
            interfaces: Vec::new(),
        }
    }

    pub fn with_interfaces(
        mut self,
        interfaces: impl IntoIterator<Item = IsolatedProgramSource>,
    ) -> Self {
        self.interfaces = interfaces.into_iter().collect();
        self
    }

    pub fn source(path: impl Into<String>, source: impl Into<String>) -> Self {
        let path = path.into();
        Self::new(path.clone(), [IsolatedProgramSource::new(path, source)])
    }
}

impl IsolatedProgramSource {
    pub fn new(path: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
        }
    }
}

#[derive(Debug)]
pub enum IsolatedExecutionError {
    InvalidConfiguration(String),
    Spawn(io::Error),
    Protocol(String),
    Worker(String),
    Timeout(Duration),
    OutputLimit { stream: &'static str, limit: usize },
    Wait(io::Error),
}

impl fmt::Display for IsolatedExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => formatter.write_str(message),
            Self::Spawn(error) => write!(formatter, "failed to spawn isolated worker: {error}"),
            Self::Protocol(message) => {
                write!(formatter, "isolated worker protocol error: {message}")
            }
            Self::Worker(message) => {
                write!(formatter, "isolated worker operation failed: {message}")
            }
            Self::Timeout(timeout) => {
                write!(
                    formatter,
                    "isolated worker exceeded wall timeout of {timeout:?}"
                )
            }
            Self::OutputLimit { stream, limit } => {
                write!(
                    formatter,
                    "isolated worker {stream} exceeded the {limit} byte limit"
                )
            }
            Self::Wait(error) => write!(formatter, "failed to wait for isolated worker: {error}"),
        }
    }
}

impl std::error::Error for IsolatedExecutionError {}

/// Evaluate one program through the reference VM in a verified isolated worker.
///
/// This function sends exactly one framed request and never retries through an
/// in-process backend.
pub fn eval_isolated_reference_vm(
    config: &IsolatedWorkerConfig,
    program: IsolatedProgram,
    args: impl IntoIterator<Item = String>,
) -> Result<EvalOutput, EvalError> {
    eval_isolated(config, program, args, EvalBackend::ReferenceVm)
        .map_err(|error| EvalError::Runtime(error.to_string()))
}

/// Evaluate one program through the native JIT in a verified isolated worker.
pub fn eval_isolated_native_jit(
    config: &IsolatedWorkerConfig,
    program: IsolatedProgram,
    args: impl IntoIterator<Item = String>,
) -> Result<EvalOutput, EvalError> {
    eval_isolated(config, program, args, EvalBackend::NativeJit)
        .map_err(|error| EvalError::Runtime(error.to_string()))
}

fn eval_isolated(
    config: &IsolatedWorkerConfig,
    program: IsolatedProgram,
    args: impl IntoIterator<Item = String>,
    backend: EvalBackend,
) -> Result<EvalOutput, IsolatedExecutionError> {
    let request = Request {
        request_id: next_request_id(),
        operation: RequestOperation::Eval(EvalRequest {
            program: ProgramBundle {
                entry: program.entry,
                sources: program
                    .sources
                    .into_iter()
                    .map(|source| ProgramSource {
                        path: source.path,
                        source: source.source,
                    })
                    .collect(),
                interfaces: program
                    .interfaces
                    .into_iter()
                    .map(|source| ProgramSource {
                        path: source.path,
                        source: source.source,
                    })
                    .collect(),
                native_bindings: Vec::new(),
            },
            backend,
            args: args.into_iter().collect(),
            prebuilt: None,
        }),
    };
    let capability = match backend {
        EvalBackend::ReferenceVm => ExecutionCapability::BoundedReferenceVm,
        EvalBackend::NativeJit => ExecutionCapability::NativeJit,
    };
    let (response, worker_stderr) = execute_isolated_request(config, &request, capability, &[])?;
    response_to_eval_output(response, worker_stderr)
}

/// Call one digest-pinned native binding inside a verified isolated worker.
pub fn call_isolated_native(
    config: &IsolatedWorkerConfig,
    library: &Path,
    sha256: impl Into<String>,
    binding: impl Into<String>,
    args: impl IntoIterator<Item = NativeValue>,
) -> Result<NativeValue, IsolatedExecutionError> {
    let library = library.canonicalize().map_err(|error| {
        IsolatedExecutionError::InvalidConfiguration(format!(
            "failed to resolve isolated native library: {error}"
        ))
    })?;
    let request = Request {
        request_id: next_request_id(),
        operation: RequestOperation::NativeCall(NativeCallRequest {
            library: NativeArtifact {
                relative_path: sandbox_relative_path(&library)?,
                sha256: sha256.into(),
            },
            binding: binding.into(),
            args: args.into_iter().map(native_to_protocol_value).collect(),
        }),
    };
    let (response, stderr) = execute_isolated_request(
        config,
        &request,
        ExecutionCapability::IsolatedNative,
        &[library.as_path()],
    )?;
    require_empty_worker_stderr(&stderr)?;
    match response.outcome {
        ResponseOutcome::Ok(ResponseValue::NativeCall(value)) => Ok(protocol_native_value(value)),
        ResponseOutcome::Ok(_) => Err(IsolatedExecutionError::Protocol(
            "worker returned a non-native response to a native request".to_string(),
        )),
        ResponseOutcome::Error(error) => Err(worker_error(error)),
    }
}

/// Execute bounded matrix multiplication inside a verified isolated worker.
pub fn metal_matmul_isolated(
    config: &IsolatedWorkerConfig,
    request: MetalMatmulRequest,
) -> Result<Vec<f32>, IsolatedExecutionError> {
    let request = Request {
        request_id: next_request_id(),
        operation: RequestOperation::MetalMatmul(request),
    };
    let (response, stderr) =
        execute_isolated_request(config, &request, ExecutionCapability::DynamicGpuShader, &[])?;
    require_empty_worker_stderr(&stderr)?;
    match response.outcome {
        ResponseOutcome::Ok(ResponseValue::MetalMatmul(values)) => Ok(values),
        ResponseOutcome::Ok(_) => Err(IsolatedExecutionError::Protocol(
            "worker returned the wrong response to a Metal matmul request".to_string(),
        )),
        ResponseOutcome::Error(error) => Err(worker_error(error)),
    }
}

/// Execute one policy-scoped dynamic shader inside a verified isolated worker.
pub fn metal_run_1d_isolated(
    config: &IsolatedWorkerConfig,
    request: MetalRun1dRequest,
) -> Result<Vec<f32>, IsolatedExecutionError> {
    let request = Request {
        request_id: next_request_id(),
        operation: RequestOperation::MetalRun1d(request),
    };
    let (response, stderr) =
        execute_isolated_request(config, &request, ExecutionCapability::DynamicGpuShader, &[])?;
    require_empty_worker_stderr(&stderr)?;
    match response.outcome {
        ResponseOutcome::Ok(ResponseValue::MetalRun1d(values)) => Ok(values),
        ResponseOutcome::Ok(_) => Err(IsolatedExecutionError::Protocol(
            "worker returned the wrong response to a Metal run request".to_string(),
        )),
        ResponseOutcome::Error(error) => Err(worker_error(error)),
    }
}

fn execute_isolated_request(
    config: &IsolatedWorkerConfig,
    request: &Request,
    capability: ExecutionCapability,
    read_only_inputs: &[&Path],
) -> Result<(Response, Vec<u8>), IsolatedExecutionError> {
    let request_bytes = encode_request(&request)
        .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))?;

    let mut sandbox = WorkerSandbox::new(&config.worker_path, config.process_limits)
        .map_err(IsolatedExecutionError::Spawn)?;
    mount_linux_system_inputs(&mut sandbox).map_err(IsolatedExecutionError::Spawn)?;
    for input in read_only_inputs {
        sandbox
            .read_only_input(*input)
            .map_err(IsolatedExecutionError::Spawn)?;
    }
    let (mut child, proof) = spawn_isolated_worker(&config.isolation_backend, &sandbox)
        .map_err(IsolatedExecutionError::Spawn)?;
    let authorization = IsolatedExecutionAuthorization::from_worker_isolation_proof(proof);
    authorization.authorize(
        DeploymentProfile::UntrustedIsolated,
        capability,
        child.child_mut().id(),
    )?;

    let stdin = child.child_mut().stdin.take().ok_or_else(|| {
        IsolatedExecutionError::Protocol("worker stdin was not piped".to_string())
    })?;
    let stdout = child.child_mut().stdout.take().ok_or_else(|| {
        IsolatedExecutionError::Protocol("worker stdout was not piped".to_string())
    })?;
    let stderr = child.child_mut().stderr.take().ok_or_else(|| {
        IsolatedExecutionError::Protocol("worker stderr was not piped".to_string())
    })?;
    let (request_tx, request_rx) = mpsc::sync_channel(1);
    let _request_thread = thread::spawn(move || {
        let _ = request_tx.send(write_request_bytes(stdin, &request_bytes));
    });
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    let _response_thread = thread::spawn(move || {
        let _ = response_tx.send(read_one_response(stdout));
    });
    let stderr_limit = config.stderr_max_bytes;
    let stderr_thread = thread::spawn(move || read_bounded_stderr(stderr, stderr_limit));

    let deadline = Instant::now()
        .checked_add(config.wall_timeout)
        .ok_or_else(|| {
            IsolatedExecutionError::InvalidConfiguration(
                "isolated worker wall timeout exceeds the platform clock range".to_string(),
            )
        })?;
    loop {
        match request_rx.try_recv() {
            Ok(Ok(())) => break,
            Ok(Err(error)) => return Err(error),
            Err(TryRecvError::Disconnected) => {
                return Err(IsolatedExecutionError::Protocol(
                    "worker request writer stopped unexpectedly".to_string(),
                ));
            }
            Err(TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(IsolatedExecutionError::Timeout(config.wall_timeout));
        }
        thread::sleep(POLL_INTERVAL);
    }

    let response = loop {
        match response_rx.try_recv() {
            Ok(Ok(response)) => break response,
            Ok(Err(error)) => return Err(error),
            Err(TryRecvError::Disconnected) => {
                return Err(IsolatedExecutionError::Protocol(
                    "worker response reader stopped unexpectedly".to_string(),
                ));
            }
            Err(TryRecvError::Empty) => {}
        }
        let _ = child.try_wait().map_err(IsolatedExecutionError::Wait)?;
        if Instant::now() >= deadline {
            return Err(IsolatedExecutionError::Timeout(config.wall_timeout));
        }
        thread::sleep(POLL_INTERVAL);
    };

    while child
        .try_wait()
        .map_err(IsolatedExecutionError::Wait)?
        .is_none()
    {
        if Instant::now() >= deadline {
            return Err(IsolatedExecutionError::Timeout(config.wall_timeout));
        }
        thread::sleep(POLL_INTERVAL);
    }
    let status = child.wait().map_err(IsolatedExecutionError::Wait)?;
    let worker_stderr = stderr_thread.join().map_err(|_| {
        IsolatedExecutionError::Protocol("worker stderr reader panicked".to_string())
    })??;
    if !status.success() {
        return Err(IsolatedExecutionError::Worker(format!(
            "worker exited with {status}: {}",
            String::from_utf8_lossy(&worker_stderr)
        )));
    }
    response
        .validate_for_request(request)
        .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))?;
    Ok((response, worker_stderr))
}

fn write_request_bytes(
    mut stdin: std::process::ChildStdin,
    request: &[u8],
) -> Result<(), IsolatedExecutionError> {
    stdin
        .write_all(request)
        .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))?;
    stdin
        .flush()
        .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))
}

fn read_one_response(
    mut stdout: std::process::ChildStdout,
) -> Result<Response, IsolatedExecutionError> {
    let response = read_response(&mut stdout)
        .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))?;
    let mut trailing = [0_u8; 1];
    if stdout
        .read(&mut trailing)
        .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))?
        != 0
    {
        return Err(IsolatedExecutionError::Protocol(
            "worker wrote trailing data after its response frame".to_string(),
        ));
    }
    Ok(response)
}

fn read_bounded_stderr(
    mut stderr: ChildStderr,
    limit: usize,
) -> Result<Vec<u8>, IsolatedExecutionError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stderr
            .read(&mut buffer)
            .map_err(|error| IsolatedExecutionError::Protocol(error.to_string()))?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(IsolatedExecutionError::OutputLimit {
                stream: "stderr",
                limit,
            });
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn response_to_eval_output(
    response: Response,
    worker_stderr: Vec<u8>,
) -> Result<EvalOutput, IsolatedExecutionError> {
    require_empty_worker_stderr(&worker_stderr)?;
    match response.outcome {
        ResponseOutcome::Ok(ResponseValue::Eval(result)) => Ok(EvalOutput {
            value: result.value,
            display_value: result.display_value,
            native_value: result.native_value.map(protocol_native_value),
            stdout: result.stdout,
            stderr: result.stderr,
        }),
        ResponseOutcome::Ok(_) => Err(IsolatedExecutionError::Protocol(
            "worker returned a non-eval response to an eval request".to_string(),
        )),
        ResponseOutcome::Error(error) => Err(worker_error(error)),
    }
}

fn worker_error(error: WorkerError) -> IsolatedExecutionError {
    IsolatedExecutionError::Worker(format!("{:?}: {}", error.code, error.message))
}

fn require_empty_worker_stderr(stderr: &[u8]) -> Result<(), IsolatedExecutionError> {
    if stderr.is_empty() {
        Ok(())
    } else {
        Err(IsolatedExecutionError::Protocol(format!(
            "worker wrote non-protocol stderr on success: {}",
            String::from_utf8_lossy(stderr)
        )))
    }
}

fn sandbox_relative_path(path: &Path) -> Result<String, IsolatedExecutionError> {
    let relative = path.strip_prefix(Path::new("/")).map_err(|_| {
        IsolatedExecutionError::InvalidConfiguration(
            "isolated worker inputs must resolve below the filesystem root".to_string(),
        )
    })?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.is_empty() {
        return Err(IsolatedExecutionError::InvalidConfiguration(
            "isolated worker input cannot be the filesystem root".to_string(),
        ));
    }
    Ok(relative)
}

fn native_to_protocol_value(value: NativeValue) -> ProtocolNativeValue {
    match value {
        NativeValue::Unit => ProtocolNativeValue::Unit,
        NativeValue::Int(value) => ProtocolNativeValue::Int(value),
        NativeValue::Float(value) => ProtocolNativeValue::Float(value),
        NativeValue::Bool(value) => ProtocolNativeValue::Bool(value),
        NativeValue::String(value) => ProtocolNativeValue::String(value),
        NativeValue::Char(value) => ProtocolNativeValue::Char(value),
        NativeValue::Bytes(value) => ProtocolNativeValue::Bytes(value),
        NativeValue::List(values) => {
            ProtocolNativeValue::List(values.into_iter().map(native_to_protocol_value).collect())
        }
        NativeValue::Map(values) => ProtocolNativeValue::Map(
            values
                .into_iter()
                .map(|(key, value)| {
                    (
                        native_to_protocol_value(key),
                        native_to_protocol_value(value),
                    )
                })
                .collect(),
        ),
        NativeValue::Json(value) => ProtocolNativeValue::Json(value),
        NativeValue::Struct { name, fields } => ProtocolNativeValue::Struct {
            name,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, native_to_protocol_value(value)))
                .collect(),
        },
        NativeValue::Variant { name, fields } => ProtocolNativeValue::Variant {
            name,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, native_to_protocol_value(value)))
                .collect(),
        },
        NativeValue::Native { type_name, id } => ProtocolNativeValue::Native { type_name, id },
    }
}

fn protocol_native_value(value: ProtocolNativeValue) -> NativeValue {
    match value {
        ProtocolNativeValue::Unit => NativeValue::Unit,
        ProtocolNativeValue::Int(value) => NativeValue::Int(value),
        ProtocolNativeValue::Float(value) => NativeValue::Float(value),
        ProtocolNativeValue::Bool(value) => NativeValue::Bool(value),
        ProtocolNativeValue::String(value) => NativeValue::String(value),
        ProtocolNativeValue::Char(value) => NativeValue::Char(value),
        ProtocolNativeValue::Bytes(value) => NativeValue::Bytes(value),
        ProtocolNativeValue::List(values) => {
            NativeValue::List(values.into_iter().map(protocol_native_value).collect())
        }
        ProtocolNativeValue::Map(values) => NativeValue::Map(
            values
                .into_iter()
                .map(|(key, value)| (protocol_native_value(key), protocol_native_value(value)))
                .collect(),
        ),
        ProtocolNativeValue::Json(value) => NativeValue::Json(value),
        ProtocolNativeValue::Struct { name, fields } => NativeValue::Struct {
            name,
            fields: convert_fields(fields),
        },
        ProtocolNativeValue::Variant { name, fields } => NativeValue::Variant {
            name,
            fields: convert_fields(fields),
        },
        ProtocolNativeValue::Native { type_name, id } => NativeValue::Native { type_name, id },
    }
}

fn convert_fields(fields: BTreeMap<String, ProtocolNativeValue>) -> BTreeMap<String, NativeValue> {
    fields
        .into_iter()
        .map(|(name, value)| (name, protocol_native_value(value)))
        .collect()
}

fn next_request_id() -> u64 {
    static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn mount_linux_system_inputs(_sandbox: &mut WorkerSandbox) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    for path in ["/usr", "/lib", "/lib64"] {
        if Path::new(path).exists() {
            _sandbox.read_only_system_input(path)?;
        }
    }
    Ok(())
}

impl From<crate::ExecutionPolicyError> for IsolatedExecutionError {
    fn from(error: crate::ExecutionPolicyError) -> Self {
        Self::InvalidConfiguration(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rss_worker_protocol::{ResponseOutcome, encode_response};

    #[test]
    fn config_rejects_relative_worker_path() {
        let error =
            IsolatedWorkerConfig::new("rss-execution-worker", WorkerIsolationBackend::bubblewrap())
                .expect_err("relative worker path must fail closed");
        assert!(error.to_string().contains("absolute path"));
    }

    #[test]
    fn config_rejects_zero_wall_timeout() {
        let config =
            IsolatedWorkerConfig::new(test_absolute_path(), WorkerIsolationBackend::bubblewrap())
                .expect("absolute worker path");
        let error = config
            .with_wall_timeout(Duration::ZERO)
            .expect_err("zero timeout must fail closed");
        assert!(error.to_string().contains("must be nonzero"));
    }

    #[test]
    fn response_rejects_mismatched_type() {
        let response = Response {
            request_id: 1,
            outcome: ResponseOutcome::Ok(ResponseValue::NativeCall(ProtocolNativeValue::Unit)),
        };
        let error = response_to_eval_output(response, Vec::new()).expect_err("wrong response type");
        assert!(error.to_string().contains("non-eval response"));
    }

    #[test]
    fn malformed_response_is_rejected_by_framing() {
        let error = rss_worker_protocol::decode_response(b"not a frame")
            .expect_err("malformed response must fail");
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn mismatched_response_id_is_rejected() {
        let request = Request {
            request_id: 7,
            operation: RequestOperation::Eval(EvalRequest {
                program: ProgramBundle {
                    entry: "main.rss".to_string(),
                    sources: vec![ProgramSource {
                        path: "main.rss".to_string(),
                        source: "fn main() -> Unit { () }".to_string(),
                    }],
                    interfaces: Vec::new(),
                    native_bindings: Vec::new(),
                },
                backend: EvalBackend::ReferenceVm,
                args: Vec::new(),
                prebuilt: None,
            }),
        };
        let response = Response {
            request_id: 8,
            outcome: ResponseOutcome::Ok(ResponseValue::Eval(rss_worker_protocol::EvalResult {
                value: "()".to_string(),
                display_value: "()".to_string(),
                native_value: Some(ProtocolNativeValue::Unit),
                stdout: String::new(),
                stderr: String::new(),
            })),
        };
        assert!(response.validate_for_request(&request).is_err());
        assert!(encode_response(&response).is_ok());
    }

    #[cfg(unix)]
    fn test_absolute_path() -> PathBuf {
        PathBuf::from("/rsscript-test/worker")
    }

    #[cfg(windows)]
    fn test_absolute_path() -> PathBuf {
        PathBuf::from(r"C:\rsscript-test\worker.exe")
    }
}
