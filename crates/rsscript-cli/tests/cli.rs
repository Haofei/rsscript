//! `rss fix` applies machine-applicable structured edits to source files.

use std::fs;
use std::process::Command;

#[cfg(feature = "execution")]
fn run_isolated_fixture(name: &str, source: &str) -> serde_json::Value {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let path = temp.path().join(name);
    fs::write(&path, source).expect("write isolated fixture");
    let output = Command::new(bin)
        .args(["run", "--json", path.to_str().expect("path is utf-8")])
        .output()
        .expect("isolated runner should execute");
    assert!(
        output.status.success(),
        "rss run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("runner emits an execution report")
}

#[cfg(feature = "execution")]
fn stable_runner_projection(report: &serde_json::Value) -> serde_json::Value {
    // Duration and Artifact identity are intentionally host/provenance-specific.
    // This checked-in projection freezes the report semantics the product
    // promises across the isolated parent/child protocol.
    serde_json::json!({
        "schema": report["schema"],
        "outcome": report["outcome"],
        "usage": report["usage"],
        "stdout": report["stdout"],
        "stderr": report["stderr"],
        "provider_call_count": report["provider_call_traces"].as_array().map(Vec::len),
        "diagnostic_count": report["diagnostics"].as_array().map(Vec::len),
    })
}

#[test]
fn top_level_help_succeeds_on_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("--help")
        .output()
        .expect("rss --help should run");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("usage:\n"));
    assert!(output.stderr.is_empty());
}

#[test]
fn generate_commands_emit_the_versioned_json_schemas() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let source = temp.path().join("prefix.rss");
    fs::write(&source, "fn main() -> Unit {\n").expect("fixture should write");
    let path = source.to_str().expect("path is utf-8");

    let status = Command::new(bin)
        .args(["generate", "prefix-status", "--json", path])
        .output()
        .expect("prefix status command should run");
    assert!(status.status.success(), "{:?}", status);
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("prefix status emits JSON");
    assert_eq!(status["schema"], "rsscript.generate.prefix_status.v1");
    assert_eq!(status["status"], "incomplete");
    assert_eq!(status["syntax_complete"], false);
    assert!(status["replace"]["start"].is_u64());
    assert!(status["terminals"].is_array());

    let continuations = Command::new(bin)
        .args([
            "generate",
            "continuations",
            "--json",
            "--no-core",
            "--max-names",
            "1",
            path,
        ])
        .output()
        .expect("continuations command should run");
    assert!(continuations.status.success(), "{:?}", continuations);
    let continuations: serde_json::Value =
        serde_json::from_slice(&continuations.stdout).expect("continuations emit JSON");
    assert_eq!(
        continuations["schema"],
        "rsscript.generate.continuations.v1"
    );
    assert!(continuations["current_terminal_completeness"].is_string());
    assert!(continuations["terminal_completeness"].is_string());
    assert!(continuations["name_completeness"].is_string());
    assert_eq!(continuations["status"], "incomplete");
    assert!(continuations["replace"]["start"].is_u64());
    assert!(continuations["replace"]["end"].is_u64());
    assert!(continuations["identity"]["session_id"].is_u64());
    assert!(continuations["identity"]["revision"].is_u64());
    assert!(continuations["identity"]["interface_revision"].is_u64());
    assert!(continuations["identity"]["source_bytes"].is_u64());
    assert!(
        continuations["names"]
            .as_array()
            .is_some_and(|names| names.len() <= 1)
    );
    assert!(continuations["total_discovered_names"].is_u64());
    assert!(continuations["truncated"].is_boolean());
}

#[test]
fn generate_no_core_changes_completion_and_semantic_validity() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let source = temp.path().join("core-prefix.rss");
    fs::write(&source, "fn main() -> Unit {\n    List.is_empty(").expect("fixture should write");
    let path = source.to_str().expect("path is utf-8");

    let run = |extra: &[&str]| {
        let mut args = vec!["generate", "continuations", "--json"];
        args.extend_from_slice(extra);
        args.push(path);
        let output = Command::new(bin)
            .args(args)
            .output()
            .expect("continuations command should run");
        assert!(output.status.success(), "{:?}", output);
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .expect("continuations emit JSON")
    };

    let with_core = run(&[]);
    assert!(
        with_core["names"]
            .as_array()
            .is_some_and(|names| { names.iter().any(|candidate| candidate["text"] == "list") })
    );

    let without_core = run(&["--no-core"]);
    assert_eq!(without_core["semantic_validity"], "invalid");
    assert_eq!(without_core["core_interfaces"], "without_core");
    assert!(
        !without_core["names"]
            .as_array()
            .is_some_and(|names| { names.iter().any(|candidate| candidate["text"] == "list") })
    );
}

