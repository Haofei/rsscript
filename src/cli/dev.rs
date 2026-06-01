use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::check::run_check;
use super::lint::run_lint;
use super::run_cmd::run_generated_rust;
use super::{is_package_directory, print_usage};

/// Action `rss dev` re-runs on every change. The default is the pure frontend
/// `check` loop, which never invokes cargo, so the inner edit loop stays fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DevAction {
    Check,
    Run,
}

#[derive(Debug)]
struct DevOptions<'a> {
    action: DevAction,
    lint: bool,
    json: bool,
    release: bool,
    use_core: bool,
    interfaces: Vec<&'a str>,
    path: Option<&'a str>,
    once: bool,
}

fn parse_dev_args(args: &[String]) -> Result<DevOptions<'_>, String> {
    let mut action = DevAction::Check;
    let mut lint = false;
    let mut json = false;
    let mut release = false;
    let mut use_core = true;
    let mut interfaces = Vec::new();
    let mut path = None;
    let mut once = false;
    let mut frontend_flags_seen = false;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--run" => action = DevAction::Run,
            "--lint" => lint = true,
            "--json" => json = true,
            "--release" => release = true,
            "--core" => {
                use_core = true;
                frontend_flags_seen = true;
            }
            "--no-core" => {
                use_core = false;
                frontend_flags_seen = true;
            }
            "--once" => once = true,
            "--interface" => {
                index += 1;
                let value = super::required_flag_value(args, index, "--interface")?;
                interfaces.push(value);
                frontend_flags_seen = true;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument `{other}`."));
            }
            other if path.is_none() => path = Some(other),
            other => return Err(format!("unexpected extra path `{other}`.")),
        }
        index += 1;
    }

    let options = DevOptions {
        action,
        lint,
        json,
        release,
        use_core,
        interfaces,
        path,
        once,
    };
    validate_dev_options(&options, frontend_flags_seen)?;
    Ok(options)
}

