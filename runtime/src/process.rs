use std::time::{Duration, Instant};

pub fn os_close(fd: i64) {
    let _ = fd;
}

pub fn args_count() -> i64 {
    std::env::args().skip(1).count() as i64
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

pub fn process_run_stdout(command: &str, args: &[String]) -> Result<String, String> {
    let mut child = std::process::Command::new(command);
    child.args(args);
    if std::env::var_os("RSSCRIPT_RAMDISK_PATH").is_none()
        && let Some(path) = default_ramdisk_root_dir()
    {
        child.env("RSSCRIPT_RAMDISK_PATH", path);
    }

    let output = child
        .output()
        .map_err(|error| format!("failed to run `{command}`: {error}"))?;
    process_output_result(command, output)
}

pub fn process_run_stdout_timeout(
    command: &str,
    args: &[String],
    timeout_ms: i64,
) -> Result<String, String> {
    if timeout_ms <= 0 {
        return process_run_stdout(command, args);
    }

    let timeout_ms = u64::try_from(timeout_ms).unwrap_or(0);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut child = std::process::Command::new(command);
    child
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if std::env::var_os("RSSCRIPT_RAMDISK_PATH").is_none()
        && let Some(path) = default_ramdisk_root_dir()
    {
        child.env("RSSCRIPT_RAMDISK_PATH", path);
    }
    let mut child = child
        .spawn()
        .map_err(|error| format!("failed to run `{command}`: {error}"))?;
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("failed to poll `{command}`: {error}"))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .map_err(|error| format!("failed to collect `{command}` output: {error}"))?;
            return process_output_result(command, output);
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|error| format!("failed to collect `{command}` output: {error}"))?;
            return Err(format!(
                "`{command}` timed out after {timeout_ms}ms: {}",
                process_output_details(&output)
            ));
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(10)));
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

pub fn process_run_many_stdout(
    command: &str,
    args: &[String],
    appended_args: &[String],
    jobs: i64,
) -> Result<Vec<String>, String> {
    process_run_many_stdout_with_runner(command, args, appended_args, jobs, process_run_stdout)
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
    println!("{message}");
}