#[test]
fn fix_write_resolves_missing_data_effects_to_a_clean_check() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let dir = temp.path();
    let file = dir.join("fixme.rss");
    // Four missing exclusive effects across three lines (one line has two), so
    // the test also exercises multi-edit-per-line application order. Default
    // `read` arguments intentionally do not produce fixes.
    fs::write(
        &file,
        concat!(
            "fn touch(left: mut List<Int>, right: mut List<Int>) -> Unit {\n",
            "    return Unit\n",
            "}\n",
            "fn main() -> Unit {\n",
            "    let mut left = List<Int>.new()\n",
            "    let mut right = List<Int>.new()\n",
            "    touch(left: left, right: right)\n",
            "    List.push(list: left, value: 1)\n",
            "    List.push(list: right, value: 2)\n",
            "    return Unit\n",
            "}\n",
        ),
    )
    .expect("fixture should write");
    let path = file.to_str().expect("path is utf-8");

    // Preview must not modify the file.
    let before = fs::read_to_string(&file).unwrap();
    let preview = Command::new(bin)
        .args(["fix", path])
        .output()
        .expect("rss fix preview runs");
    assert!(preview.status.success());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        before,
        "preview must not write"
    );

    // `--json` reports the applied edits structurally.
    let json_out = Command::new(bin)
        .args(["fix", "--json", path])
        .output()
        .expect("rss fix --json runs");
    let report: serde_json::Value =
        serde_json::from_slice(&json_out.stdout).expect("fix emits JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(
        report["applied"].as_array().map(Vec::len),
        Some(4),
        "four machine-applicable edits planned: {report}"
    );

    // `--write` applies the edits; a follow-up check must be clean.
    let write = Command::new(bin)
        .args(["fix", "--write", path])
        .output()
        .expect("rss fix --write runs");
    assert!(
        write.status.success(),
        "fix --write failed: {}",
        String::from_utf8_lossy(&write.stderr)
    );
    let fixed = fs::read_to_string(&file).unwrap();
    assert!(fixed.contains("left: mut left"), "fixed source:\n{fixed}");
    assert!(fixed.contains("right: mut right"), "fixed source:\n{fixed}");
    assert!(
        fixed.contains("List.push(list: mut left, value: 1)"),
        "fixed source:\n{fixed}"
    );
    assert!(
        fixed.contains("List.push(list: mut right, value: 2)"),
        "fixed source:\n{fixed}"
    );

    let check = Command::new(bin)
        .args(["check", path])
        .output()
        .expect("rss check runs");
    let check_out = String::from_utf8_lossy(&check.stdout);
    assert!(
        check_out.contains("ok"),
        "post-fix check not clean:\n{check_out}\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[cfg(feature = "execution")]
#[test]
fn run_cli_defaults_to_the_isolated_verified_vm() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir should be creatable");
    let file = temp.path().join("hello.rss");
    fs::write(
        &file,
        concat!(
            "fn main() -> String {\n",
            "    return \"hello VM\"\n",
            "}\n",
        ),
    )
    .expect("fixture should write");

    let output = Command::new(bin)
        .args(["run", file.to_str().expect("path is utf-8")])
        .output()
        .expect("rss run should execute through the isolated VM runner");

    assert!(
        output.status.success(),
        "rss run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello VM\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[cfg(feature = "execution")]
#[test]
fn structured_async_example_has_a_stable_isolated_runner_report() {
    let report = run_isolated_fixture(
        "structured-async-runner.rss",
        include_str!("../../../examples/structured-async-pipeline/script/isolated.rss"),
    );
    let projection = stable_runner_projection(&report);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/structured-async-pipeline/fixtures/isolated-runner.report.json"
    ))
    .expect("checked-in runner report projection is valid JSON");
    assert_eq!(projection, expected);
}

#[cfg(feature = "execution")]
#[test]
fn embedded_report_example_has_a_stable_isolated_runner_report() {
    let report = run_isolated_fixture(
        "embedded-report-runner.rss",
        include_str!("../../../examples/embedded-report-pipeline/script/isolated.rss"),
    );
    let projection = stable_runner_projection(&report);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/embedded-report-pipeline/fixtures/isolated-runner.report.json"
    ))
    .expect("checked-in runner report projection is valid JSON");
    assert_eq!(projection, expected);
}

#[cfg(feature = "execution")]
#[test]
fn artifact_bundle_verify_run_and_semantic_diff_form_one_cli_workflow() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let temp = tempfile::tempdir().expect("temp dir");
    let old = temp.path().join("old.rss");
    let new = temp.path().join("new.rss");
    let bundle = temp.path().join("old.rssbundle");
    fs::write(&old, "fn main() -> Int { return 1 }\n").unwrap();
    fs::write(&new, "fn main() -> Int { return 2 }\n").unwrap();

    let build = Command::new(bin)
        .args([
            "build",
            "--out",
            bundle.to_str().unwrap(),
            old.to_str().unwrap(),
        ])
        .output()
        .expect("build bundle");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        Command::new(bin)
            .args(["verify", bundle.to_str().unwrap()])
            .status()
            .expect("verify bundle")
            .success()
    );

    let run = Command::new(bin)
        .args(["run", bundle.to_str().unwrap()])
        .output()
        .expect("run bundle");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n");

    let diff = Command::new(bin)
        .args([
            "diff",
            "--json",
            old.to_str().unwrap(),
            new.to_str().unwrap(),
        ])
        .output()
        .expect("semantic diff");
    assert!(
        diff.status.success(),
        "{}",
        String::from_utf8_lossy(&diff.stderr)
    );
    let diff: serde_json::Value = serde_json::from_slice(&diff.stdout).expect("diff JSON");
    assert_eq!(diff["schema"], "rsscript.semantic_diff.v2");
    assert_ne!(diff["old"]["module_digest"], diff["new"]["module_digest"]);
}
