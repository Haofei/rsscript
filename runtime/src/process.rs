use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::stream_from_external_receiver;
use crate::{ChannelError, NativeAsyncPending, RssStream, spawn_tokio_native};
use crate::{JsonValue, json_to_string};
use crate::{RssCancellationToken, cancellation_token_is_cancelled};

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
    let mut child = std::process::Command::new(command);
    child.args(args);
    apply_default_ramdisk_env(&mut child);

    let output = child
        .output()
        .map_err(|error| format!("failed to run `{command}`: {error}"))?;
    Ok(process_output(command, output))
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
    let mut child = std::process::Command::new(command);
    child.args(args);
    apply_default_ramdisk_env(&mut child);

    let output = child
        .output()
        .map_err(|error| format!("failed to run `{command}`: {error}"))?;
    process_output_result(command, output)
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
    if timeout_ms <= 0 {
        return process_run(command, args);
    }

    let request = ProcessRequest {
        command: command.to_string(),
        args: args.to_vec(),
        cwd: None,
        stdin: None,
        env: Vec::new(),
        timeout_ms,
        merge_stderr: false,
        output_cap_bytes: 0,
    };
    let output = process_run_request(&request)?;
    if output.status == -1 && output.merged.contains("timed out") {
        return Err(output.merged);
    }
    Ok(output)
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

    let timeout = u64::try_from(request.timeout_ms)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_millis);
    let cap = usize::try_from(request.output_cap_bytes)
        .ok()
        .filter(|value| *value > 0);
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
    configure_process_child(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run `{}`: {error}", request.command))?;
    let stdin = request.stdin.clone();
    let command_name = request.command.clone();
    let limit = Arc::new(Mutex::new(ProcessStreamLimit::new(cap)));
    let (sender, receiver) = mpsc::channel::<Result<ProcessEvent, ChannelError>>();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(stdout) = stdout {
        spawn_process_event_reader(stdout, "stdout", Arc::clone(&limit), sender.clone());
    }
    if let Some(stderr) = stderr {
        spawn_process_event_reader(stderr, "stderr", Arc::clone(&limit), sender.clone());
    }
    std::thread::spawn(move || {
        if let Some(stdin) = stdin
            && let Some(mut child_stdin) = child.stdin.take()
            && let Err(error) = child_stdin.write_all(stdin.as_bytes())
        {
            let _ = sender.send(Ok(ProcessEvent {
                kind: "error".to_string(),
                data: format!("failed to write stdin for `{command_name}`: {error}"),
                status: -1,
            }));
        }
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
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
                        terminate_process_child(&mut child);
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

    Ok(stream_from_external_receiver(receiver))
}

fn process_run_request_with_cancellation(
    request: &ProcessRequest,
    cancellation: Option<&RssCancellationToken>,
) -> Result<ProcessOutput, String> {
    if request.command.trim().is_empty() {
        return Err("process command must not be empty".to_string());
    }

    let timeout = u64::try_from(request.timeout_ms)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_millis);
    let deadline = timeout.map(|duration| Instant::now() + duration);
    let cap = usize::try_from(request.output_cap_bytes)
        .ok()
        .filter(|value| *value > 0);
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
    configure_process_child(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run `{}`: {error}", request.command))?;

    let stdin_thread = request.stdin.as_ref().and_then(|stdin| {
        child.stdin.take().map(|mut child_stdin| {
            let stdin = stdin.clone();
            std::thread::spawn(move || child_stdin.write_all(stdin.as_bytes()))
        })
    });

    let (sender, receiver) = mpsc::channel();
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| spawn_process_reader(stdout, false, sender.clone()));
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| spawn_process_reader(stderr, true, sender.clone()));
    drop(sender);

    let mut captured = ProcessCapture::new(cap, request.merge_stderr);
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
            terminate_process_child(&mut child);
            break;
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            timed_out = true;
            terminate_process_child(&mut child);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for `{}`: {error}", request.command))?;
    if let Some(thread) = stdin_thread {
        match thread.join() {
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
    join_process_reader(stdout_thread, "stdout", &request.command)?;
    join_process_reader(stderr_thread, "stderr", &request.command)?;
    while let Ok(chunk) = receiver.try_recv() {
        captured.push(chunk);
    }

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
    Ok(output)
}

struct ProcessChunk {
    stderr: bool,
    bytes: Vec<u8>,
}

struct ProcessStreamLimit {
    cap: Option<usize>,
    used: usize,
    truncated_sent: bool,
}

impl ProcessStreamLimit {
    fn new(cap: Option<usize>) -> Self {
        Self {
            cap,
            used: 0,
            truncated_sent: false,
        }
    }

    fn take(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let Some(cap) = self.cap else {
            return (bytes.to_vec(), false);
        };
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
            if sender
                .send(ProcessChunk {
                    stderr,
                    bytes: buffer[..bytes].to_vec(),
                })
                .is_err()
            {
                return Ok(());
            }
        }
    })
}

fn spawn_process_event_reader<R>(
    mut reader: R,
    kind: &'static str,
    limit: Arc<Mutex<ProcessStreamLimit>>,
    sender: mpsc::Sender<Result<ProcessEvent, ChannelError>>,
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

fn configure_process_child(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

fn terminate_process_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = std::process::Command::new("kill")
            .arg("-TERM")
            .arg("--")
            .arg(&group)
            .status();
        std::thread::sleep(Duration::from_millis(25));
        if child.try_wait().ok().flatten().is_none() {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg("--")
                .arg(&group)
                .status();
        }
    }
    let _ = child.kill();
}

fn process_output(command: &str, output: std::process::Output) -> ProcessOutput {
    let status = output.status.code().map(i64::from).unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let merged = process_output_details(&output);
    let _ = command;
    ProcessOutput {
        status,
        stdout,
        stderr,
        merged,
        truncated: false,
    }
}

fn process_output_result(command: &str, output: std::process::Output) -> Result<String, String> {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if output.status.success() {
        return Ok(stdout);
    }

    let code = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    Err(format!(
        "`{command}` exited with {code}: {}",
        process_output_details(&output)
    ))
}

fn process_output_details(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
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
        return jobs as usize;
    }
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .max(1)
}

#[cfg(test)]
mod tests {
    use crate::Executor;

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

        assert!(error.contains("cancelled"));
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
