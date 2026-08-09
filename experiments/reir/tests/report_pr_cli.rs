use std::fs;
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
fn report_pr_rejects_unbound_and_unknown_targets() {
    let unbound = Command::new(env!("CARGO_BIN_EXE_reir"))
        .args(["report-pr", "--target", "prod"])
        .output()
        .expect("report-pr should run");
    assert!(!unbound.status.success());
    assert!(String::from_utf8_lossy(&unbound.stderr).contains("requires an explicit --principal"));

    let temp_dir = unique_temp_dir("rsscript-reir-target-policy");
    fs::create_dir_all(&temp_dir).expect("temporary directory should be created");
    let policy = temp_dir.join("policy.toml");
    fs::write(&policy, "[target.prod]\nprincipal = \"role.prod\"\n").unwrap();
    let unknown = Command::new(env!("CARGO_BIN_EXE_reir"))
        .args([
            "report-pr",
            "--target",
            "staging",
            "--policy",
            policy.to_str().unwrap(),
        ])
        .output()
        .expect("report-pr should run");
    assert!(!unknown.status.success());
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("unknown gate policy target `staging`")
    );
    let _ = fs::remove_dir_all(&temp_dir);
}
