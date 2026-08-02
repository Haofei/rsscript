use std::fs;
use std::path::Path;
use std::process::Command;

use reir::{Bundle, FactRole};

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
        .and_then(Path::parent)
        .expect("reir crate should live under the workspace root");
    let temp_dir = unique_temp_dir("rsscript-reir-report-pr-cli");
    fs::create_dir_all(&temp_dir).expect("temporary directory should be created");
    let package_review_json = temp_dir.join("package-review.json");
    let head_reir = temp_dir.join("head.reir.json");

    // Package-review generation is covered in the rsscript package tests. Keep
    // this REIR CLI test self-contained so it does not invoke a nested Cargo
    // build with a different feature set during `cargo test --workspace`.
    let package_review = serde_json::json!({
        "package": {
            "name": "rss-s3-uploader",
            "version": "0.2.0"
        },
        "risk": "high",
        "summary": {
            "native_apis": 2
        },
        "exports": [
            {
                "name": "S3.delete_object",
                "kind": "function",
                "classification": "review_if_changed",
                "reasons": ["async boundary", "native boundary", "public function"],
                "function_kind": "async",
                "normalized_effects": ["native", "suspends"]
            },
            {
                "name": "S3.put_object",
                "kind": "function",
                "classification": "review_if_changed",
                "reasons": ["async boundary", "native boundary", "public function"],
                "function_kind": "async",
                "normalized_effects": ["native", "suspends"]
            }
        ],
        "external_bindings": [
            {
                "function": "Reports.cleanup_old_reports",
                "binding_symbol": "S3.delete_object",
                "category": "object_storage.delete",
                "provider": "aws",
                "service": "s3",
                "action": "s3:DeleteObject",
                "resource": "arn:aws:s3:::reports-prod/*",
                "call_chain": ["Reports.cleanup_old_reports", "S3.delete_object"],
                "span": {
                    "file": "src/upload.rss",
                    "line": 27,
                    "column": 11,
                    "length": 2
                }
            },
            {
                "function": "S3.delete_object",
                "binding_symbol": "S3.delete_object",
                "category": "object_storage.delete",
                "provider": "aws",
                "service": "s3",
                "action": "s3:DeleteObject",
                "resource": "arn:aws:s3:::reports-prod/*",
                "call_chain": ["S3.delete_object"],
                "span": {
                    "file": "interface/s3.rssi",
                    "line": 9,
                    "column": 1,
                    "length": 3
                }
            },
            {
                "function": "Reports.upload_batch",
                "binding_symbol": "S3.put_object",
                "category": "object_storage.write",
                "provider": "aws",
                "service": "s3",
                "action": "s3:PutObject",
                "resource": "arn:aws:s3:::reports-prod/*",
                "call_chain": ["Reports.upload_batch", "upload_report", "S3.put_object"],
                "span": {
                    "file": "src/upload.rss",
                    "line": 15,
                    "column": 11,
                    "length": 13
                }
            },
            {
                "function": "S3.put_object",
                "binding_symbol": "S3.put_object",
                "category": "object_storage.write",
                "provider": "aws",
                "service": "s3",
                "action": "s3:PutObject",
                "resource": "arn:aws:s3:::reports-prod/*",
                "call_chain": ["S3.put_object"],
                "span": {
                    "file": "interface/s3.rssi",
                    "line": 2,
                    "column": 1,
                    "length": 3
                }
            },
            {
                "function": "upload_report",
                "binding_symbol": "S3.put_object",
                "category": "object_storage.write",
                "provider": "aws",
                "service": "s3",
                "action": "s3:PutObject",
                "resource": "arn:aws:s3:::reports-prod/*",
                "call_chain": ["upload_report", "S3.put_object"],
                "span": {
                    "file": "src/upload.rss",
                    "line": 8,
                    "column": 11,
                    "length": 2
                }
            }
        ],
        "native_rust": {
            "cargo_features": [],
            "semantic": {
                "author_declaration": {
                    "worker_thread_parallelism": false,
                    "native_parallel_backend": null,
                    "risk_reasons": ["native Rust wrapper path is outside the package root"]
                },
                "source_scan_best_effort": {
                    "tool": "rss-native-source-scan",
                    "selected_graph": "package-native-rust",
                    "worker_thread_parallelism_detected": false,
                    "native_parallel_backends": [],
                    "unsafe_detected": false,
                    "ffi_detected": false,
                    "filesystem_detected": false,
                    "network_detected": true,
                    "build_script_present": false
                }
            }
        },
        "diagnostics": [
            {
                "code": "RS0206",
                "severity": "error",
                "summary": "call to `S3.put_object` does not resolve.",
                "span": {"file": "src/upload.rss", "line": 8, "column": 11, "length": 2}
            },
            {
                "code": "RS0206",
                "severity": "error",
                "summary": "call to `S3.delete_object` does not resolve.",
                "span": {"file": "src/upload.rss", "line": 28, "column": 11, "length": 2}
            },
            {
                "code": "RS0030",
                "severity": "error",
                "summary": "`await` must consume an async call.",
                "span": {"file": "src/upload.rss", "line": 8, "column": 5, "length": 5}
            },
            {
                "code": "RS0030",
                "severity": "error",
                "summary": "`await` must consume an async call.",
                "span": {"file": "src/upload.rss", "line": 28, "column": 5, "length": 5}
            },
            {
                "code": "RS1301",
                "severity": "error",
                "summary": "package interface function `S3.delete_object` has no public source implementation.",
                "span": {"file": "interface/s3.rssi", "line": 10, "column": 1, "length": 3}
            },
            {
                "code": "RS1301",
                "severity": "error",
                "summary": "package interface function `S3.put_object` has no public source implementation.",
                "span": {"file": "interface/s3.rssi", "line": 3, "column": 1, "length": 3}
            }
        ]
    });
    fs::write(
        &package_review_json,
        serde_json::to_vec(&package_review).expect("package review fixture should serialize"),
    )
    .expect("package review JSON should be written");

    let collect = Command::new(env!("CARGO_BIN_EXE_reir"))
        .current_dir(workspace)
        .args([
            "collect",
            "--producer",
            "rsscript",
            "--package-review",
            package_review_json
                .to_str()
                .expect("temporary path should be utf-8"),
            "--out",
            head_reir.to_str().expect("temporary path should be utf-8"),
        ])
        .output()
        .expect("reir collect command should run");
    assert!(
        collect.status.success(),
        "reir collect should convert package-manager JSON to REIR\nstderr:\n{}",
        String::from_utf8_lossy(&collect.stderr)
    );

    let bundle_json = fs::read(&head_reir).expect("head REIR bundle should be readable");
    let bundle: Bundle =
        serde_json::from_slice(&bundle_json).expect("reir collect should emit valid REIR JSON");
    assert!(bundle.facts.iter().any(|fact| {
        fact.role == Some(FactRole::Required)
            && fact
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:DeleteObject"))
    }));

    let report_pr = Command::new(env!("CARGO_BIN_EXE_reir"))
        .current_dir(workspace)
        .args([
            "report-pr",
            "--required",
            head_reir.to_str().expect("temporary path should be utf-8"),
            "--granted",
            "examples/demos/s3-iam-reir/expected/prod-grants.reir.json",
            "--principal",
            "prod/report-uploader",
            "--allow-unknown",
            "--allow-excess",
            "--allow-unverified-capabilities",
        ])
        .output()
        .expect("reir report-pr command should run");

    let expected =
        fs::read_to_string(workspace.join("examples/demos/s3-iam-reir/expected/pr-comment.md"))
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
