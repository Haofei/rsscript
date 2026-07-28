use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crate::channel::stream_from_external_receiver_with_drop;
use crate::{ChannelError, NativeAsyncPending, RssStream, spawn_tokio_native};
use crate::{JsonValue, json_to_string};
use crate::{RssCancellationToken, cancellation_token_is_cancelled};

pub const RUNTIME_PROCESS_OUTPUT_CEILING_BYTES: usize = 64 * 1024 * 1024;
pub const RUNTIME_PROCESS_CONCURRENCY_CEILING: usize = 32;
pub const RUNTIME_PROCESS_TIMEOUT_CEILING_MS: u64 = 24 * 60 * 60 * 1_000;
const PROCESS_STREAM_CHANNEL_CAPACITY: usize = 64;

fn process_timeout(timeout_ms: i64) -> Result<Option<Duration>, String> {
    if timeout_ms <= 0 {
        return Ok(None);
    }
    let timeout_ms = u64::try_from(timeout_ms)
        .map_err(|_| "process timeout must be a positive integer".to_string())?;
    if timeout_ms > RUNTIME_PROCESS_TIMEOUT_CEILING_MS {
        return Err(format!(
            "process timeout exceeds the {}ms runtime ceiling",
            RUNTIME_PROCESS_TIMEOUT_CEILING_MS
        ));
    }
    Ok(Some(Duration::from_millis(timeout_ms)))
}

struct ProcessConcurrency {
    active: Mutex<usize>,
    ready: Condvar,
}

struct ProcessPermit {
    concurrency: &'static ProcessConcurrency,
}

impl Drop for ProcessPermit {
    fn drop(&mut self) {
        let mut active = self
            .concurrency
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        self.concurrency.ready.notify_one();
    }
}

fn process_concurrency_limit() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, RUNTIME_PROCESS_CONCURRENCY_CEILING)
}

fn acquire_process_permit(
    cancellation: Option<&RssCancellationToken>,
) -> Result<ProcessPermit, String> {
    static CONCURRENCY: OnceLock<ProcessConcurrency> = OnceLock::new();
    let concurrency = CONCURRENCY.get_or_init(|| ProcessConcurrency {
        active: Mutex::new(0),
        ready: Condvar::new(),
    });
    let mut active = concurrency
        .active
        .lock()
        .map_err(|_| "process concurrency lock poisoned".to_string())?;
    while *active >= process_concurrency_limit() {
        if cancellation.is_some_and(cancellation_token_is_cancelled) {
            return Err("process cancelled while waiting for a concurrency slot".to_string());
        }
        let (next, _) = concurrency
            .ready
            .wait_timeout(active, Duration::from_millis(25))
            .map_err(|_| "process concurrency lock poisoned".to_string())?;
        active = next;
    }
    *active += 1;
    Ok(ProcessPermit { concurrency })
}

pub fn os_close(fd: i64) {
    let _ = fd;
}

pub fn args_count() -> i64 {
    std::env::args().skip(1).count() as i64
}

pub fn args_all() -> Vec<String> {
    std::env::args().skip(1).collect()
}

pub fn args_get(index: i64) -> Option<String> {
    if index < 0 {
        return None;
    }
    std::env::args().skip(1).nth(index as usize)
}