/// Reject flag combinations whose behavior would be silently dropped or whose
/// output would be corrupted, so `rss dev` never accepts an argument it ignores.
fn validate_dev_options(options: &DevOptions<'_>, frontend_flags_seen: bool) -> Result<(), String> {
    if options.json && !options.once {
        return Err(
            "`rss dev --json` requires `--once`; watch mode interleaves human status lines that would corrupt JSON output."
                .to_string(),
        );
    }
    if options.action == DevAction::Run && options.lint {
        return Err(
            "`rss dev --run` cannot be combined with `--lint`; run a `rss dev --lint` loop or `rss dev --run` loop separately."
                .to_string(),
        );
    }
    if options.release && options.action != DevAction::Run {
        return Err("`rss dev --release` only applies to `--run`.".to_string());
    }
    if options.action == DevAction::Run && frontend_flags_seen {
        return Err(
            "`rss dev --run` does not forward `--core`, `--no-core`, or `--interface`; use a `rss dev` check loop for those."
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn run_dev(args: &[String]) -> ExitCode {
    let options = match parse_dev_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(path) = options.path else {
        print_usage();
        return ExitCode::from(2);
    };

    if options.once {
        return run_once(&options, path);
    }

    let mut last_signature: Option<BTreeMap<PathBuf, u128>> = None;
    let poll = Duration::from_millis(300);
    loop {
        let signature = watch_signature(path, &options.interfaces);
        if last_signature.as_ref() != Some(&signature) {
            let _ = run_once(&options, path);
            print_watch_footer(path);
            last_signature = Some(signature);
        }
        thread::sleep(poll);
    }
}

/// Run the configured action(s) once and return the aggregate exit code.
fn run_once(options: &DevOptions<'_>, path: &str) -> ExitCode {
    if !options.json {
        clear_screen();
        println!("[rss dev] {} - {}", action_label(options), path);
    }
    let started = Instant::now();

    let status = match options.action {
        // `run_lint` already emits the frontend diagnostics, so the lint loop
        // runs lint alone rather than check-then-lint to avoid duplicate output.
        DevAction::Check if options.lint && !is_package_directory(path) => {
            run_lint(&check_args(options, path))
        }
        DevAction::Check => {
            if options.lint {
                eprintln!(
                    "[rss dev] --lint is skipped for package directories; lint runs per source file."
                );
            }
            run_check(&check_args(options, path))
        }
        DevAction::Run => run_generated_rust(&run_args(options, path)),
    };

    if !options.json {
        let elapsed = started.elapsed();
        let ok = succeeded(&status);
        println!(
            "[rss dev] {} in {} ms",
            if ok { "ok" } else { "failed" },
            elapsed.as_millis()
        );
    }
    status
}

fn check_args(options: &DevOptions<'_>, path: &str) -> Vec<String> {
    let mut args = Vec::new();
    if options.json {
        args.push("--json".to_string());
    }
    if options.use_core {
        args.push("--core".to_string());
    } else {
        args.push("--no-core".to_string());
    }
    for interface in &options.interfaces {
        args.push("--interface".to_string());
        args.push((*interface).to_string());
    }
    args.push(path.to_string());
    args
}

fn run_args(options: &DevOptions<'_>, path: &str) -> Vec<String> {
    let mut args = Vec::new();
    if options.json {
        args.push("--json".to_string());
    }
    if options.release {
        args.push("--release".to_string());
    }
    args.push(path.to_string());
    args
}

fn action_label(options: &DevOptions<'_>) -> &'static str {
    match (options.action, options.lint) {
        (DevAction::Run, _) => "run",
        (DevAction::Check, true) => "check + lint",
        (DevAction::Check, false) => "check",
    }
}

fn clear_screen() {
    // ANSI clear screen + move cursor home; harmless on terminals without it.
    print!("\x1b[2J\x1b[H");
}

fn print_watch_footer(path: &str) {
    println!("[rss dev] watching {path} (Ctrl-C to stop)");
}

fn succeeded(status: &ExitCode) -> bool {
    *status == ExitCode::SUCCESS
}

/// Build a deterministic snapshot of every watched source file and its
/// modification time. Recollecting each tick lets newly created or deleted
/// files trigger a rerun, not just edits to known files.
fn watch_signature(path: &str, interfaces: &[&str]) -> BTreeMap<PathBuf, u128> {
    let mut files = collect_watch_paths(Path::new(path));
    for interface in interfaces {
        files.push(PathBuf::from(interface));
    }

    let mut signature = BTreeMap::new();
    for file in files {
        let modified = fs::metadata(&file)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        signature.insert(file, modified);
    }
    signature
}

fn collect_watch_paths(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }
    if !root.is_dir() {
        return Vec::new();
    }

    let mut paths = Vec::new();
    collect_watch_paths_into(root, &mut paths);
    paths
}

fn collect_watch_paths_into(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if is_skipped_dir(&path) {
                continue;
            }
            collect_watch_paths_into(&path, paths);
        } else if is_watched_file(&path) {
            paths.push(path);
        }
    }
}

fn is_skipped_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("target" | ".git" | "rsscript-generated-target" | "rsscript-temp")
    )
}

fn is_watched_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("rss" | "rssi" | "toml" | "lock")
    )
}

