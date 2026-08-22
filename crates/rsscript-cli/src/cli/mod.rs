use std::env;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

#[cfg(feature = "execution")]
mod artifact;
mod check;
mod fix;
mod fmt;
#[cfg(feature = "execution")]
mod profile;
#[cfg(feature = "execution")]
mod run_cmd;
#[cfg(feature = "execution")]
mod runner;

pub fn run() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(command) = args.get(1).map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };

    match command {
        "--help" | "-h" => {
            print_help();
            ExitCode::SUCCESS
        }
        #[cfg(feature = "execution")]
        "build" => artifact::run_build(&args[2..]),
        #[cfg(feature = "execution")]
        "diff" => artifact::run_diff(&args[2..]),
        "check" => check::run_check(&args[2..]),
        "fix" => fix::run_fix(&args[2..]),
        "fmt" => fmt::run_fmt(&args[2..]),
        #[cfg(feature = "execution")]
        "inspect" => artifact::run_inspect(&args[2..]),
        #[cfg(feature = "execution")]
        "profile" => profile::run_profile(&args[2..]),
        #[cfg(feature = "execution")]
        "verify" => artifact::run_verify(&args[2..]),
        #[cfg(feature = "execution")]
        "run" => run_cmd::run_input(&args[2..]),
        #[cfg(feature = "execution")]
        "__runner-v1" => runner::runner_entrypoint(),
        #[cfg(not(feature = "execution"))]
        "build" | "diff" | "inspect" | "run" | "verify" => {
            eprintln!("`rss {command}` requires the `execution` feature");
            ExitCode::from(2)
        }
        _ => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

pub(crate) fn parse_path_args(args: &[String]) -> Result<(bool, Option<&str>), String> {
    let mut json = false;
    let mut path = None;

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument `{arg}`."));
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            return Err(format!("unexpected extra path `{arg}`."));
        }
    }

    Ok((json, path))
}

pub(crate) fn required_flag_value<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, String> {
    let Some(value) = args.get(index) else {
        return Err(format!("missing value for `{flag}`."));
    };
    if value.starts_with("--") {
        return Err(format!("missing value for `{flag}`."));
    }
    Ok(value.as_str())
}

pub(crate) struct InterfaceSource {
    pub(crate) path: String,
    pub(crate) contents: String,
}

pub(crate) const CLI_SOURCE_MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn read_cli_source(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "RSScript source must be a regular non-symlink file: {}",
            path.display()
        ));
    }
    if metadata.len() > CLI_SOURCE_MAX_BYTES {
        return Err(format!(
            "RSScript source exceeds the {} byte CLI limit: {}",
            CLI_SOURCE_MAX_BYTES,
            path.display()
        ));
    }
    let file =
        File::open(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        format!(
            "RSScript source is too large for this platform: {}",
            path.display()
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::take(file, CLI_SOURCE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if bytes.len() as u64 > CLI_SOURCE_MAX_BYTES {
        return Err(format!(
            "RSScript source exceeds the {} byte CLI limit while reading: {}",
            CLI_SOURCE_MAX_BYTES,
            path.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        format!(
            "RSScript source is not valid UTF-8 at {}: {error}",
            path.display()
        )
    })
}

pub(crate) fn read_interface_sources(paths: &[&str]) -> Result<Vec<InterfaceSource>, String> {
    paths
        .iter()
        .map(|path| {
            read_cli_source(Path::new(path))
                .map(|contents| InterfaceSource {
                    path: (*path).to_string(),
                    contents,
                })
                .map_err(|error| format!("failed to read interface {path}: {error}"))
        })
        .collect()
}

pub(crate) fn is_package_directory(path: &str) -> bool {
    let path = Path::new(path);
    path.is_dir() && path.join("rsspkg.toml").exists()
}

const USAGE: &str = r#"usage:
  rss build [--out <artifact.rssbundle>] [--analysis-out <analysis.json>] <file-or-package-directory>
  rss diff [--json|--markdown] <old-source-package-or-bundle> <new-source-package-or-bundle>
  rss verify <artifact.rssbundle>
  rss check [--json] [--lint] [--core|--no-core] [--interface <file.rssi> ...] <file.rss>
  rss check [--json] <package-directory>
  rss check --explain <code>
  rss fix [--write] [--json] [--interface <file.rssi> ...] <file.rss>  # apply machine-applicable fixes
  rss fmt <file.rss>  # writes formatted source to stdout
  rss inspect <imports|bytecode> [--json] <file-or-artifact-or-package>
  rss inspect <analysis|resources|async|call-graph> [--json] <package-directory>
  rss profile [--json] [profile-name]  # inspect host-selected runner presets
  rss run [--json] [--profile <profile-name>] <file-package-or-bundle> [-- <args>...]  # isolated runner + verified VM
  rss run --trusted-in-process [--native] [--json] <file-package-or-bundle> [-- <args>...]"#;

fn usage() -> String {
    USAGE.to_owned()
}

pub(crate) fn print_usage() {
    eprintln!("{}", usage());
}

fn print_help() {
    println!("{}", usage());
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_path_args_rejects_extra_paths() {
        let values = args(&["one.rss", "two.rss"]);
        let error = super::parse_path_args(&values).expect_err("extra path should fail");

        assert_eq!(error, "unexpected extra path `two.rss`.");
    }

    #[test]
    fn cli_source_read_rejects_input_over_limit_before_allocation() {
        let root = unique_temp_dir("source-limit");
        let source = root.join("large.rss");
        let file = fs::File::create(&source).expect("source fixture should create");
        file.set_len(super::CLI_SOURCE_MAX_BYTES + 1)
            .expect("source fixture should resize");

        let error = super::read_cli_source(&source).expect_err("oversized source must fail");
        assert!(error.contains("CLI limit"), "{error}");
        fs::remove_dir_all(root).expect("temp directory should clean up");
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp directory should create");
        path
    }
}