pub fn args_get_or_default(index: i64, default: &str) -> String {
    if index < 0 {
        return default.to_string();
    }
    std::env::args()
        .skip(1)
        .nth(index as usize)
        .unwrap_or_else(|| default.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: i64,
    pub stdout: String,
    pub stderr: String,
    pub merged: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEnv {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEvent {
    pub kind: String,
    pub data: String,
    pub status: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<String>,
    pub env: Vec<ProcessEnv>,
    pub timeout_ms: i64,
    pub merge_stderr: bool,
    pub output_cap_bytes: i64,
}

pub fn process_run(command: &str, args: &[String]) -> Result<ProcessOutput, String> {
    process_run_request(&simple_process_request(command, args, 0))
}

pub fn process_run_async(
    command: &str,
    args: &[String],
) -> NativeAsyncPending<Result<ProcessOutput, String>> {
    let command = command.to_string();
    let args = args.to_vec();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || process_run(&command, &args))
            .await
            .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_run_stdout(command: &str, args: &[String]) -> Result<String, String> {
    process_run(command, args).and_then(|output| process_stdout_result(command, output))
}

pub fn process_run_stdout_async(
    command: &str,
    args: &[String],
) -> NativeAsyncPending<Result<String, String>> {
    let command = command.to_string();
    let args = args.to_vec();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || process_run_stdout(&command, &args))
            .await
            .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_run_stdout_timeout(
    command: &str,
    args: &[String],
    timeout_ms: i64,
) -> Result<String, String> {
    if timeout_ms <= 0 {
        return process_run_stdout(command, args);
    }

    process_run_timeout(command, args, timeout_ms).and_then(|output| {
        if output.status == 0 {
            Ok(output.stdout)
        } else {
            Err(format!(
                "`{command}` exited with {}: {}",
                output.status,
                process_output_details_from_strings(&output.stdout, &output.stderr)
            ))
        }
    })
}

pub fn process_run_stdout_timeout_async(
    command: &str,
    args: &[String],
    timeout_ms: i64,
) -> NativeAsyncPending<Result<String, String>> {
    let command = command.to_string();
    let args = args.to_vec();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || process_run_stdout_timeout(&command, &args, timeout_ms))
            .await
            .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_run_timeout(
    command: &str,
    args: &[String],
    timeout_ms: i64,
) -> Result<ProcessOutput, String> {
    let request = simple_process_request(command, args, timeout_ms);
    let output = process_run_request(&request)?;
    Ok(output)
}

fn simple_process_request(command: &str, args: &[String], timeout_ms: i64) -> ProcessRequest {
    ProcessRequest {
        command: command.to_string(),
        args: args.to_vec(),
        cwd: None,
        stdin: None,
        env: Vec::new(),
        timeout_ms,
        merge_stderr: false,
        output_cap_bytes: RUNTIME_PROCESS_OUTPUT_CEILING_BYTES as i64,
    }
}

pub fn process_run_timeout_async(
    command: &str,
    args: &[String],
    timeout_ms: i64,
) -> NativeAsyncPending<Result<ProcessOutput, String>> {
    let command = command.to_string();
    let args = args.to_vec();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || process_run_timeout(&command, &args, timeout_ms))
            .await
            .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_run_request(request: &ProcessRequest) -> Result<ProcessOutput, String> {
    process_run_request_with_cancellation(request, None)
}

pub fn process_run_request_async(
    request: &ProcessRequest,
) -> NativeAsyncPending<Result<ProcessOutput, String>> {
    let request = request.clone();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || process_run_request(&request))
            .await
            .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_run_request_cancellable_async(
    request: &ProcessRequest,
    token: &RssCancellationToken,
) -> NativeAsyncPending<Result<ProcessOutput, String>> {
    let request = request.clone();
    let token = token.clone();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || {
            process_run_request_with_cancellation(&request, Some(&token))
        })
        .await
        .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_stream(request: &ProcessRequest) -> Result<RssStream<ProcessEvent>, String> {
    if request.command.trim().is_empty() {
        return Err("process command must not be empty".to_string());
    }
    let process_permit = acquire_process_permit(None)?;

    let timeout = process_timeout(request.timeout_ms)?;
    let cap = normalized_process_output_cap(request.output_cap_bytes);
    let mut command = std::process::Command::new(&request.command);
    command
        .args(&request.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if request.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for env in &request.env {
        command.env(&env.name, &env.value);
    }
    apply_default_ramdisk_env(&mut command);
    let mut child = rss_process_guard::spawn_guarded_child(
        &mut command,
        rss_process_guard::ProcessLimits::generated_program(),
    )
    .map_err(|error| format!("failed to run `{}`: {error}", request.command))?;
    let stdin = request.stdin.clone();
    let command_name = request.command.clone();
    let stream_dropped = Arc::new(AtomicBool::new(false));
    let monitor_dropped = Arc::clone(&stream_dropped);
    let limit = Arc::new(Mutex::new(ProcessStreamLimit::new(cap)));
    let (sender, receiver) =
        mpsc::sync_channel::<Result<ProcessEvent, ChannelError>>(PROCESS_STREAM_CHANNEL_CAPACITY);
    let stdout = child.child_mut().stdout.take();
    let stderr = child.child_mut().stderr.take();
    if let Some(stdout) = stdout {
        spawn_process_event_reader(stdout, "stdout", Arc::clone(&limit), sender.clone());
    }
    if let Some(stderr) = stderr {
        spawn_process_event_reader(stderr, "stderr", Arc::clone(&limit), sender.clone());
    }
    if let Some(stdin) = stdin
        && let Some(mut child_stdin) = child.child_mut().stdin.take()
    {
        let stdin_sender = sender.clone();
        let stdin_command = command_name.clone();
        std::thread::spawn(move || {
            if let Err(error) = child_stdin.write_all(stdin.as_bytes()) {
                let _ = stdin_sender.send(Ok(ProcessEvent {
                    kind: "error".to_string(),
                    data: format!("failed to write stdin for `{stdin_command}`: {error}"),
                    status: -1,
                }));
            }
        });
    }
    std::thread::spawn(move || {
        let _process_permit = process_permit;
        let started = Instant::now();
        loop {
            if monitor_dropped.load(Ordering::Acquire) {
                let _ = child.terminate();
                let _ = child.wait();
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let status = child.wait().unwrap_or(status);
                    let _ = sender.send(Ok(ProcessEvent {
                        kind: "exit".to_string(),
                        data: String::new(),
                        status: status.code().map(i64::from).unwrap_or(-1),
                    }));
                    break;
                }
                Ok(None) => {
                    if let Some(timeout) = timeout
                        && started.elapsed() >= timeout
                    {
                        let _ = child.terminate();
                        let _ = sender.send(Ok(ProcessEvent {
                            kind: "timeout".to_string(),
                            data: format!(
                                "`{command_name}` timed out after {}ms",
                                timeout.as_millis()
                            ),
                            status: -1,
                        }));
                        let _ = child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => {
                    let _ = child.terminate();
                    let _ = child.wait();
                    let _ = sender.send(Ok(ProcessEvent {
                        kind: "error".to_string(),
                        data: format!("failed to poll `{command_name}`: {error}"),
                        status: -1,
                    }));
                    break;
                }
            }
        }
    });

    Ok(stream_from_external_receiver_with_drop(
        receiver,
        Some(Box::new(move || {
            stream_dropped.store(true, Ordering::Release);
        })),
    ))
}

fn process_run_request_with_cancellation(
    request: &ProcessRequest,
    cancellation: Option<&RssCancellationToken>,
) -> Result<ProcessOutput, String> {
    if request.command.trim().is_empty() {
        return Err("process command must not be empty".to_string());
    }
    let _process_permit = acquire_process_permit(cancellation)?;

    let timeout = process_timeout(request.timeout_ms)?;
    let deadline = timeout.and_then(|duration| Instant::now().checked_add(duration));
    if timeout.is_some() && deadline.is_none() {
        return Err("process timeout cannot be represented by the platform clock".to_string());
    }
    let cap = normalized_process_output_cap(request.output_cap_bytes);
    let mut command = std::process::Command::new(&request.command);
    command
        .args(&request.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if request.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for env in &request.env {
        command.env(&env.name, &env.value);
    }
    apply_default_ramdisk_env(&mut command);
    let mut child = rss_process_guard::spawn_guarded_child(
        &mut command,
        rss_process_guard::ProcessLimits::generated_program(),
    )
    .map_err(|error| format!("failed to run `{}`: {error}", request.command))?;

    let stdin_thread = request.stdin.as_ref().and_then(|stdin| {
        child.child_mut().stdin.take().map(|mut child_stdin| {
            let stdin = stdin.clone();
            std::thread::spawn(move || child_stdin.write_all(stdin.as_bytes()))
        })
    });

    let (sender, receiver) = mpsc::channel();
    let capture_budget = Arc::new(AtomicUsize::new(cap));
    let capture_truncated = Arc::new(AtomicBool::new(false));
    let stdout_thread = child.child_mut().stdout.take().map(|stdout| {
        spawn_process_reader(
            stdout,
            false,
            sender.clone(),
            Arc::clone(&capture_budget),
            Arc::clone(&capture_truncated),
        )
    });
    let stderr_thread = child.child_mut().stderr.take().map(|stderr| {
        spawn_process_reader(
            stderr,
            true,
            sender.clone(),
            Arc::clone(&capture_budget),
            Arc::clone(&capture_truncated),
        )
    });
    drop(sender);

    let mut captured = ProcessCapture::new(Some(cap), request.merge_stderr);
    let mut timed_out = false;
    let mut cancelled = false;
    loop {
        drain_process_chunks(&receiver, &mut captured);
        if child
            .try_wait()
            .map_err(|error| format!("failed to poll `{}`: {error}", request.command))?
            .is_some()
        {
            break;
        }
        if cancellation.is_some_and(cancellation_token_is_cancelled) {
            cancelled = true;
            let _ = child.terminate();
            break;
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            timed_out = true;
            let _ = child.terminate();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for `{}`: {error}", request.command))?;
    let stdin_result = stdin_thread.map(|thread| thread.join());
    join_process_reader(stdout_thread, "stdout", &request.command)?;
    join_process_reader(stderr_thread, "stderr", &request.command)?;
    while let Ok(chunk) = receiver.try_recv() {
        captured.push(chunk);
    }
    captured.truncated |= capture_truncated.load(Ordering::Acquire);

    let output = ProcessOutput {
        status: status.code().map(i64::from).unwrap_or(-1),
        stdout: String::from_utf8_lossy(&captured.stdout).to_string(),
        stderr: String::from_utf8_lossy(&captured.stderr).to_string(),
        merged: String::from_utf8_lossy(&captured.merged).to_string(),
        truncated: captured.truncated,
    };
    if timed_out {
        return Err(format!(
            "`{}` timed out after {}ms: {}",
            request.command,
            request.timeout_ms,
            process_output_details_from_strings(&output.stdout, &output.stderr)
        ));
    }
    if cancelled {
        return Err(format!(
            "`{}` cancelled: {}",
            request.command,
            process_output_details_from_strings(&output.stdout, &output.stderr)
        ));
    }
    if let Some(stdin_result) = stdin_result {
        match stdin_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(format!(
                    "failed to write stdin for `{}`: {error}",
                    request.command
                ));
            }
            Err(_) => return Err(format!("stdin writer panicked for `{}`", request.command)),
        }
    }
    Ok(output)
}

struct ProcessChunk {
    stderr: bool,
    bytes: Vec<u8>,
    truncated: bool,
}

struct ProcessStreamLimit {
    cap: usize,
    used: usize,
    truncated_sent: bool,
}

impl ProcessStreamLimit {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            used: 0,
            truncated_sent: false,
        }
    }

    fn take(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let cap = self.cap;
        if self.used >= cap {
            if !self.truncated_sent {
                self.truncated_sent = true;
                return (Vec::new(), true);
            }
            return (Vec::new(), false);
        }
        let remaining = cap - self.used;
        if bytes.len() > remaining {
            self.used = cap;
            let truncated = !self.truncated_sent;
            self.truncated_sent = true;
            (bytes[..remaining].to_vec(), truncated)
        } else {
            self.used += bytes.len();
            (bytes.to_vec(), false)
        }
    }
}

struct ProcessCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    merged: Vec<u8>,
    cap: Option<usize>,
    used: usize,
    merge_stderr: bool,
    truncated: bool,
}

impl ProcessCapture {
    fn new(cap: Option<usize>, merge_stderr: bool) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            merged: Vec::new(),
            cap,
            used: 0,
            merge_stderr,
            truncated: false,
        }
    }

    fn push(&mut self, chunk: ProcessChunk) {
        self.truncated |= chunk.truncated;
        let bytes = self.capped_bytes(&chunk.bytes);
        if bytes.is_empty() {
            return;
        }
        if chunk.stderr {
            self.stderr.extend_from_slice(bytes);
        } else {
            self.stdout.extend_from_slice(bytes);
        }
        if self.merge_stderr || !chunk.stderr {
            self.merged.extend_from_slice(bytes);
        }
    }

    fn capped_bytes<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        let Some(cap) = self.cap else {
            return bytes;
        };
        if self.used >= cap {
            self.truncated = true;
            return &bytes[..0];
        }
        let remaining = cap - self.used;
        if bytes.len() > remaining {
            self.truncated = true;
            self.used = cap;
            &bytes[..remaining]
        } else {
            self.used += bytes.len();
            bytes
        }
    }
}

