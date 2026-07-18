//! `rss fix` applies machine-applicable structured edits to source files.

mod common;

use std::fs;
use std::process::Command;

#[test]
fn fix_write_resolves_missing_data_effects_to_a_clean_check() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let dir = common::unique_temp_dir("rss-fix");
    fs::create_dir_all(&dir).expect("temp dir should be creatable");
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

    let _ = fs::remove_dir_all(&dir);
}