#[allow(dead_code)]
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_dev_args_defaults_to_check() {
        let values = args(&["demo.rss"]);
        let options = super::parse_dev_args(&values).expect("arguments should parse");

        assert_eq!(options.action, super::DevAction::Check);
        assert!(!options.lint);
        assert!(options.use_core);
        assert_eq!(options.path, Some("demo.rss"));
    }

    #[test]
    fn parse_dev_args_collects_lint_and_interfaces() {
        let values = args(&["--lint", "--interface", "api.rssi", "demo.rss"]);
        let options = super::parse_dev_args(&values).expect("arguments should parse");

        assert_eq!(options.action, super::DevAction::Check);
        assert!(options.lint);
        assert_eq!(options.interfaces, vec!["api.rssi"]);
        assert_eq!(options.path, Some("demo.rss"));
    }

    #[test]
    fn parse_dev_args_accepts_run_with_release() {
        let values = args(&["--run", "--release", "pkg"]);
        let options = super::parse_dev_args(&values).expect("arguments should parse");

        assert_eq!(options.action, super::DevAction::Run);
        assert!(options.release);
        assert_eq!(options.path, Some("pkg"));
    }

    #[test]
    fn parse_dev_args_rejects_json_without_once() {
        let values = args(&["--json", "demo.rss"]);
        let error = super::parse_dev_args(&values).expect_err("json without once should fail");

        assert!(error.contains("`rss dev --json` requires `--once`"));
    }

    #[test]
    fn parse_dev_args_accepts_json_with_once() {
        let values = args(&["--json", "--once", "demo.rss"]);
        let options = super::parse_dev_args(&values).expect("arguments should parse");

        assert!(options.json);
        assert!(options.once);
    }

    #[test]
    fn parse_dev_args_rejects_run_with_lint() {
        let values = args(&["--run", "--lint", "pkg"]);
        let error = super::parse_dev_args(&values).expect_err("run with lint should fail");

        assert!(error.contains("`rss dev --run` cannot be combined with `--lint`"));
    }

    #[test]
    fn parse_dev_args_rejects_release_without_run() {
        let values = args(&["--release", "demo.rss"]);
        let error = super::parse_dev_args(&values).expect_err("release without run should fail");

        assert!(error.contains("`rss dev --release` only applies to `--run`"));
    }

    #[test]
    fn parse_dev_args_rejects_frontend_flags_under_run() {
        for flag in [
            args(&["--run", "--no-core", "pkg"]),
            args(&["--run", "--core", "pkg"]),
            args(&["--run", "--interface", "api.rssi", "pkg"]),
        ] {
            let error =
                super::parse_dev_args(&flag).expect_err("frontend flag under run should fail");
            assert!(error.contains("does not forward"));
        }
    }

    #[test]
    fn parse_dev_args_rejects_missing_interface_value() {
        let values = args(&["--interface", "--json", "demo.rss"]);
        let error = super::parse_dev_args(&values).expect_err("missing interface should fail");

        assert_eq!(error, "missing value for `--interface`.");
    }

    #[test]
    fn parse_dev_args_rejects_unknown_flag() {
        let values = args(&["--watch", "demo.rss"]);
        let error = super::parse_dev_args(&values).expect_err("unknown flag should fail");

        assert_eq!(error, "unknown argument `--watch`.");
    }

    #[test]
    fn check_args_forward_core_and_interfaces() {
        let values = args(&["--no-core", "--interface", "api.rssi", "demo.rss"]);
        let options = super::parse_dev_args(&values).expect("arguments should parse");
        let forwarded = super::check_args(&options, "demo.rss");

        assert_eq!(
            forwarded,
            vec!["--no-core", "--interface", "api.rssi", "demo.rss"]
        );
    }

    #[test]
    fn watch_signature_tracks_added_files() {
        let root = unique_temp_dir("dev-watch-signature");
        fs::write(root.join("a.rss"), "fn main() -> Unit {}\n").expect("write source");

        let before = super::watch_signature(root.to_str().unwrap(), &[]);
        assert_eq!(before.len(), 1);

        fs::write(root.join("b.rss"), "fn helper() -> Unit {}\n").expect("write source");
        let after = super::watch_signature(root.to_str().unwrap(), &[]);
        assert_eq!(after.len(), 2);
        assert_ne!(before, after);

        fs::remove_dir_all(root).expect("clean up temp dir");
    }

    #[test]
    fn collect_watch_paths_skips_target_and_non_source() {
        let root = unique_temp_dir("dev-collect-paths");
        fs::write(root.join("main.rss"), "fn main() -> Unit {}\n").expect("write source");
        fs::write(root.join("notes.txt"), "ignored").expect("write text");
        let target = root.join("target");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("generated.rss"), "skip").expect("write generated");

        let mut collected = super::collect_watch_paths(&root);
        collected.sort();

        assert_eq!(collected, vec![root.join("main.rss")]);

        fs::remove_dir_all(root).expect("clean up temp dir");
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = super::now_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp directory should create");
        path
    }
}