fn spawn_process_reader<R>(
    mut reader: R,
    stderr: bool,
    sender: mpsc::Sender<ProcessChunk>,
    remaining: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<()>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let bytes = reader.read(&mut buffer)?;
            if bytes == 0 {
                return Ok(());
            }
            let retained = reserve_process_output_bytes(&remaining, bytes);
            if retained < bytes {
                truncated.store(true, Ordering::Release);
            }
            if retained == 0 {
                continue;
            }
            if sender
                .send(ProcessChunk {
                    stderr,
                    bytes: buffer[..retained].to_vec(),
                    truncated: retained < bytes,
                })
                .is_err()
            {
                return Ok(());
            }
        }
    })
}

fn normalized_process_output_cap(requested: i64) -> usize {
    usize::try_from(requested)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(RUNTIME_PROCESS_OUTPUT_CEILING_BYTES)
        .min(RUNTIME_PROCESS_OUTPUT_CEILING_BYTES)
}

fn reserve_process_output_bytes(remaining: &AtomicUsize, requested: usize) -> usize {
    let mut available = remaining.load(Ordering::Acquire);
    loop {
        let retained = requested.min(available);
        match remaining.compare_exchange_weak(
            available,
            available - retained,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return retained,
            Err(actual) => available = actual,
        }
    }
}

