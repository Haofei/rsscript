use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rss_process_guard::{GuardedChild, ProcessLimits};
use rsscript_runner_protocol::{
    MAX_RESPONSE_BYTES, RunnerLimitsV1, RunnerProfileV1, RunnerRequestV1, RunnerResponseV1,
    RunnerTerminationV1, read_request, read_response, write_request, write_response,
};
use rsscript_sdk::{
    artifact::{ARTIFACT_BUNDLE_MAGIC, ArtifactBundle, ArtifactVerifier},
    compile::Compiler,
    operation::MonotonicDeadline,
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};

use super::{is_package_directory, read_cli_source};

const RUNNER_STDERR_LIMIT: usize = 1024 * 1024;

pub(crate) fn run_isolated(path: &str, program_args: &[&str], json: bool) -> ExitCode {
    let bundle = match build_bundle(path) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let request = match RunnerRequestV1::new(
        program_args
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    ) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("invalid runner request: {error}");
            return ExitCode::from(2);
        }
    };
    let response = match invoke_runner(&request, &bundle) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("isolated runner failed: {error}");
            return ExitCode::from(2);
        }
    };
    finish_response(response, json)
}

pub(crate) fn run_trusted_in_process(path: &str, program_args: &[&str], json: bool) -> ExitCode {
    let bundle = match build_bundle(path) {
        Ok(bundle) => bundle,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let verified = match ArtifactVerifier.verify_bundle(bundle) {
        Ok(verified) => verified,
        Err(error) => {
            eprintln!("verification failed: {error}");
            return ExitCode::from(1);
        }
    };
    let linked = match Runtime::new(ProviderRegistry::default()).link(&verified) {
        Ok(linked) => linked,
        Err(error) => {
            eprintln!("link failed: {error}");
            return ExitCode::from(1);
        }
    };
    let report = linked.execute(
        ExecutionRequest::new(program_args.iter().copied())
            .limits(runner_limits(&RunnerLimitsV1::default()))
            .trace(TracePolicy::MetadataOnly),
    );
    finish_report(
        serde_json::to_value(report).expect("execution report serializes"),
        json,
    )
}

fn build_bundle(path: &str) -> Result<ArtifactBundle, String> {
    if Path::new(path).is_file() {
        let bytes = fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
        if bytes.starts_with(ARTIFACT_BUNDLE_MAGIC) {
            return ArtifactBundle::from_bytes(&bytes).map_err(|error| error.to_string());
        }
    }
    let compiler = Compiler;
    if is_package_directory(path) {
        compiler
            .compile_package(Path::new(path))
            .map(|built| built.into_bundle())
            .map_err(|error| error.to_string())
    } else {
        let source = read_cli_source(Path::new(path))?;
        compiler
            .compile(path, &source)
            .map(|built| built.into_bundle())
            .map_err(|error| error.to_string())
    }
}

fn invoke_runner(
    request: &RunnerRequestV1,
    bundle: &ArtifactBundle,
) -> Result<RunnerResponseV1, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate current rss executable: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg("__runner-v1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    let limits = ProcessLimits {
        cpu_seconds: request.limits.wall_time_ms.div_ceil(1000).saturating_add(5),
        address_space_bytes: 1024 * 1024 * 1024,
        open_files: 64,
        file_size_bytes: 2 * 1024 * 1024,
    };
    let mut child = spawn_runner(&mut command, limits)
        .map_err(|error| format!("cannot create guarded runner process: {error}"))?;
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "runner stdin is unavailable".to_string())?;
    let bundle = bundle.to_bytes().map_err(|error| error.to_string())?;
    write_request(&mut stdin, request, &bundle).map_err(|error| error.to_string())?;
    drop(stdin);

    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "runner stdout is unavailable".to_string())?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| "runner stderr is unavailable".to_string())?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_RESPONSE_BYTES + 16));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, RUNNER_STDERR_LIMIT));
    let deadline =
        Instant::now() + Duration::from_millis(request.limits.wall_time_ms.saturating_add(1000));
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("runner wait failed: {error}"))?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            child
                .terminate()
                .map_err(|error| format!("cannot terminate expired runner: {error}"))?;
            // A termination request alone is not containment: retain ownership
            // until the guarded child is reaped and both pipe readers have
            // observed EOF. Otherwise a timed-out CLI invocation could leave
            // detached reader threads behind (and make a failed runner harder
            // to distinguish from a still-running one).
            let status = child
                .wait()
                .map_err(|error| format!("cannot reap expired runner: {error}"))?;
            let stdout = join_runner_output(stdout_reader, "stdout")?;
            let stderr = join_runner_output(stderr_reader, "stderr")?;
            return Err(format_runner_deadline_error(status, &stdout, &stderr));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let status = child
        .wait()
        .map_err(|error| format!("runner wait failed: {error}"))?;
    let stdout = join_runner_output(stdout_reader, "stdout")?;
    let stderr = join_runner_output(stderr_reader, "stderr")?;
    if !status.success() {
        return Err(format!(
            "runner exited with {status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    read_response(stdout.as_slice()).map_err(|error| error.to_string())
}

fn join_runner_output(
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("runner {stream} reader panicked"))?
        .map_err(|error| format!("runner {stream} failed: {error}"))
}

fn format_runner_deadline_error(
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout_bytes = stdout.len();
    if stderr.is_empty() {
        format!(
            "runner process deadline exceeded; terminated and reaped with {status} (discarded {stdout_bytes} stdout bytes)"
        )
    } else {
        format!("runner process deadline exceeded; terminated and reaped with {status}: {stderr}")
    }
}

fn spawn_runner(command: &mut Command, limits: ProcessLimits) -> io::Result<GuardedChild> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        rss_process_guard::spawn_guarded_child_strict(command, limits)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        rss_process_guard::spawn_guarded_child(command, limits)
    }
}

