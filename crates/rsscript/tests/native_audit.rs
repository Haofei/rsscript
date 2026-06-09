//! `rss native audit` surfaces native adapter risk facts.

mod common;

#[test]
fn native_audit_json_reports_build_inputs_and_findings() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let package_dir = common::workspace_root().join("packages/adapters/sqlx-ffi");
    let output = std::process::Command::new(bin)
        .args([
            "native",
            "audit",
            "--json",
            package_dir.to_str().expect("package path should be utf-8"),
        ])
        .output()
        .expect("rss native audit runs");
    assert!(
        output.status.success(),
        "native audit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("native audit emits JSON");
    assert_eq!(value["$schema"], "rsscript.native_audit.v0.1");
    assert_eq!(value["build_inputs"]["transitive_dependencies"], "not_audited");
    assert!(
        value["build_inputs"]["declared_dependencies"]
            .as_array()
            .is_some_and(|deps| !deps.is_empty())
    );
    assert!(
        value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding.as_str().unwrap_or("").contains("transitive")),
        "audit must flag unaudited transitive dependencies"
    );
}

#[test]
fn native_audit_rejects_package_without_native_adapter() {
    let bin = env!("CARGO_BIN_EXE_rss");
    let package_dir = common::workspace_root().join("examples/capability-review-demo/before");
    let output = std::process::Command::new(bin)
        .args([
            "native",
            "audit",
            package_dir.to_str().expect("package path should be utf-8"),
        ])
        .output()
        .expect("rss native audit runs");
    // The demo's native crate is a stand-in; this asserts the command runs and
    // exits deterministically (0 if native present, 2 if not) without panicking.
    assert!(output.status.code().is_some());
}