fn spawn_process_event_reader<R>(
    mut reader: R,
    kind: &'static str,
    limit: Arc<Mutex<ProcessStreamLimit>>,
    sender: mpsc::SyncSender<Result<ProcessEvent, ChannelError>>,
) -> std::thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let bytes = match reader.read(&mut buffer) {
                Ok(0) => return,
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = sender.send(Ok(ProcessEvent {
                        kind: "error".to_string(),
                        data: format!("failed to read {kind}: {error}"),
                        status: -1,
                    }));
                    return;
                }
            };
            let (chunk, truncated) = {
                let mut limit = limit.lock().expect("process stream limit lock poisoned");
                limit.take(&buffer[..bytes])
            };
            if !chunk.is_empty() {
                let _ = sender.send(Ok(ProcessEvent {
                    kind: kind.to_string(),
                    data: String::from_utf8_lossy(&chunk).to_string(),
                    status: -1,
                }));
            }
            if truncated {
                let _ = sender.send(Ok(ProcessEvent {
                    kind: "truncated".to_string(),
                    data: String::new(),
                    status: -1,
                }));
            }
        }
    })
}

fn drain_process_chunks(receiver: &mpsc::Receiver<ProcessChunk>, captured: &mut ProcessCapture) {
    while let Ok(chunk) = receiver.try_recv() {
        captured.push(chunk);
    }
}

