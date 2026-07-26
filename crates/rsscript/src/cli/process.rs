use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) fn run_bounded(
    command: &mut Command,
    operation: &str,
    timeout: Duration,
    output_cap: usize,
) -> Result<BoundedOutput, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start {operation}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("failed to capture {operation} stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("failed to capture {operation} stderr"))?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout_exceeded = Arc::clone(&exceeded);
    let stderr_exceeded = Arc::clone(&exceeded);
    let stdout_reader = thread::spawn(move || read_bounded(stdout, output_cap, &stdout_exceeded));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, output_cap, &stderr_exceeded));
    let deadline = Instant::now() + timeout;

    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed while waiting for {operation}: {error}"))?
        {
            break status;
        }
        if exceeded.load(Ordering::Acquire) {
            terminate(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!(
                "{operation} exceeded the {output_cap} byte output limit per stream"
            ));
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!(
                "{operation} exceeded the {} second deadline",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };

    let (stdout, stdout_overflow) = stdout_reader
        .join()
        .map_err(|_| format!("{operation} stdout reader panicked"))??;
    let (stderr, stderr_overflow) = stderr_reader
        .join()
        .map_err(|_| format!("{operation} stderr reader panicked"))??;
    if stdout_overflow || stderr_overflow {
        return Err(format!(
            "{operation} exceeded the {output_cap} byte output limit per stream"
        ));
    }
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(
    mut input: impl Read,
    cap: usize,
    exceeded: &AtomicBool,
) -> Result<(Vec<u8>, bool), String> {
    let mut output = Vec::with_capacity(cap.min(64 * 1024));
    let mut overflow = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("failed to read child output: {error}"))?;
        if read == 0 {
            break;
        }
        let remaining = cap.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
        if read > remaining {
            overflow = true;
            exceeded.store(true, Ordering::Release);
        }
    }
    Ok((output, overflow))
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{}", child.id()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn command_is_stopped_at_deadline_or_output_limit() {
        let mut sleeping = Command::new("sh");
        sleeping.args(["-c", "sleep 2"]);
        let error = run_bounded(
            &mut sleeping,
            "sleep fixture",
            Duration::from_millis(25),
            1024,
        )
        .expect_err("sleep must time out");
        assert!(error.contains("deadline"), "{error}");

        let mut verbose = Command::new("sh");
        verbose.args(["-c", "printf 123456789"]);
        let error = run_bounded(&mut verbose, "output fixture", Duration::from_secs(2), 4)
            .expect_err("output must be capped");
        assert!(error.contains("output limit"), "{error}");
    }
}
