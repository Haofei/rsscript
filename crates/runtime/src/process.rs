mod capture;
mod environment;
mod policy;
mod supervisor;

use std::path::PathBuf;

use crate::RssCancellationToken;
use crate::{JsonValue, json_to_string};
use crate::{NativeAsyncPending, RssStream, spawn_tokio_native};

pub use environment::ProcessEnv;
use policy::process_worker_count;
pub use policy::{
    DEFAULT_RUNTIME_PROCESS_TIMEOUT_MS, RUNTIME_PROCESS_CONCURRENCY_CEILING,
    RUNTIME_PROCESS_TIMEOUT_CEILING_MS,
};
use supervisor::process_run_request_with_cancellation;

pub const RUNTIME_PROCESS_OUTPUT_CEILING_BYTES: usize = 64 * 1024 * 1024;

pub fn os_close(fd: i64) {
    let _ = fd;
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
    supervisor::process_stream(request)
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

pub(super) fn process_output_details_from_strings(stdout: &str, stderr: &str) -> String {
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
            super::capture::normalized_process_output_cap(0),
            super::RUNTIME_PROCESS_OUTPUT_CEILING_BYTES
        );
        assert_eq!(
            super::capture::normalized_process_output_cap(-1),
            super::RUNTIME_PROCESS_OUTPUT_CEILING_BYTES
        );
        assert_eq!(
            super::capture::normalized_process_output_cap(i64::MAX),
            super::RUNTIME_PROCESS_OUTPUT_CEILING_BYTES
        );
        assert_eq!(super::capture::normalized_process_output_cap(17), 17);
    }

    #[test]
    fn process_worker_count_is_hard_capped() {
        assert_eq!(
            super::process_worker_count(i64::MAX),
            super::policy::process_concurrency_limit()
        );
        assert!(super::process_worker_count(0) <= super::RUNTIME_PROCESS_CONCURRENCY_CEILING);
        assert_eq!(super::process_worker_count(1), 1);
    }

    #[test]
    fn zero_timeout_uses_a_finite_default() {
        assert_eq!(
            super::policy::process_timeout(0).expect("default timeout"),
            std::time::Duration::from_millis(super::DEFAULT_RUNTIME_PROCESS_TIMEOUT_MS)
        );
        assert_eq!(
            super::policy::process_timeout(-1).expect("negative timeout uses default"),
            std::time::Duration::from_millis(super::DEFAULT_RUNTIME_PROCESS_TIMEOUT_MS)
        );
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
            "rsscript-aot-runtime-descendant-{}",
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
    fn process_environment_is_allowlisted_and_explicit_values_are_preserved() {
        assert!(
            std::env::var_os("HOME").is_some(),
            "test requires a parent HOME value"
        );
        let request = super::ProcessRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'home=%s explicit=%s' \"${HOME-unset}\" \"$RSS_EXPLICIT\"".to_string(),
            ],
            cwd: None,
            stdin: None,
            env: vec![super::ProcessEnv {
                name: "RSS_EXPLICIT".to_string(),
                value: "visible".to_string(),
            }],
            timeout_ms: 10_000,
            merge_stderr: false,
            output_cap_bytes: 1024,
        };

        let output = super::process_run_request(&request).expect("child should run");

        assert_eq!(output.stdout, "home=unset explicit=visible");
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