fn join_process_reader(
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stream: &str,
    command: &str,
) -> Result<(), String> {
    let Some(thread) = thread else {
        return Ok(());
    };
    match thread.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("failed to read {stream} for `{command}`: {error}")),
        Err(_) => Err(format!("{stream} reader panicked for `{command}`")),
    }
}

fn process_stdout_result(command: &str, output: ProcessOutput) -> Result<String, String> {
    if output.status == 0 {
        return Ok(output.stdout);
    }
    Err(format!(
        "`{command}` exited with {}: {}",
        output.status,
        process_output_details_from_strings(&output.stdout, &output.stderr)
    ))
}

fn process_output_details_from_strings(stdout: &str, stderr: &str) -> String {
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    }
}

pub fn process_run_many_stdout(
    command: &str,
    args: &[String],
    appended_args: &[String],
    jobs: i64,
) -> Result<Vec<String>, String> {
    process_run_many_stdout_with_runner(command, args, appended_args, jobs, process_run_stdout)
}

pub fn process_run_many_stdout_async(
    command: &str,
    args: &[String],
    appended_args: &[String],
    jobs: i64,
) -> NativeAsyncPending<Result<Vec<String>, String>> {
    let command = command.to_string();
    let args = args.to_vec();
    let appended_args = appended_args.to_vec();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || {
            process_run_many_stdout(&command, &args, &appended_args, jobs)
        })
        .await
        .map_err(|error| format!("process task failed: {error}"))?
    })
}

pub fn process_run_many_stdout_timeout(
    command: &str,
    args: &[String],
    appended_args: &[String],
    jobs: i64,
    timeout_ms: i64,
) -> Result<Vec<String>, String> {
    process_run_many_stdout_with_runner(command, args, appended_args, jobs, |command, args| {
        process_run_stdout_timeout(command, args, timeout_ms)
    })
}

pub fn process_run_many_stdout_timeout_async(
    command: &str,
    args: &[String],
    appended_args: &[String],
    jobs: i64,
    timeout_ms: i64,
) -> NativeAsyncPending<Result<Vec<String>, String>> {
    let command = command.to_string();
    let args = args.to_vec();
    let appended_args = appended_args.to_vec();
    spawn_tokio_native(async move {
        tokio::task::spawn_blocking(move || {
            process_run_many_stdout_timeout(&command, &args, &appended_args, jobs, timeout_ms)
        })
        .await
        .map_err(|error| format!("process task failed: {error}"))?
    })
}

