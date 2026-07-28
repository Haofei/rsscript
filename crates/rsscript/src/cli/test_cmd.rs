use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::Serialize;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const TEST_COMMAND_OUTPUT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// `rss test` is the native productized entry point over `.rsstest.toml`
/// manifests. It mirrors the self-hosted `packages/test-runner` semantics so the
/// same manifests run either way, but skips bootstrapping the self-hosted
/// runner through cargo.
#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    tests: Vec<TestCase>,
}

#[derive(Debug, Deserialize)]
struct TestCase {
    name: String,
    kind: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    directory: String,
    #[serde(default)]
    extension: String,
    #[serde(default = "default_recursive")]
    recursive: bool,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    jobs: i64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    contains: String,
    #[serde(default)]
    contains_all: Vec<String>,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    allow_empty: bool,
}

fn default_recursive() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    // Generous enough that a debug-build `rss` subprocess (or a quick `cargo`
    // invocation) survives a heavily loaded CI runner; commands that actually
    // compile a crate set a larger explicit `timeout_ms` in the manifest.
    60_000
}

#[derive(Debug, Default, Serialize)]
struct Summary {
    total: usize,
    selected: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
}

#[derive(Serialize)]
struct TestResult {
    name: String,
    status: &'static str,
    duration_ms: u128,
}

#[derive(Debug)]
struct Options {
    all: bool,
    filter: String,
    json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestProfile {
    Default,
    All,
}

fn parse_test_args(args: &[String]) -> Result<Options, String> {
    let mut all = false;
    let mut filter = String::new();
    let mut json = false;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--json" => json = true,
            "--all" => all = true,
            "--filter" => {
                index += 1;
                filter = super::required_flag_value(args, index, "--filter")?.to_string();
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument `{other}`."));
            }
            other => return Err(format!("unexpected argument `{other}`.")),
        }
        index += 1;
    }

    Ok(Options { all, filter, json })
}

pub(crate) fn run_test(args: &[String]) -> ExitCode {
    let options = match parse_test_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let manifest_path = match resolve_manifest_path(&options) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    let contents = match fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!(
                "failed to read manifest {}: {error}",
                manifest_path.display()
            );
            return ExitCode::from(2);
        }
    };
    let tests = match parse_manifest(&contents) {
        Ok(manifest) => manifest.tests,
        Err(error) => {
            eprintln!(
                "failed to parse manifest {}: {error}",
                manifest_path.display()
            );
            return ExitCode::from(2);
        }
    };

    let mut summary = Summary {
        total: tests.len(),
        ..Summary::default()
    };
    let mut json_results: Vec<TestResult> = Vec::new();

    for test in &tests {
        if !name_matches_filter(&test.name, &options.filter) {
            continue;
        }
        summary.selected += 1;

        if test.ignored {
            print_line(options.json, "skip", &test.name);
            summary.skipped += 1;
            json_results.push(TestResult {
                name: test.name.clone(),
                status: "skip",
                duration_ms: 0,
            });
            continue;
        }

        let started = Instant::now();
        match run_one(test) {
            Ok(true) => {
                print_line(options.json, "pass", &test.name);
                summary.passed += 1;
                json_results.push(TestResult {
                    name: test.name.clone(),
                    status: "pass",
                    duration_ms: started.elapsed().as_millis(),
                });
            }
            Ok(false) => {
                print_line(options.json, "fail", &test.name);
                summary.failed += 1;
                json_results.push(TestResult {
                    name: test.name.clone(),
                    status: "fail",
                    duration_ms: started.elapsed().as_millis(),
                });
            }
            Err(error) => {
                print_line(options.json, "fail", &test.name);
                if !options.json {
                    eprintln!("  {error}");
                }
                summary.failed += 1;
                json_results.push(TestResult {
                    name: test.name.clone(),
                    status: "fail",
                    duration_ms: started.elapsed().as_millis(),
                });
            }
        }
    }

    if options.json {
        println!("{}", summary_json(&json_results, &summary));
    } else {
        println!("{}", summary_line(&summary));
    }

    if summary.selected == 0 {
        if !options.json {
            eprintln!("no tests matched the selected profile and filter");
        }
        ExitCode::from(1)
    } else if summary.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn resolve_manifest_path(options: &Options) -> Result<PathBuf, String> {
    let profile = if options.all {
        TestProfile::All
    } else {
        TestProfile::Default
    };
    let manifest = profile_manifest(profile);
    if manifest.exists() {
        return Ok(manifest.to_path_buf());
    }
    Err(format!(
        "missing RSScript test manifest {}",
        manifest.display()
    ))
}