fn read_bounded(mut input: impl Read, maximum: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((maximum as u64) + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runner output exceeds its byte limit",
        ));
    }
    Ok(bytes)
}

pub(crate) fn runner_entrypoint() -> ExitCode {
    let response = match read_request(io::stdin().lock()) {
        Ok((request, bundle)) => execute_request(request, bundle),
        Err(error) => {
            RunnerResponseV1::rejected(RunnerTerminationV1::ProtocolRejected, error.to_string())
        }
    };
    match write_response(io::stdout().lock(), &response) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cannot write runner response: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute_request(request: RunnerRequestV1, bundle: Vec<u8>) -> RunnerResponseV1 {
    let verified = match ArtifactVerifier.verify_bytes(&bundle) {
        Ok(verified) => verified,
        Err(error) => {
            return RunnerResponseV1::rejected(
                RunnerTerminationV1::VerificationRejected,
                error.to_string(),
            );
        }
    };
    let runtime = Runtime::new(profiled_registry(request.profile));
    let linked = match runtime.link(&verified) {
        Ok(linked) => linked,
        Err(error) => {
            return RunnerResponseV1::rejected(
                RunnerTerminationV1::LinkRejected,
                error.to_string(),
            );
        }
    };
    let trace = if request.metadata_only_trace {
        TracePolicy::MetadataOnly
    } else {
        TracePolicy::None
    };
    let report = linked.execute(
        ExecutionRequest::new(request.args)
            .limits(runner_limits(&request.limits))
            .trace(trace),
    );
    match serde_json::to_value(report) {
        Ok(report) => RunnerResponseV1::report(report),
        Err(error) => RunnerResponseV1::rejected(
            RunnerTerminationV1::HostFailure,
            format!("cannot serialize execution report: {error}"),
        ),
    }
}

fn runner_limits(limits: &RunnerLimitsV1) -> RunLimits {
    RunLimits::bounded()
        .with_max_depth(limits.max_depth)
        .with_step_budget(limits.step_budget)
        .with_allocation_budget(limits.allocation_budget)
        .with_live_memory_limit(limits.live_memory_limit)
        .with_output_budget(limits.output_budget)
        .with_intrinsic_call_budget(limits.intrinsic_call_budget)
        .with_provider_call_budget(limits.provider_call_budget)
        .with_resource_limit(limits.resource_limit)
        .with_deadline(MonotonicDeadline::after(Duration::from_millis(
            limits.wall_time_ms,
        )))
}

fn profiled_registry(profile: RunnerProfileV1) -> ProviderRegistry {
    match profile {
        // Provider implementations and all authority remain host-owned. The
        // reference profile intentionally fails closed for external imports.
        RunnerProfileV1::NoProviders => ProviderRegistry::default(),
    }
}

fn finish_response(response: RunnerResponseV1, json: bool) -> ExitCode {
    match response.report {
        Some(report) if response.runner_termination == RunnerTerminationV1::Completed => {
            finish_report(report, json)
        }
        _ => {
            eprintln!(
                "runner {:?}: {}",
                response.runner_termination,
                response.error.as_deref().unwrap_or("no error details")
            );
            ExitCode::from(1)
        }
    }
}

fn finish_report(report: serde_json::Value, json: bool) -> ExitCode {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("execution report serializes")
        );
    } else {
        print!("{}", report["stdout"].as_str().unwrap_or_default());
        eprint!("{}", report["stderr"].as_str().unwrap_or_default());
        println!("{}", report["value"].as_str().unwrap_or_default());
        if let Some(message) = report["failure"]["message"].as_str() {
            eprintln!("{message}");
        }
    }
    if report["termination_reason"] == "completed" {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn request_protocol_has_no_provider_or_dynamic_library_injection_field() {
        let request = RunnerRequestV1::new(Vec::new()).expect("request");
        let json = serde_json::to_string(&request).expect("request JSON");
        assert!(!json.contains("provider_id"));
        assert!(!json.contains("provider_path"));
        assert!(!json.contains("library"));
        assert!(!json.contains("path"));
    }

    #[test]
    fn bounded_reader_rejects_runner_output_over_the_configured_limit() {
        let error = read_bounded(Cursor::new(vec![0_u8; 5]), 4).expect_err("must reject");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("byte limit"));
    }

    #[test]
    fn deadline_error_records_reap_without_treating_child_output_as_a_report() {
        let status = if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", "exit", "1"])
                .status()
                .expect("status")
        } else {
            Command::new("sh")
                .args(["-c", "exit 1"])
                .status()
                .expect("status")
        };
        let error = format_runner_deadline_error(status, b"not a report", b"child diagnostics\n");
        assert!(error.contains("terminated and reaped"));
        assert!(error.contains("child diagnostics"));
        assert!(!error.contains("not a report"));
    }

    #[test]
    fn runner_reverifies_and_rejects_malformed_bundles() {
        let request = RunnerRequestV1::new(Vec::new()).expect("request");
        let response = execute_request(request, b"not an artifact bundle".to_vec());
        assert_eq!(
            response.runner_termination,
            RunnerTerminationV1::VerificationRejected
        );
        assert!(response.report.is_none());
    }
}