fn process_run_many_stdout_with_runner(
    command: &str,
    args: &[String],
    appended_args: &[String],
    jobs: i64,
    runner: impl Fn(&str, &[String]) -> Result<String, String> + Send + Sync,
) -> Result<Vec<String>, String> {
    if appended_args.is_empty() {
        return Ok(Vec::new());
    }

    let worker_count = process_worker_count(jobs).min(appended_args.len());
    let next_index = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let results = std::sync::Arc::new(
        (0..appended_args.len())
            .map(|_| std::sync::Mutex::new(None))
            .collect::<Vec<std::sync::Mutex<Option<String>>>>(),
    );
    let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let args = std::sync::Arc::new(args.to_vec());
    let appended_args = std::sync::Arc::new(appended_args.to_vec());
    let runner = &runner;

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let next_index = std::sync::Arc::clone(&next_index);
            let results = std::sync::Arc::clone(&results);
            let errors = std::sync::Arc::clone(&errors);
            let args = std::sync::Arc::clone(&args);
            let appended_args = std::sync::Arc::clone(&appended_args);
            scope.spawn(move || {
                loop {
                    let index = next_index.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(appended_arg) = appended_args.get(index) else {
                        break;
                    };
                    let mut command_args = (*args).clone();
                    command_args.push(appended_arg.clone());
                    match runner(command, &command_args) {
                        Ok(stdout) => {
                            if let Ok(mut result) = results[index].lock() {
                                *result = Some(stdout);
                            }
                        }
                        Err(error) => {
                            if let Ok(mut errors) = errors.lock() {
                                errors.push(format!("command {index}: {error}"));
                            }
                        }
                    }
                }
            });
        }
    });

    let errors = errors
        .lock()
        .map_err(|_| "process error lock poisoned".to_string())?;
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    drop(errors);

    results
        .iter()
        .map(|result| {
            result
                .lock()
                .map_err(|_| "process result lock poisoned".to_string())?
                .clone()
                .ok_or_else(|| "missing process result".to_string())
        })
        .collect()
}

fn process_worker_count(jobs: i64) -> usize {
    if jobs > 0 {
        return usize::try_from(jobs)
            .unwrap_or(RUNTIME_PROCESS_CONCURRENCY_CEILING)
            .min(process_concurrency_limit());
    }
    process_concurrency_limit()
}

fn apply_default_ramdisk_env(command: &mut std::process::Command) {
    if ramdisk_auto_env_enabled()
        && std::env::var_os("RSSCRIPT_RAMDISK_PATH").is_none()
        && let Some(path) = default_ramdisk_root_dir()
    {
        command.env("RSSCRIPT_RAMDISK_PATH", path);
    }
}

