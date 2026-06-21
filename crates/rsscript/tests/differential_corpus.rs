//! Differential fixture corpus.
//!
//! Each fixture is a `.rss` program plus a `.toml` sidecar declaring what to
//! expect. The whole corpus runs inside the `differential` harness so Cargo
//! exposes one differential test type instead of a separate corpus runner.
//!
//! Two fixture kinds:
//! * `execution` -- run the program and check output. By default it runs on
//!   the register VM; listing `compiled` in `backends` also lowers + builds +
//!   runs it and asserts the two backends agree (differential / parity by
//!   default).
//! * `diagnostics` -- assert the checker reports exactly the listed error codes.
//!
//! Sidecar schema (`<name>.toml`):
//!   kind     = "execution" | "diagnostics"
//!   backends = ["vm", "compiled"]   # execution only; default ["vm"]
//!   args     = ["..."]              # program args
//!   stdout   = "..."                # expected stdout (optional)
//!   value    = "..."                # expected `main` return display (optional)
//!   codes    = ["RS0206"]           # diagnostics: expected error codes
//!   tags     = ["arithmetic"]       # capability tags (feed the coverage gate)
#![allow(clippy::duplicate_mod)]

mod common;

use std::path::{Path, PathBuf};

use serde::Deserialize;

const CORPUS_DIR: &str = "tests/corpus";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Execution,
    Diagnostics,
}

#[derive(Debug, Deserialize)]
struct FixtureSpec {
    kind: Kind,
    #[serde(default)]
    backends: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Capability tags the corpus must always cover, so "test all aspects" is an
/// enforced invariant rather than an aspiration. Adding a feature should add a
/// fixture (and, when it's a new capability, a tag here).
const REQUIRED_TAGS: &[&str] = &[
    "arithmetic",
    "string",
    "list",
    "option",
    "result",
    "match",
    "diagnostics",
];

#[test]
fn corpus_fixtures_pass() {
    let paths = collect_fixture_paths();
    for path in &paths {
        run_fixture(path).unwrap_or_else(|error| panic!("{}: {error}", trial_name(path)));
    }
    coverage_gate(&paths).unwrap_or_else(|error| panic!("coverage::required_tags: {error}"));
}

fn collect_fixture_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_rss(Path::new(CORPUS_DIR), &mut paths);
    paths.sort();
    paths
}

/// Fail if any required capability tag has no fixture covering it.
fn coverage_gate(paths: &[PathBuf]) -> Result<(), String> {
    let mut covered = std::collections::BTreeSet::new();
    for path in paths {
        if let Ok(spec) = load_spec(path) {
            covered.extend(spec.tags);
        }
    }
    let missing: Vec<&str> = REQUIRED_TAGS
        .iter()
        .copied()
        .filter(|tag| !covered.contains(*tag))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "corpus is missing fixtures for capabilities: {missing:?}"
        ));
    }
    Ok(())
}

fn load_spec(path: &Path) -> Result<FixtureSpec, String> {
    let spec_path = path.with_extension("toml");
    let spec_text = std::fs::read_to_string(&spec_path)
        .map_err(|e| format!("missing sidecar {spec_path:?}: {e}"))?;
    toml::from_str(&spec_text).map_err(|e| format!("bad sidecar {spec_path:?}: {e}"))
}

fn collect_rss(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rss(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rss") {
            out.push(path);
        }
    }
}

fn trial_name(path: &Path) -> String {
    path.strip_prefix(CORPUS_DIR)
        .unwrap_or(path)
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "::")
}

fn run_fixture(path: &Path) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    let spec = load_spec(path)?;
    if spec.tags.is_empty() {
        return Err("fixture must declare at least one `tags` entry".into());
    }
    let file = path.file_name().unwrap().to_string_lossy().to_string();
    let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();

    match spec.kind {
        Kind::Execution => run_execution(&file, &source, &args, &spec),
        Kind::Diagnostics => run_diagnostics(&file, &source, &spec),
    }
}

fn run_execution(
    file: &str,
    source: &str,
    args: &[&str],
    spec: &FixtureSpec,
) -> Result<(), String> {
    let vm = common::run_vm_source(file, source, args)
        .map_err(|error| format!("VM evaluation failed: {error:?}"))?;

    if let Some(expected) = &spec.stdout
        && &vm.stdout != expected
    {
        return Err(format!(
            "VM stdout mismatch\n expected: {expected:?}\n   actual: {:?}",
            vm.stdout
        ));
    }
    if let Some(expected) = &spec.value
        && &vm.display_value != expected
    {
        return Err(format!(
            "VM return mismatch\n expected: {expected:?}\n   actual: {:?}",
            vm.display_value
        ));
    }

    let backends = if spec.backends.is_empty() {
        vec!["vm".to_string()]
    } else {
        spec.backends.clone()
    };
    if backends.iter().any(|b| b == "compiled") {
        let (stdout, stderr) = common::run_compiled_source(file, source, args);
        // Parity: the compiled backend must match the VM exactly.
        if stdout != vm.stdout {
            return Err(format!(
                "backend parity: stdout differs\n      vm: {:?}\ncompiled: {:?}",
                vm.stdout, stdout
            ));
        }
        if !stderr.is_empty() {
            return Err(format!("compiled backend wrote to stderr:\n{stderr}"));
        }
    }
    Ok(())
}

fn run_diagnostics(file: &str, source: &str, spec: &FixtureSpec) -> Result<(), String> {
    let mut expected = spec.codes.clone();
    expected.sort();
    expected.dedup();
    let actual = common::error_codes(file, source);
    if actual != expected {
        return Err(format!(
            "diagnostic codes mismatch\n expected: {expected:?}\n   actual: {actual:?}"
        ));
    }
    Ok(())
}
