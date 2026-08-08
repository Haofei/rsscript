//! `rss fix` applies machine-applicable structured edits to source files.

use std::fs;
use std::process::Command;

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
    assert_eq!(diff["schema"], "rsscript.semantic_diff.v1");
    assert_ne!(diff["old"]["module_digest"], diff["new"]["module_digest"]);
}