fn ramdisk_auto_env_enabled() -> bool {
    matches!(
        std::env::var("RSSCRIPT_ENABLE_RAMDISK").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[cfg(target_os = "macos")]
fn default_ramdisk_root_dir() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from("/Volumes/RSScriptRAMDisk");
    if path.is_dir() {
        return Some(path);
    }

    let gib = std::env::var("RSSCRIPT_RAMDISK_GIB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8);
    let sectors = gib
        .saturating_mul(1024)
        .saturating_mul(1024)
        .saturating_mul(1024)
        / 512;
    let attach = std::process::Command::new("hdiutil")
        .arg("attach")
        .arg("-nomount")
        .arg(format!("ram://{sectors}"))
        .output()
        .ok()?;
    if !attach.status.success() {
        return None;
    }
    let device = String::from_utf8_lossy(&attach.stdout).trim().to_string();
    if device.is_empty() {
        return None;
    }

    let erase = std::process::Command::new("diskutil")
        .arg("erasevolume")
        .arg("HFS+")
        .arg("RSScriptRAMDisk")
        .arg(device)
        .output()
        .ok()?;
    if !erase.status.success() || !path.is_dir() {
        return None;
    }

    Some(path)
}

#[cfg(not(target_os = "macos"))]
fn default_ramdisk_root_dir() -> Option<std::path::PathBuf> {
    None
}

pub fn log_write(message: &str) {
    if bench_silences_log() {
        std::hint::black_box(message);
        return;
    }
    println!("{message}");
}

pub fn log_write_json(value: &JsonValue) {
    if bench_silences_log() {
        std::hint::black_box(value);
        return;
    }
    println!("{}", json_to_string(value));
}

fn bench_silences_log() -> bool {
    std::env::var_os("RSSCRIPT_BENCH_SILENCE_LOG").is_some_and(|value| value == "1")
}

pub fn log_error(message: &str) {
    eprintln!("{message}");
}

pub fn log_error_json(value: &JsonValue) {
    eprintln!("{}", json_to_string(value));
}

pub fn log_trace(event: &str, message: &str) {
    tracing::info!(event, message, "rsscript_trace");
    println!("trace {event}: {message}");
}

#[cfg(test)]
mod tests {
    use crate::Executor;

    #[test]
    fn zero_and_oversized_process_caps_use_the_runtime_ceiling() {
        assert_eq!(
            super::normalized_process_output_cap(0),
            super::RUNTIME_PROCESS_OUTPUT_CEILING_BYTES
        );
        assert_eq!(
            super::normalized_process_output_cap(-1),
            super::RUNTIME_PROCESS_OUTPUT_CEILING_BYTES
        );
        assert_eq!(
            super::normalized_process_output_cap(i64::MAX),
            super::RUNTIME_PROCESS_OUTPUT_CEILING_BYTES
        );
        assert_eq!(super::normalized_process_output_cap(17), 17);
    }

    #[test]
    fn process_worker_count_is_hard_capped() {
        assert_eq!(
            super::process_worker_count(i64::MAX),
            super::process_concurrency_limit()
        );
        assert!(super::process_worker_count(0) <= super::RUNTIME_PROCESS_CONCURRENCY_CEILING);
        assert_eq!(super::process_worker_count(1), 1);
    }

    #[test]
    fn process_run_stdout_async_completes_on_native_runtime() {
        let args = vec!["--version".to_string()];
        let stdout = Executor::new()
            .run_pending(super::process_run_stdout_async("cargo", &args))
            .expect("cargo --version should run");
        assert!(stdout.contains("cargo"));
    }

    #[test]
    #[cfg(unix)]
    fn process_children_receive_generated_program_limits() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "ulimit -n".to_string()],
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout_ms: 10_000,
            merge_stderr: false,
            output_cap_bytes: 1024,
        };

        let output = super::process_run_request(&request).expect("limited child should run");
        assert_eq!(output.status, 0);
        assert_eq!(output.stdout.trim(), "256");
    }

    #[test]
    #[cfg(unix)]
    fn normal_root_exit_terminates_background_descendants() {
        let marker = std::env::temp_dir().join(format!(
            "rsscript-runtime-descendant-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("(sleep 1; touch '{}') & exit 0", marker.display()),
            ],
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout_ms: 10_000,
            merge_stderr: false,
            output_cap_bytes: 1024,
        };
        let output = super::process_run_request(&request).expect("root process should finish");
        assert_eq!(output.status, 0);
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert!(!marker.exists(), "background descendant escaped runtime");
    }

    #[test]
    #[cfg(unix)]
    fn process_run_request_supports_agent_controls() {
        let cwd =
            std::env::temp_dir().join(format!("rsscript-process-request-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "pwd; read line; echo \"$RSS_TEST_VALUE:$line\"; echo err >&2".to_string(),
            ],
            cwd: Some(cwd.clone()),
            stdin: Some("input\n".to_string()),
            env: vec![super::ProcessEnv {
                name: "RSS_TEST_VALUE".to_string(),
                value: "env".to_string(),
            }],
            timeout_ms: 10_000,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };

        let output = super::process_run_request(&request).expect("process request should run");

        assert_eq!(output.status, 0);
        assert!(output.stdout.contains(&cwd.display().to_string()));
        assert!(output.stdout.contains("env:input"));
        assert!(output.stderr.contains("err"));
        assert!(output.merged.contains("env:input"));
        assert!(output.merged.contains("err"));
        assert!(!output.truncated);
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    #[cfg(unix)]
    fn process_run_request_caps_output_while_reading() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'abcdef'; printf 'ghijkl' >&2".to_string(),
            ],
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout_ms: 10_000,
            merge_stderr: true,
            output_cap_bytes: 5,
        };

        let output = super::process_run_request(&request).expect("process request should run");

        assert_eq!(output.stdout.len() + output.stderr.len(), 5);
        assert!(output.truncated);
    }

    #[test]
    #[cfg(unix)]
    fn process_run_timeout_drains_large_stdout_while_waiting() {
        let args = vec![
            "-c".to_string(),
            "i=0; while [ $i -lt 20000 ]; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; i=$((i + 1)); done"
                .to_string(),
        ];

        let output =
            super::process_run_timeout("sh", &args, 10_000).expect("large stdout should drain");

        assert_eq!(output.status, 0);
        assert!(output.stdout.len() > 500_000);
        assert!(output.stderr.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn process_run_stdout_timeout_drains_large_stdout_while_waiting() {
        let args = vec![
            "-c".to_string(),
            "i=0; while [ $i -lt 20000 ]; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; i=$((i + 1)); done"
                .to_string(),
        ];

        let stdout = super::process_run_stdout_timeout("sh", &args, 10_000)
            .expect("large stdout should drain");

        assert!(stdout.len() > 500_000);
    }

    #[test]
    #[cfg(unix)]
    fn process_run_request_cancellable_async_kills_child() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 5; echo done".to_string()],
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout_ms: 30_000,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };
        let mut source = crate::cancellation_source_new();
        let token = crate::cancellation_source_token(&source);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            crate::cancellation_source_cancel(&mut source);
        });

        let error = Executor::new()
            .run_pending(super::process_run_request_cancellable_async(
                &request, &token,
            ))
            .expect_err("process should be cancelled");

        assert!(error.contains("cancelled"), "unexpected error: {error}");
    }

    #[test]
    #[cfg(unix)]
    fn process_timeout_wins_over_stdin_broken_pipe() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 5".to_string()],
            cwd: None,
            stdin: Some("x".repeat(8 * 1024 * 1024)),
            env: Vec::new(),
            timeout_ms: 30,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };

        let error = super::process_run_request(&request).expect_err("process should time out");

        assert!(error.contains("timed out"), "{error}");
        assert!(!error.contains("failed to write stdin"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn process_cancellation_wins_over_stdin_broken_pipe() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 5".to_string()],
            cwd: None,
            stdin: Some("x".repeat(8 * 1024 * 1024)),
            env: Vec::new(),
            timeout_ms: 30_000,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };
        let mut source = crate::cancellation_source_new();
        let token = crate::cancellation_source_token(&source);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            crate::cancellation_source_cancel(&mut source);
        });

        let error = super::process_run_request_with_cancellation(&request, Some(&token))
            .expect_err("process should be cancelled");

        assert!(error.contains("cancelled"), "{error}");
        assert!(!error.contains("failed to write stdin"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn process_stream_pushes_stdout_stderr_and_exit_events() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf out; printf err >&2; exit 7".to_string(),
            ],
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout_ms: 10_000,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };
        let stream = super::process_stream(&request).expect("process stream should start");
        let mut kinds = Vec::new();
        let mut data = String::new();
        let mut status = None;
        let mut executor = Executor::new();
        for _ in 0..200 {
            let mut pending = crate::stream_next(&stream);
            match executor.poll_once(&mut pending) {
                crate::AsyncPoll::Ready(Ok(Some(event))) => {
                    if event.kind == "exit" {
                        status = Some(event.status);
                    }
                    data.push_str(&event.data);
                    kinds.push(event.kind);
                    if status.is_some()
                        && kinds.iter().any(|kind| kind == "stdout")
                        && kinds.iter().any(|kind| kind == "stderr")
                    {
                        break;
                    }
                }
                crate::AsyncPoll::Ready(Ok(None)) => break,
                crate::AsyncPoll::Ready(Err(error)) => panic!("stream next failed: {error:?}"),
                crate::AsyncPoll::Pending => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
            }
        }

        assert!(kinds.iter().any(|kind| kind == "stdout"));
        assert!(kinds.iter().any(|kind| kind == "stderr"));
        assert_eq!(status, Some(7));
        assert!(data.contains("out"));
        assert!(data.contains("err"));
    }

    #[test]
    #[cfg(unix)]
    fn process_stream_timeout_is_not_blocked_by_stdin_writes() {
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 5".to_string()],
            cwd: None,
            stdin: Some("x".repeat(8 * 1024 * 1024)),
            env: Vec::new(),
            timeout_ms: 30,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };
        let started = std::time::Instant::now();
        let stream = super::process_stream(&request).expect("process stream should start");
        let events = crate::stream_collect_list(&stream).expect("stream should complete");

        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "timeout monitor was blocked by stdin"
        );
        assert!(events.iter().any(|event| event.kind == "timeout"));
    }

    #[test]
    #[cfg(unix)]
    fn dropping_process_stream_terminates_the_child() {
        let pid_file = std::env::temp_dir().join(format!(
            "rsscript-process-stream-drop-{}.pid",
            std::process::id()
        ));
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("echo $$ > '{}'; sleep 5", pid_file.display()),
            ],
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout_ms: 30_000,
            merge_stderr: true,
            output_cap_bytes: 1024,
        };
        let stream = super::process_stream(&request).expect("process stream should start");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !pid_file.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let pid = std::fs::read_to_string(&pid_file)
            .expect("child should publish its pid")
            .trim()
            .to_string();

        drop(stream);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut alive = true;
        while alive && std::time::Instant::now() < deadline {
            alive = std::process::Command::new("kill")
                .args(["-0", &pid])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if alive {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        let _ = std::fs::remove_file(pid_file);
        assert!(!alive, "child process {pid} survived stream drop");
    }
}