fn profile_manifest(profile: TestProfile) -> &'static Path {
    Path::new(match profile {
        TestProfile::Default => "packages/test-runner/manifests/default.rsstest.toml",
        TestProfile::All => "packages/test-runner/manifests/all.rsstest.toml",
    })
}

fn parse_manifest(contents: &str) -> Result<Manifest, String> {
    toml::from_str(contents).map_err(|error| error.to_string())
}

fn name_matches_filter(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.contains(filter)
}

fn text_matches(value: &str, contains: &str, contains_all: &[String]) -> bool {
    if !contains.is_empty() && !value.contains(contains) {
        return false;
    }
    contains_all.iter().all(|needle| value.contains(needle))
}

/// Outcome of one spawned process: success plus captured streams. `rss test`
/// searches stdout and stderr together so a diagnostic printed to either is
/// assertable.
struct CommandOutcome {
    success: bool,
    code: Option<i32>,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

impl CommandOutcome {
    /// Trimmed stdout/stderr combined, mirroring the runtime's
    /// `process_output_details` so failure text is assertable the same way.
    fn details(&self) -> String {
        let stdout = self.stdout.trim();
        let stderr = self.stderr.trim();
        if stdout.is_empty() {
            stderr.to_string()
        } else if stderr.is_empty() {
            stdout.to_string()
        } else {
            format!("{stdout}\n{stderr}")
        }
    }

    /// Failure message mirroring the runtime's `process_output_result` /
    /// timeout error so manifests assert against the same text either way.
    fn failure_message(&self, command: &str, timeout_ms: u64) -> String {
        if self.timed_out {
            return format!(
                "`{command}` timed out after {timeout_ms}ms: {}",
                self.details()
            );
        }
        let code = self
            .code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        format!("`{command}` exited with {code}: {}", self.details())
    }
}

fn run_one(test: &TestCase) -> Result<bool, String> {
    match test.kind.as_str() {
        "file_contains" => {
            let text = fs::read_to_string(&test.path)
                .map_err(|error| format!("read file {}: {error}", test.path))?;
            Ok(text_matches(&text, &test.contains, &test.contains_all))
        }
        "command_success" => {
            let outcome = run_command(&test.command, &test.args, test.timeout_ms)?;
            if outcome.success {
                Ok(true)
            } else {
                Err(outcome.failure_message(&test.command, test.timeout_ms))
            }
        }
        "command_stdout_contains" => {
            let outcome = run_command(&test.command, &test.args, test.timeout_ms)?;
            if !outcome.success {
                return Err(outcome.failure_message(&test.command, test.timeout_ms));
            }
            Ok(text_matches(
                &outcome.stdout,
                &test.contains,
                &test.contains_all,
            ))
        }
        "rss_run_success" => {
            let outcome = run_rss(test)?;
            if outcome.success {
                Ok(true)
            } else {
                Err(outcome.failure_message(&rss_command_label(&test.command), test.timeout_ms))
            }
        }
        "rss_run_stdout_contains" => {
            let outcome = run_rss(test)?;
            if !outcome.success {
                return Err(
                    outcome.failure_message(&rss_command_label(&test.command), test.timeout_ms)
                );
            }
            Ok(text_matches(
                &outcome.stdout,
                &test.contains,
                &test.contains_all,
            ))
        }
        "rss_run_failure_contains" => {
            let outcome = run_rss(test)?;
            let failure =
                outcome.failure_message(&rss_command_label(&test.command), test.timeout_ms);
            // The test asserts the run failed; succeeding is itself a failure.
            Ok(!outcome.success && text_matches(&failure, &test.contains, &test.contains_all))
        }
        "command_for_each_path" => run_each(
            &test.command,
            &test.args,
            &test.paths,
            test.jobs,
            test.timeout_ms,
            test.allow_empty,
        ),
        "command_for_each_file" => {
            let files = collect_each_file(&test.directory, &test.extension, test.recursive)?;
            run_each(
                &test.command,
                &test.args,
                &files,
                test.jobs,
                test.timeout_ms,
                test.allow_empty,
            )
        }
        other => Err(format!("unsupported test kind `{other}`")),
    }
}

fn run_rss(test: &TestCase) -> Result<CommandOutcome, String> {
    let command = rss_binary(&test.command);
    let mut args = vec!["run".to_string(), test.path.clone()];
    args.extend(test.args.iter().cloned());
    run_command_os(&command, &args, test.timeout_ms)
}

/// Resolve the `rss` binary: an explicit `command` wins, otherwise reuse the
/// currently running executable so `rss test` drives the same build it is.
fn rss_binary(command: &str) -> OsString {
    if !command.is_empty() {
        return OsString::from(command);
    }
    env::current_exe()
        .ok()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("target/debug/rss"))
}

