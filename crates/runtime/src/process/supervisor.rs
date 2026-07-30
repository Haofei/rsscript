use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::channel::stream_from_external_receiver_with_drop;
use crate::{ChannelError, RssCancellationToken, RssStream, cancellation_token_is_cancelled};

use super::capture::{
    PROCESS_CAPTURE_CHANNEL_CAPACITY, PROCESS_STREAM_CHANNEL_CAPACITY, ProcessCapture,
    ProcessStreamLimit, drain_process_chunks, join_process_reader, normalized_process_output_cap,
    spawn_process_event_reader, spawn_process_reader,
};
use super::environment::{apply_default_ramdisk_env, configure_process_environment};
use super::policy::{acquire_process_permit, process_timeout};
use super::{ProcessEvent, ProcessOutput, ProcessRequest, process_output_details_from_strings};

pub(super) fn process_stream(request: &ProcessRequest) -> Result<RssStream<ProcessEvent>, String> {
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
    configure_process_environment(&mut command, &request.env);
    if request.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
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
                    if started.elapsed() >= timeout {
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

pub(super) fn process_run_request_with_cancellation(
    request: &ProcessRequest,
    cancellation: Option<&RssCancellationToken>,
) -> Result<ProcessOutput, String> {
    if request.command.trim().is_empty() {
        return Err("process command must not be empty".to_string());
    }
    let _process_permit = acquire_process_permit(cancellation)?;

    let timeout = process_timeout(request.timeout_ms)?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| "process timeout cannot be represented by the platform clock".to_string())?;
    let cap = normalized_process_output_cap(request.output_cap_bytes);
    let mut command = std::process::Command::new(&request.command);
    command
        .args(&request.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_environment(&mut command, &request.env);
    if request.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
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

    let (sender, receiver) = mpsc::sync_channel(PROCESS_CAPTURE_CHANNEL_CAPACITY);
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
        if Instant::now() >= deadline {
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
            timeout.as_millis(),
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
