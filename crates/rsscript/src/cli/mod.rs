use std::env;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rsscript::{
    Diagnostic, format_diagnostics_human, format_diagnostics_json, lower_source_to_rust_package,
    lower_sources_to_rust_package_with_options, prepare_package_for_execution,
};
use sha2::{Digest, Sha256};

mod check;
mod fix;
mod fmt;
mod package;
mod process;
mod run_cmd;

pub fn run() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(command) = args.get(1).map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };

    match command {
        "check" => check::run_check(&args[2..]),
        "fix" => fix::run_fix(&args[2..]),
        "fmt" => fmt::run_fmt(&args[2..]),
        "new" => package::run_new_package(&args[2..]),
        "pkg" => package::run_package(&args[2..]),
        "run" => run_cmd::run_generated_rust(&args[2..]),
        _ => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

pub(crate) fn generated_target_dir_from_env() -> Option<PathBuf> {
    let path = env::var_os("RSSCRIPT_GENERATED_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| ramdisk_root_dir().map(|root| root.join("rsscript-generated-target")))?;
    let _ = fs::create_dir_all(&path);

    Some(path)
}
pub(crate) fn print_diagnostics(json: bool, diagnostics: &[Diagnostic]) {
    if json {
        println!("{}", format_diagnostics_json(diagnostics));
    } else {
        print!("{}", format_diagnostics_human(diagnostics));
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
pub(crate) fn lower_cli_input_to_rust_package(
    path: &str,
    runtime_path: &Path,
    json: bool,
) -> Result<rsscript::GeneratedRustPackage, ExitCode> {
    let runtime_path = runtime_path.display().to_string();
    if is_package_directory(path) {
        let input = package_execution_lowering_input(Path::new(path)).map_err(|error| {
            eprintln!("{error}");
            ExitCode::from(2)
        })?;
        return lower_sources_to_rust_package_with_options(
            &input.sources,
            &input.package.name,
            &runtime_path,
            &input.interfaces,
            &input.native_dependencies,
        )
        .map_err(|diagnostics| {
            print_diagnostics(json, &diagnostics);
            ExitCode::from(1)
        });
    }

    let source = read_cli_source(Path::new(path)).map_err(|error| {
        eprintln!("{error}");
        ExitCode::from(2)
    })?;
    let package_name = generated_package_name(path);
    lower_source_to_rust_package(path, &source, &package_name, &runtime_path).map_err(
        |diagnostics| {
            print_diagnostics(json, &diagnostics);
            ExitCode::from(1)
        },
    )
}

fn package_execution_lowering_input(
    package_dir: &Path,
) -> Result<rsscript::PackageLoweringInput, String> {
    let prepared = prepare_package_for_execution(package_dir)?;
    if !prepared.requires_external_provider() {
        return prepared.into_lowering_input();
    }
    let package = prepared.verify()?;
    Ok(package.lowering_input().clone())
}

pub(crate) fn default_runtime_path() -> Result<PathBuf, String> {
    let current_dir =
        env::current_dir().map_err(|error| format!("failed to read current directory: {error}"))?;
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("RSSCRIPT_RUNTIME_PATH") {
        candidates.push(("RSSCRIPT_RUNTIME_PATH", PathBuf::from(path)));
    }
    candidates.push(("current directory", current_dir.join("crates/runtime")));
    candidates.push((
        "compiled manifest directory",
        manifest_dir.join("../runtime"),
    ));
    select_runtime_path(candidates)
}

pub(crate) fn select_runtime_path(
    candidates: Vec<(&'static str, PathBuf)>,
) -> Result<PathBuf, String> {
    let mut checked = Vec::new();
    for (source, path) in candidates {
        checked.push(format!("{source}: {}", path.display()));
        if path.join("Cargo.toml").is_file() {
            return path
                .canonicalize()
                .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()));
        }
    }
    Err(format!(
        "failed to locate rsscript-runtime crate; checked {}. Set RSSCRIPT_RUNTIME_PATH to the runtime crate directory.",
        checked.join(", ")
    ))
}

/// Resolves the generated package name for `input_path` without performing a
/// full lowering, so the run cache directory can be located before deciding
/// whether re-lowering is needed. Returns `None` for a package directory whose
/// manifest can't be read (the caller then falls back to lowering).
pub(crate) fn cli_input_package_name(input_path: &str) -> Option<String> {
    if is_package_directory(input_path) {
        package_execution_lowering_input(Path::new(input_path))
            .ok()
            .map(|input| input.package.name)
    } else {
        Some(generated_package_name(input_path))
    }
}

pub(crate) fn generated_package_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("rsscript-generated")
        .to_string()
}

/// Computes a content fingerprint of everything that affects the generated Rust
/// package for `input_path`, so an unchanged run can reuse the cached package and
/// skip re-lowering. Covers the raw source(s)/interfaces, the runtime path, the
/// release flag, and a compiler-version marker (so a rebuilt `rss` invalidates
/// stale generated output). Returns `None` if any input can't be read, which
/// forces the cautious full lower+write path.
pub(crate) fn run_input_fingerprint(
    input_path: &str,
    runtime_path: &Path,
    release: bool,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    // The package cache must invalidate on *any* compiler edit, not merely a
    // version bump. build.rs derives this fingerprint from every source file.
    parts.push(format!(
        "rss-compiler:{}",
        env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT")
    ));
    parts.push(format!("runtime:{}", runtime_path.display()));
    parts.push(format!("release:{release}"));

    if is_package_directory(input_path) {
        let input = package_execution_lowering_input(Path::new(input_path)).ok()?;
        parts.push(format!("package:{}", input.package.name));
        // Sources/interfaces already carry their contents; include the native
        // dependency identity (path + features + bindings) since it changes the
        // generated Cargo.toml and lowering.
        let mut sources = input.sources.clone();
        sources.sort();
        for (path, contents) in &sources {
            parts.push(format!("src:{path}\n{contents}"));
        }
        let mut interfaces = input.interfaces.clone();
        interfaces.sort();
        for (path, contents) in &interfaces {
            parts.push(format!("iface:{path}\n{contents}"));
        }
        for dependency in &input.native_dependencies {
            parts.push(format!(
                "native:{}|{}|{}|{}",
                dependency.crate_name,
                dependency.path,
                dependency.cargo_features.join(","),
                dependency
                    .bindings
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
    } else {
        let source = read_cli_source(Path::new(input_path)).ok()?;
        parts.push(format!("file:{input_path}\n{source}"));
    }

    Some(stable_hash_hex(&parts.join("\u{1e}")))
}

/// Reads the fingerprint stored alongside a cached generated package.
pub(crate) fn read_cached_fingerprint(cache_dir: &Path) -> Option<String> {
    fs::read_to_string(cache_dir.join(".rss-cache-hash"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Stores the fingerprint alongside the cached generated package.
pub(crate) fn write_cached_fingerprint(cache_dir: &Path, fingerprint: &str) {
    let _ = fs::create_dir_all(cache_dir);
    let _ = fs::write(cache_dir.join(".rss-cache-hash"), fingerprint);
}

pub(crate) fn run_cache_dir(input_path: &str, package_name: &str) -> PathBuf {
    let key = stable_input_key(input_path);
    run_cache_root_dir().join(format!(
        "{}-{}",
        sanitize_path_component(package_name),
        stable_hash_hex(&key)
    ))
}

fn run_cache_root_dir() -> PathBuf {
    let root = env::var_os("RSSCRIPT_RUN_CACHE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::current_dir()
                .unwrap_or_else(|_| env::temp_dir())
                .join("target")
                .join("rsscript-run-cache")
        });
    let _ = fs::create_dir_all(&root);
    root
}

fn stable_input_key(input_path: &str) -> String {
    let path = Path::new(input_path);
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn stable_hash_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "rsscript-generated".to_string()
    } else {
        sanitized
    }
}

fn ramdisk_root_dir() -> Option<PathBuf> {
    env::var_os("RSSCRIPT_RAMDISK_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn cleanup_temp_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}
pub(crate) fn is_package_directory(path: &str) -> bool {
    let path = Path::new(path);
    path.is_dir() && path.join("rsspkg.toml").exists()
}
pub(crate) fn print_usage() {
    eprintln!("usage:");
    eprintln!(
        "  rss check [--json] [--lint] [--core|--no-core] [--interface <file.rssi> ...] <file.rss>"
    );
    eprintln!("  rss check [--json] <package-directory>");
    eprintln!("  rss check --explain <code>");
    eprintln!(
        "  rss fix [--write] [--json] [--interface <file.rssi> ...] <file.rss>  # apply machine-applicable fixes"
    );
    eprintln!("  rss fmt <file.rss>  # writes formatted source to stdout");
    eprintln!("  rss new <package-name>");
    eprintln!("  rss run [--json] [--vm] <file-or-package-directory> [-- <args>...]");
    eprintln!(
        "  rss run [--json] [--release] [--dry-run] <file-or-package-directory> [--out-dir <directory>] [-- <args>...]"
    );
    eprintln!("  rss pkg [--json] [package-directory]");
    eprintln!("  rss pkg add <dependency|dependency@version|path-to-package>");
    eprintln!("  rss pkg analysis [package-directory]");
    eprintln!("  rss pkg review [--json] [package-directory]");
    eprintln!("  rss pkg diff [--json] <old-package-directory> <new-package-directory>");
    eprintln!("  rss pkg ci [--json] [package-directory]");
    eprintln!("  rss pkg lock [--json] [package-directory]");
    eprintln!("  rss pkg tree [--json] [package-directory]");
    eprintln!("  rss pkg metadata [--verify|--dry-run] [--json] [package-directory]");
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
    fn runtime_path_selection_uses_first_valid_candidate() {
        let root = unique_temp_dir("runtime-path-selection");
        let invalid = root.join("missing");
        let valid = root.join("runtime");
        fs::create_dir_all(&valid).expect("runtime directory should create");
        fs::write(
            valid.join("Cargo.toml"),
            "[package]\nname = \"rsscript-runtime\"\n",
        )
        .expect("runtime manifest should write");

        let selected =
            super::select_runtime_path(vec![("env", invalid), ("manifest", valid.clone())])
                .expect("valid runtime path should be selected");

        assert_eq!(
            selected,
            valid
                .canonicalize()
                .expect("valid path should canonicalize")
        );
        fs::remove_dir_all(root).expect("temp runtime path should clean up");
    }

    #[test]
    fn runtime_path_selection_reports_checked_candidates() {
        let root = unique_temp_dir("runtime-path-missing");
        let missing = root.join("missing");
        let error = super::select_runtime_path(vec![("env", missing.clone())])
            .expect_err("missing runtime should fail");

        assert!(error.contains("RSSCRIPT_RUNTIME_PATH"));
        assert!(error.contains(&missing.display().to_string()));
        fs::remove_dir_all(root).expect("temp runtime path should clean up");
    }

    #[test]
    fn run_cache_dir_is_stable_per_input_path() {
        let first = super::run_cache_dir("examples/one.rss", "demo");
        let second = super::run_cache_dir("examples/one.rss", "demo");
        let other = super::run_cache_dir("examples/two.rss", "demo");

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(
            first
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("demo-"))
        );
    }

    #[test]
    fn run_fingerprint_changes_for_every_run_specific_input() {
        let root = unique_temp_dir("run-fingerprint");
        let source = root.join("main.rss");
        fs::write(&source, "fn main() -> Unit { return Unit }\n").expect("source should write");
        let source = source.to_string_lossy();
        let first = super::run_input_fingerprint(&source, &root.join("runtime-a"), false)
            .expect("fingerprint should compute");
        let release = super::run_input_fingerprint(&source, &root.join("runtime-a"), true)
            .expect("release fingerprint should compute");
        let runtime = super::run_input_fingerprint(&source, &root.join("runtime-b"), false)
            .expect("runtime fingerprint should compute");
        fs::write(
            &*source,
            "fn main() -> Unit { Log.write(message: \"changed\") }\n",
        )
        .expect("source should change");
        let changed = super::run_input_fingerprint(&source, &root.join("runtime-a"), false)
            .expect("changed fingerprint should compute");

        assert_ne!(first, release);
        assert_ne!(first, runtime);
        assert_ne!(first, changed);
        fs::remove_dir_all(root).expect("temp directory should clean up");
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

    #[test]
    fn aot_execution_input_preserves_unlocked_pure_package_compatibility() {
        let root = package_fixture("aot-pure-package", "");

        let input = super::package_execution_lowering_input(&root)
            .expect("pure package should not require external provider verification");
        assert!(input.native_dependencies.is_empty());

        fs::remove_dir_all(root).expect("temp directory should clean up");
    }

    #[test]
    fn aot_execution_input_rejects_unreviewed_native_package() {
        let native = r#"
[native.rust]
enabled = true
path = "native/rust"
crate = "aot_native_fixture"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#;
        let root = package_fixture("aot-native-package", native);
        fs::create_dir_all(root.join("native/rust/src")).expect("native source directory");
        fs::write(
            root.join("native/rust/Cargo.toml"),
            "[package]\nname = \"aot_native_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("native Cargo manifest");
        fs::write(root.join("native/rust/src/lib.rs"), "pub fn unused() {}\n")
            .expect("native source");

        let root_text = root.to_string_lossy();
        assert!(
            super::cli_input_package_name(&root_text).is_none(),
            "unauthorized native packages must not enter the cached-run fast path"
        );
        assert!(
            super::run_input_fingerprint(&root_text, &root.join("runtime"), false).is_none(),
            "unauthorized native packages must not reuse a generated Cargo package"
        );
        let error = super::package_execution_lowering_input(&root)
            .expect_err("native AOT input without an approved lock must be rejected");
        assert!(error.contains("native build/load denied"), "{error}");
        assert!(error.contains("rsspkg.lock missing"), "{error}");

        fs::remove_dir_all(root).expect("temp directory should clean up");
    }

    fn package_fixture(name: &str, extra_manifest: &str) -> PathBuf {
        let root = unique_temp_dir(name);
        fs::create_dir_all(root.join("src")).expect("package source directory");
        fs::write(
            root.join("rsspkg.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n{extra_manifest}"
            ),
        )
        .expect("package manifest");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("package source");
        root
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