/// Human-readable command label for `rss run` failure messages.
fn rss_command_label(command: &str) -> String {
    rss_binary(command).to_string_lossy().into_owned()
}

fn run_command(command: &str, args: &[String], timeout_ms: u64) -> Result<CommandOutcome, String> {
    if command.is_empty() {
        return Err("test is missing a `command`".to_string());
    }
    run_command_os(&OsString::from(command), args, timeout_ms)
}

fn run_command_os(
    command: &OsString,
    args: &[String],
    timeout_ms: u64,
) -> Result<CommandOutcome, String> {
    let (mut child, guard) = spawn_with_fallback(command, args)?;

    // Drain stdout/stderr on threads so a full pipe cannot deadlock the wait.
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_reader(child.stdout.take(), Arc::clone(&output_exceeded));
    let stderr_reader = spawn_reader(child.stderr.take(), Arc::clone(&output_exceeded));

    // `timeout_ms == 0` disables the timeout, matching the runtime's
    // `process_run_stdout_timeout` (`timeout_ms <= 0` runs without a deadline).
    let deadline = (timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if output_exceeded.load(Ordering::Acquire) {
                    terminate_process_tree(&mut child, &guard);
                    let _ = child.wait();
                    break None;
                }
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    terminate_process_tree(&mut child, &guard);
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => return Err(format!("failed to wait on process: {error}")),
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if output_exceeded.load(Ordering::Acquire) {
        return Err(format!(
            "test command exceeded output limit of {TEST_COMMAND_OUTPUT_MAX_BYTES} bytes per stream"
        ));
    }

    Ok(CommandOutcome {
        success: status.is_some_and(|status| status.success()),
        code: status.and_then(|status| status.code()),
        timed_out,
        stdout,
        stderr,
    })
}

/// Spawn `command`, falling back to `command + ".exe"` if the first spawn fails,
/// matching the self-hosted runner's Windows fallback. The original error is
/// preserved if the fallback also fails.
fn spawn_with_fallback(
    command: &OsString,
    args: &[String],
) -> Result<(std::process::Child, rss_process_guard::ProcessGuard), String> {
    match spawn_piped(command, args) {
        Ok(child) => Ok(child),
        Err(first_error) => {
            if command.to_string_lossy().ends_with(".exe") {
                return Err(format!(
                    "failed to spawn {}: {first_error}",
                    command.to_string_lossy()
                ));
            }
            let mut exe = command.clone();
            exe.push(".exe");
            spawn_piped(&exe, args).map_err(|_| {
                format!(
                    "failed to spawn {}: {first_error}",
                    command.to_string_lossy()
                )
            })
        }
    }
}

fn spawn_piped(
    command: &OsString,
    args: &[String],
) -> std::io::Result<(std::process::Child, rss_process_guard::ProcessGuard)> {
    let mut command = Command::new(command);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    rss_process_guard::spawn_guarded(
        &mut command,
        rss_process_guard::ProcessLimits::generated_program(),
    )
}

fn terminate_process_tree(
    child: &mut std::process::Child,
    guard: &rss_process_guard::ProcessGuard,
) {
    let _ = guard.terminate();
    let _ = child.kill();
}

fn spawn_reader<R: Read + Send + 'static>(
    stream: Option<R>,
    output_exceeded: Arc<AtomicBool>,
) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            let mut chunk = [0_u8; 8192];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let remaining = TEST_COMMAND_OUTPUT_MAX_BYTES.saturating_sub(bytes.len());
                        if read > remaining {
                            bytes.extend_from_slice(&chunk[..remaining]);
                            output_exceeded.store(true, Ordering::Release);
                            break;
                        }
                        bytes.extend_from_slice(&chunk[..read]);
                    }
                }
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Run `command` once per appended item, succeeding only if every invocation
/// succeeds. `jobs >= 1` runs that many concurrently; `jobs <= 0` auto-sizes to
/// the available parallelism. The first failure's detail is reported.
fn run_each(
    command: &str,
    args: &[String],
    items: &[String],
    jobs: i64,
    timeout_ms: u64,
    allow_empty: bool,
) -> Result<bool, String> {
    if command.is_empty() {
        return Err("test is missing a `command`".to_string());
    }
    if items.is_empty() {
        return if allow_empty {
            Ok(true)
        } else {
            Err(
                "per-item test selected no inputs; set `allow_empty = true` only when intentional"
                    .to_string(),
            )
        };
    }

    let worker_count = resolve_jobs(jobs, items.len());
    let next = AtomicUsize::new(0);
    let first_failure: Mutex<Option<String>> = Mutex::new(None);

    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    let mut invocation = args.to_vec();
                    invocation.push(item.clone());
                    let outcome = run_command(command, &invocation, timeout_ms);
                    let failure = match outcome {
                        Ok(result) if result.success => None,
                        Ok(result) => Some(format!(
                            "`{command}` failed for `{item}`: {}",
                            first_line(&result.details())
                        )),
                        Err(error) => Some(error),
                    };
                    if let Some(message) = failure {
                        let mut slot = first_failure.lock().expect("failure mutex poisoned");
                        if slot.is_none() {
                            *slot = Some(message);
                        }
                    }
                }
            });
        }
    });

    match first_failure.into_inner().expect("failure mutex poisoned") {
        Some(message) => Err(message),
        None => Ok(true),
    }
}

