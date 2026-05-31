use std::fs;
use std::path::Path;
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos()
    ));
    path
}

#[test]
fn report_pr_cli_matches_s3_demo_golden_comment() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .expect("reir crate should live under the workspace root");
    let temp_dir = unique_temp_dir("rsscript-reir-report-pr-cli");
    fs::create_dir_all(&temp_dir).expect("temporary directory should be created");
    let head_reir = temp_dir.join("head.reir.json");

    let package_review = Command::new("cargo")
        .current_dir(workspace)
        .args([
            "run",
            "--quiet",
            "--bin",
            "rss",
            "--",
            "pkg",
            "review",
            "--reir",
            "demos/s3-iam-reir/scenarios/03-code-adds-delete",
        ])
        .output()
        .expect("rss pkg review command should run");
    assert!(
        !package_review.stdout.is_empty(),
        "rss pkg review should emit the REIR bundle even when package risk makes the review command non-zero\nstderr:\n{}",
        String::from_utf8_lossy(&package_review.stderr)
    );
    fs::write(&head_reir, &package_review.stdout).expect("head REIR bundle should be written");

    let report_pr = Command::new(env!("CARGO_BIN_EXE_reir"))
        .current_dir(workspace)
        .args([
            "report-pr",
            "--required",
            head_reir.to_str().expect("temporary path should be utf-8"),
            "--granted",
            "demos/s3-iam-reir/expected/prod-grants.reir.json",
            "--target",
            "prod",
        ])
        .output()
        .expect("reir report-pr command should run");

    let expected = fs::read_to_string(workspace.join("demos/s3-iam-reir/expected/pr-comment.md"))
        .expect("golden PR comment should be readable");
    let stdout = String::from_utf8(report_pr.stdout).expect("report-pr stdout should be utf-8");
    assert_eq!(stdout, expected);
    assert!(
        !report_pr.status.success(),
        "missing deployment capability should make report-pr exit non-zero"
    );
    assert!(
        report_pr.stderr.is_empty(),
        "report-pr should not write stderr on expected missing-capability result: {}",
        String::from_utf8_lossy(&report_pr.stderr)
    );

    let _ = fs::remove_dir_all(&temp_dir);
}
