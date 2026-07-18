use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

const POLL_INTERVAL: Duration = Duration::from_millis(20);

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

#[derive(Debug, Default)]
struct Summary {
    total: usize,
    selected: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
}

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

    if summary.failed > 0 {
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
        ),
        "command_for_each_file" => {
            let files = collect_each_file(&test.directory, &test.extension, test.recursive)?;
            run_each(
                &test.command,
                &test.args,
                &files,
                test.jobs,
                test.timeout_ms,
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
    let mut child = spawn_with_fallback(command, args)?;

    // Drain stdout/stderr on threads so a full pipe cannot deadlock the wait.
    let stdout_reader = spawn_reader(child.stdout.take());
    let stderr_reader = spawn_reader(child.stderr.take());

    // `timeout_ms == 0` disables the timeout, matching the runtime's
    // `process_run_stdout_timeout` (`timeout_ms <= 0` runs without a deadline).
    let deadline = (timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    let _ = child.kill();
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
fn spawn_with_fallback(command: &OsString, args: &[String]) -> Result<std::process::Child, String> {
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

fn spawn_piped(command: &OsString, args: &[String]) -> std::io::Result<std::process::Child> {
    Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

fn spawn_reader<R: Read + Send + 'static>(stream: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_string(&mut buffer);
        }
        buffer
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
) -> Result<bool, String> {
    if command.is_empty() {
        return Err("test is missing a `command`".to_string());
    }
    if items.is_empty() {
        return Ok(true);
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
    for entry in entries.flatten() {
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
    let mut tests = String::from("[");
    for (index, result) in results.iter().enumerate() {
        if index > 0 {
            tests.push(',');
        }
        tests.push_str(&format!(
            "{{\"name\":{},\"status\":\"{}\",\"duration_ms\":{}}}",
            json_string(&result.name),
            result.status,
            result.duration_ms,
        ));
    }
    tests.push(']');
    format!(
        "{{\"total\":{},\"selected\":{},\"passed\":{},\"failed\":{},\"skipped\":{},\"tests\":{tests}}}",
        summary.total, summary.selected, summary.passed, summary.failed, summary.skipped
    )
}

fn json_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
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
    fn file_contains_kind_runs_end_to_end() {
        let root = unique_temp_dir("test-file-contains");
        let target = root.join("data.txt");
        fs::write(&target, "the answer is 42\n").expect("write target");
        let manifest = format!(
            "[[tests]]\nname = \"answer present\"\nkind = \"file_contains\"\npath = {}\ncontains = \"42\"\n",
            super::json_string(target.to_str().unwrap())
        );
        let parsed = super::parse_manifest(&manifest).expect("manifest should parse");

        let result = super::run_one(&parsed.tests[0]).expect("file_contains should not error");
        assert!(result);

        fs::remove_dir_all(root).expect("clean up temp dir");
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