fn resolve_jobs(jobs: i64, item_count: usize) -> usize {
    let requested = if jobs >= 1 {
        jobs as usize
    } else {
        thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
    };
    requested.clamp(1, item_count.max(1))
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

fn collect_each_file(
    directory: &str,
    extension: &str,
    recursive: bool,
) -> Result<Vec<String>, String> {
    let root = Path::new(directory);
    if !root.is_dir() {
        return Err(format!("directory not found: {directory}"));
    }
    let mut matched = Vec::new();
    collect_each_file_into(root, extension, recursive, &mut matched)?;
    matched.sort();
    Ok(matched
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

fn collect_each_file_into(
    dir: &Path,
    extension: &str,
    recursive: bool,
    matched: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("read directory {}: {error}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("read directory entry in {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                collect_each_file_into(&path, extension, recursive, matched)?;
            }
        } else if file_has_extension(&path, extension) {
            matched.push(path);
        }
    }
    Ok(())
}

fn file_has_extension(path: &Path, extension: &str) -> bool {
    if extension.is_empty() {
        return true;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(extension))
        .unwrap_or(false)
}

fn print_line(json: bool, status: &str, name: &str) {
    if !json {
        println!("{status} {name}");
    }
}

fn summary_line(summary: &Summary) -> String {
    format!(
        "rss test summary total={} selected={} passed={} failed={} skipped={}",
        summary.total, summary.selected, summary.passed, summary.failed, summary.skipped
    )
}

fn summary_json(results: &[TestResult], summary: &Summary) -> String {
    serde_json::to_string(&serde_json::json!({
        "total": summary.total,
        "selected": summary.selected,
        "passed": summary.passed,
        "failed": summary.failed,
        "skipped": summary.skipped,
        "tests": results,
    }))
    .expect("test summary should serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn strings(values: &[&str]) -> Vec<String> {
        args(values)
    }

    #[test]
    fn parse_test_args_reads_filter() {
        let values = args(&["--filter", "lint"]);
        let options = super::parse_test_args(&values).expect("arguments should parse");

        assert_eq!(options.filter, "lint");
        assert!(!options.json);
    }

    #[test]
    fn parse_test_args_rejects_positionals() {
        let values = args(&["manifest.rsstest.toml"]);
        let error = super::parse_test_args(&values).expect_err("positional should fail");

        assert_eq!(error, "unexpected argument `manifest.rsstest.toml`.");
    }

    #[test]
    fn parse_test_args_accepts_all() {
        let values = args(&["--all", "--filter", "lint"]);
        let options = super::parse_test_args(&values).expect("all flag should parse");

        assert!(options.all);
        assert_eq!(options.filter, "lint");
    }

    #[test]
    fn parse_test_args_rejects_unknown_flag() {
        let values = args(&["--watch"]);
        let error = super::parse_test_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--watch`.");
    }

    #[test]
    fn parse_manifest_reads_defaults() {
        let manifest = super::parse_manifest(
            "[[tests]]\nname = \"a\"\nkind = \"command_success\"\ncommand = \"true\"\n",
        )
        .expect("manifest should parse");

        assert_eq!(manifest.tests.len(), 1);
        let test = &manifest.tests[0];
        assert_eq!(test.name, "a");
        assert!(test.recursive, "recursive should default to true");
        assert_eq!(test.timeout_ms, 60_000);
        assert!(!test.ignored);
    }

    #[test]
    fn command_outcome_details_combines_trimmed_streams() {
        let outcome = super::CommandOutcome {
            success: false,
            code: Some(1),
            timed_out: false,
            stdout: "  out line  \n".to_string(),
            stderr: "  err line  \n".to_string(),
        };

        assert_eq!(outcome.details(), "out line\nerr line");
    }

    #[test]
    fn command_outcome_failure_message_reports_code_and_output() {
        let outcome = super::CommandOutcome {
            success: false,
            code: Some(3),
            timed_out: false,
            stdout: String::new(),
            stderr: "boom".to_string(),
        };

        assert_eq!(
            outcome.failure_message("git", 10_000),
            "`git` exited with 3: boom"
        );
    }

    #[test]
    fn command_outcome_failure_message_reports_timeout() {
        let outcome = super::CommandOutcome {
            success: false,
            code: None,
            timed_out: true,
            stdout: "partial".to_string(),
            stderr: String::new(),
        };

        assert_eq!(
            outcome.failure_message("cargo", 250),
            "`cargo` timed out after 250ms: partial"
        );
    }

    #[cfg(unix)]
    #[test]
    fn command_success_failure_surfaces_output_as_error() {
        // A command that exits non-zero: portable across the unix-like CI hosts.
        let manifest = "[[tests]]\nname = \"fails\"\nkind = \"command_success\"\ncommand = \"sh\"\nargs = [\"-c\", \"echo nope 1>&2; exit 7\"]\n";
        let parsed = super::parse_manifest(manifest).expect("manifest should parse");

        let error = super::run_one(&parsed.tests[0])
            .expect_err("non-zero command_success should surface an error");
        assert!(error.contains("exited with 7"), "got: {error}");
        assert!(error.contains("nope"), "got: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn zero_timeout_disables_deadline() {
        // `timeout_ms = 0` must not kill a command that runs longer than 1ms.
        let manifest = "[[tests]]\nname = \"slow ok\"\nkind = \"command_success\"\ncommand = \"sh\"\nargs = [\"-c\", \"sleep 0.2\"]\ntimeout_ms = 0\n";
        let parsed = super::parse_manifest(manifest).expect("manifest should parse");

        let result = super::run_one(&parsed.tests[0]).expect("zero timeout should not error");
        assert!(result, "command should succeed without being timed out");
    }

    #[test]
    fn text_matches_requires_contains_and_all() {
        assert!(super::text_matches(
            "hello world",
            "hello",
            &strings(&["world"])
        ));
        assert!(!super::text_matches("hello world", "missing", &[]));
        assert!(!super::text_matches(
            "hello world",
            "hello",
            &strings(&["absent"])
        ));
    }

    #[test]
    fn name_matches_filter_is_substring() {
        assert!(super::name_matches_filter("rss script examples lint", ""));
        assert!(super::name_matches_filter(
            "rss script examples lint",
            "lint"
        ));
        assert!(!super::name_matches_filter(
            "rss script examples lint",
            "package"
        ));
    }

    #[test]
    fn resolve_jobs_clamps_to_item_count() {
        assert_eq!(super::resolve_jobs(1, 5), 1);
        assert_eq!(super::resolve_jobs(8, 3), 3);
        assert_eq!(super::resolve_jobs(0, 1), 1);
    }

    #[test]
    fn collect_each_file_filters_extension_and_recursion() {
        let root = unique_temp_dir("test-collect-files");
        fs::write(root.join("a.rss"), "x").expect("write a");
        fs::write(root.join("b.txt"), "x").expect("write b");
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("create nested");
        fs::write(nested.join("c.rss"), "x").expect("write c");

        let shallow = super::collect_each_file(root.to_str().unwrap(), ".rss", false)
            .expect("collect shallow");
        assert_eq!(shallow.len(), 1);
        assert!(shallow[0].ends_with("a.rss"));

        let deep =
            super::collect_each_file(root.to_str().unwrap(), ".rss", true).expect("collect deep");
        assert_eq!(deep.len(), 2);

        fs::remove_dir_all(root).expect("clean up temp dir");
    }

    #[test]
    fn summary_line_matches_self_hosted_format() {
        let summary = super::Summary {
            total: 3,
            selected: 2,
            passed: 1,
            failed: 1,
            skipped: 0,
        };

        assert_eq!(
            super::summary_line(&summary),
            "rss test summary total=3 selected=2 passed=1 failed=1 skipped=0"
        );
    }

    #[test]
    fn summary_json_includes_per_test_duration() {
        let summary = super::Summary {
            total: 1,
            selected: 1,
            passed: 1,
            ..super::Summary::default()
        };
        let results = [super::TestResult {
            name: "fast check".to_string(),
            status: "pass",
            duration_ms: 42,
        }];

        assert_eq!(
            super::summary_json(&results, &summary),
            "{\"total\":1,\"selected\":1,\"passed\":1,\"failed\":0,\"skipped\":0,\"tests\":[{\"name\":\"fast check\",\"status\":\"pass\",\"duration_ms\":42}]}"
        );
    }

    #[test]
    fn summary_json_escapes_all_control_characters() {
        let summary = super::Summary {
            total: 1,
            selected: 1,
            passed: 1,
            ..super::Summary::default()
        };
        let results = [super::TestResult {
            name: "control\u{0001}name".to_string(),
            status: "pass",
            duration_ms: 1,
        }];
        let json = super::summary_json(&results, &summary);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("summary must be JSON");
        assert_eq!(parsed["tests"][0]["name"], "control\u{0001}name");
    }

    #[test]
    fn per_item_test_rejects_an_empty_input_set_by_default() {
        let error = super::run_each("echo", &[], &[], 1, 1_000, false)
            .expect_err("empty input should fail closed");
        assert!(error.contains("selected no inputs"));
        assert!(
            super::run_each("echo", &[], &[], 1, 1_000, true)
                .expect("explicitly allowed empty input should pass")
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_pipe_inheriting_descendants() {
        let started = Instant::now();
        let outcome = super::run_command(
            "sh",
            &["-c".to_string(), "(sleep 30) & wait".to_string()],
            50,
        )
        .expect("timed command should return");
        assert!(outcome.timed_out);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn file_contains_kind_runs_end_to_end() {
        let root = unique_temp_dir("test-file-contains");
        let target = root.join("data.txt");
        fs::write(&target, "the answer is 42\n").expect("write target");
        let manifest = format!(
            "[[tests]]\nname = \"answer present\"\nkind = \"file_contains\"\npath = {}\ncontains = \"42\"\n",
            serde_json::to_string(target.to_str().unwrap()).expect("path should serialize")
        );
        let parsed = super::parse_manifest(&manifest).expect("manifest should parse");

        let result = super::run_one(&parsed.tests[0]).expect("file_contains should not error");
        assert!(result);

        fs::remove_dir_all(root).expect("clean up temp dir");
    }

    #[test]
    fn command_output_reader_stops_at_the_capture_limit() {
        let exceeded = Arc::new(AtomicBool::new(false));
        let input = std::io::Cursor::new(vec![b'x'; TEST_COMMAND_OUTPUT_MAX_BYTES + 1]);
        let reader = spawn_reader(Some(input), Arc::clone(&exceeded));
        let output = reader.join().expect("reader thread should complete");

        assert_eq!(output.len(), TEST_COMMAND_OUTPUT_MAX_BYTES);
        assert!(exceeded.load(Ordering::Acquire));
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp directory should create");
        path
    }
}
