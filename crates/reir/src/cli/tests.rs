use crate::cli::bundle_ops::*;
use crate::cli::commands::*;
use crate::cli::rendering::*;
use crate::cli::safe_io::*;
use crate::cli::{CliError, USAGE};
use reir::{
    AcquisitionMode, CapabilityCategory, Confidence, ConfidenceLevel, Edge, EdgeKind, EvidenceKind,
    FactKind, FactRole, FactValue, Precision, Profile, ProfileBudget, Subject, SubjectKind,
};
use reir::{
    Bundle, Capability, Diff, DiffItem, DiffItemKind, Evidence, Fact, ReconciliationKind, Slice,
    SliceKind,
};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

fn subject(id: &str) -> Subject {
    Subject {
        kind: SubjectKind::Package,
        id: id.to_owned(),
        name: Some(id.to_owned()),
        package: None,
    }
}

fn confidence() -> Confidence {
    Confidence {
        level: ConfidenceLevel::Computed,
        source: Some("test".to_owned()),
    }
}

fn package_risk_fact(id: &str, subject: Subject) -> Fact {
    Fact {
        schema: "reir.fact.v0.1".to_owned(),
        id: id.to_owned(),
        kind: FactKind::PackageRisk,
        role: Some(FactRole::Observed),
        subject,
        capability: None,
        value: FactValue::True,
        confidence: confidence(),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Exact,
        evidence: Vec::new(),
        unknown_reason: None,
    }
}

fn capability_fact(id: &str, role: FactRole, subject: Subject, action: &str) -> Fact {
    capability_fact_with_category(
        id,
        role,
        subject,
        CapabilityCategory::ObjectStorageWrite,
        action,
    )
}

fn capability_fact_with_category(
    id: &str,
    role: FactRole,
    subject: Subject,
    category: CapabilityCategory,
    action: &str,
) -> Fact {
    let principal =
        matches!(role, FactRole::Granted | FactRole::Denied).then(|| subject.id.clone());
    Fact {
        schema: "reir.fact.v0.1".to_owned(),
        id: id.to_owned(),
        kind: FactKind::Capability,
        role: Some(role),
        subject,
        capability: Some(Capability {
            category,
            provider: Some("aws".to_owned()),
            service: Some("s3".to_owned()),
            action: Some(action.to_owned()),
            resource: Some("arn:aws:s3:::reports-prod/*".to_owned()),
            constraints: std::collections::HashMap::new(),
        }),
        value: FactValue::True,
        confidence: confidence(),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::ResourceScoped,
        evidence: vec![Evidence {
            kind: EvidenceKind::PackageMetadata,
            file: Some("test-fixture".to_owned()),
            line: None,
            column: None,
            length: None,
            symbol: None,
            reason: Some("test capability evidence".to_owned()),
            json_pointer: None,
            resource: None,
            provider: None,
            value: None,
            event_id: None,
            time: None,
            source: Some("test".to_owned()),
            event_name: None,
            principal,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: Some(action.to_owned()),
        }],
        unknown_reason: None,
    }
}

fn edge(id: &str, from: Subject, to: Subject) -> Edge {
    Edge {
        schema: "reir.edge.v0.1".to_owned(),
        id: id.to_owned(),
        kind: EdgeKind::DependsOn,
        from,
        to,
        confidence: confidence(),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Exact,
        evidence: Vec::new(),
    }
}

fn unique_temp_dir(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    path.push(format!("reir-{name}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

#[test]
fn take_value_rejects_next_flag_as_missing_value() {
    let args = vec!["--out".to_string(), "--json".to_string()];
    let mut index = 0;

    let error = take_value(&args, &mut index, "--out").expect_err("flag value should fail");

    assert!(matches!(error, CliError::Usage(message) if message == "missing value for --out"));
}

#[test]
fn merge_bundle_values_dedupes_and_rebuilds_subject_index_and_slices() {
    let package = subject("pkg-a@0.1.0");
    let dependency = subject("pkg-b@0.1.0");
    let risk = package_risk_fact("fact.package_risk.pkg-a", package.clone());
    let dependency_edge = edge(
        "edge.depends.pkg-a.pkg-b",
        package.clone(),
        dependency.clone(),
    );

    let mut first = Bundle::new();
    first.producers.push(reir::subject::Producer {
        name: "rsscript".to_owned(),
        version: "0.1.0".to_owned(),
        adapter: Some("rsscript".to_owned()),
        adapter_version: None,
        source: None,
    });
    first.subjects.push(package.clone());
    first.facts.push(risk.clone());
    first.profiles.push(Profile {
        kind: "prod".to_owned(),
        allow: HashMap::new(),
        budget: Some(ProfileBudget {
            max_missing_capabilities: 0,
            max_unknown_coverage: 0,
            max_excess_grants: 1,
        }),
    });
    first.slices.push(Slice {
        schema: "reir.slice.v0.1".to_owned(),
        id: "slice.package_risk".to_owned(),
        kind: SliceKind::PackageRiskSlice,
        facts: vec!["stale.fact".to_owned()],
        edges: Vec::new(),
        reconciliations: Vec::new(),
        evidence: Vec::new(),
    });

    let mut second = Bundle::new();
    second.producers = first.producers.clone();
    second.subjects.push(package.clone());
    second.facts.push(risk);
    second.profiles = first.profiles.clone();
    second.edges.push(dependency_edge);

    let merged = merge_bundle_values(vec![first, second]).expect("merge should succeed");

    assert_eq!(merged.producers.len(), 1);
    assert_eq!(merged.facts.len(), 1);
    assert_eq!(merged.edges.len(), 1);
    assert_eq!(merged.profiles.len(), 1);
    assert_eq!(merged.profiles[0].kind, "prod");
    assert!(
        merged
            .subjects
            .iter()
            .any(|subject| subject.id == package.id)
    );
    assert!(
        merged
            .subjects
            .iter()
            .any(|subject| subject.id == dependency.id)
    );
    assert_eq!(merged.subjects.len(), 2);

    let package_slice = merged
        .slices
        .iter()
        .find(|slice| slice.kind == SliceKind::PackageRiskSlice)
        .expect("package risk slice should be recomputed");
    assert_eq!(
        package_slice.facts,
        vec!["fact.package_risk.pkg-a".to_owned()]
    );
    assert!(!package_slice.facts.contains(&"stale.fact".to_owned()));
}

#[test]
fn collect_rsscript_bundle_merges_package_manager_inputs() {
    let package_review = r#"{
            "package": { "name": "demo", "version": "0.1.0" },
            "risk": "low",
            "summary": {
                "public_apis": 1,
                "mutating_apis": 0,
                "retaining_apis": 0,
                "resource_apis": 0,
                "native_apis": 0,
                "unsafe_apis": 0,
                "unknown_apis": 0
            },
            "exports": [
                { "name": "Api.run", "kind": "function", "classification": "low_semantic_risk" }
            ]
        }"#;
    let package_check = r#"{
            "package": { "name": "demo", "version": "0.1.0", "edition": "2026" },
            "package_dir": "/tmp/demo",
            "ok": false,
            "risk": "elevated",
            "reasons": ["rsspkg.lock missing"],
            "summary": { "diagnostics": 0, "errors": 0, "dependencies": 0 },
            "graph": { "ok": true, "risk": "low", "reasons": [] },
            "lock": {
                "path": "/tmp/demo/rsspkg.lock",
                "present": false,
                "matches": false,
                "risk": "elevated",
                "reasons": ["rsspkg.lock missing"],
                "package_changes": []
            },
            "diagnostics": []
        }"#;
    let package_lock = r#"{
            "version": 1,
            "package": [
                {
                    "name": "demo",
                    "version": "0.1.0",
                    "source": "path+/tmp/demo",
                    "checksum": "sha256:pkg",
                    "interface_hash": "sha256:interface",
                    "review_hash": "sha256:review",
                    "features": []
                }
            ]
        }"#;

    let bundle = collect_rsscript_bundle(RsscriptCollectInputs {
        review_map_json: None,
        package_review_json: Some(package_review),
        package_check_json: Some(package_check),
        package_lock_json: Some(package_lock),
        package_lock_path: None,
        lock_update_json: None,
        package_tree_json: None,
        package_publish_json: None,
        package_metadata_json: None,
        package_vendor_json: None,
        package_name: None,
    })
    .expect("RSScript package-manager JSON should collect");

    assert_eq!(bundle.producers.len(), 3);
    assert!(bundle.facts.iter().any(|fact| {
        fact.id == "fact.package.demo_0_1_0.risk" && fact.kind == FactKind::PackageRisk
    }));
    assert!(bundle.facts.iter().any(|fact| {
        fact.id == "fact.package_check.demo_0_1_0.status" && fact.kind == FactKind::PolicyResult
    }));
    assert!(bundle.facts.iter().any(|fact| {
        fact.id == "fact.lockfile.demo_0_1_0.effective_interface_hash"
            && fact.kind == FactKind::SupplyChain
    }));
    assert!(bundle.slices.iter().any(|slice| {
        slice.kind == SliceKind::PackageRiskSlice
            && slice
                .facts
                .contains(&"fact.lockfile.demo_0_1_0.effective_interface_hash".to_owned())
    }));
}

#[test]
fn collect_rsscript_cli_reads_package_manager_inputs_and_writes_bundle() {
    let temp_dir = unique_temp_dir("collect-rsscript-cli");
    let review_path = temp_dir.join("package-review.json");
    let check_path = temp_dir.join("package-check.json");
    let lock_path = temp_dir.join("rsspkg.lock.json");
    let out_path = temp_dir.join("bundle.json");
    std::fs::write(
        &review_path,
        r#"{
                "package": { "name": "demo", "version": "0.1.0" },
                "risk": "low",
                "summary": {
                    "public_apis": 1,
                    "mutating_apis": 0,
                    "retaining_apis": 0,
                    "resource_apis": 0,
                    "native_apis": 0,
                    "unsafe_apis": 0,
                    "unknown_apis": 0
                },
                "exports": [
                    {
                        "name": "Api.run",
                        "kind": "function",
                        "classification": "low_semantic_risk"
                    }
                ]
            }"#,
    )
    .expect("package review fixture should be written");
    std::fs::write(
        &check_path,
        r#"{
                "package": { "name": "demo", "version": "0.1.0", "edition": "2026" },
                "package_dir": "/tmp/demo",
                "ok": false,
                "risk": "elevated",
                "reasons": ["rsspkg.lock missing"],
                "summary": { "diagnostics": 0, "errors": 0, "dependencies": 0 },
                "graph": { "ok": true, "risk": "low", "reasons": [] },
                "lock": {
                    "path": "/tmp/demo/rsspkg.lock",
                    "present": false,
                    "matches": false,
                    "risk": "elevated",
                    "reasons": ["rsspkg.lock missing"],
                    "package_changes": []
                },
                "diagnostics": []
            }"#,
    )
    .expect("package check fixture should be written");
    std::fs::write(
        &lock_path,
        r#"{
                "version": 1,
                "package": [
                    {
                        "name": "demo",
                        "version": "0.1.0",
                        "source": "path+/tmp/demo",
                        "checksum": "sha256:pkg",
                        "interface_hash": "sha256:interface",
                        "review_hash": "sha256:review",
                        "features": []
                    }
                ]
            }"#,
    )
    .expect("package lock fixture should be written");

    let args = vec![
        "--producer".to_owned(),
        "rsscript".to_owned(),
        "--package-review".to_owned(),
        review_path.to_string_lossy().into_owned(),
        "--package-check".to_owned(),
        check_path.to_string_lossy().into_owned(),
        "--package-lock".to_owned(),
        lock_path.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out_path.to_string_lossy().into_owned(),
        "--json".to_owned(),
    ];

    let code = try_run_collect(&args).expect("collect command should succeed");
    assert_eq!(code, ExitCode::SUCCESS);

    let bundle_json =
        std::fs::read_to_string(&out_path).expect("collect command should write bundle");
    let bundle = Bundle::from_json(&bundle_json).expect("written bundle should parse");
    assert_eq!(bundle.producers.len(), 3);
    assert!(bundle.facts.iter().any(|fact| {
        fact.id == "fact.package.demo_0_1_0.risk" && fact.kind == FactKind::PackageRisk
    }));
    assert!(bundle.facts.iter().any(|fact| {
        fact.id == "fact.package_check.demo_0_1_0.status" && fact.kind == FactKind::PolicyResult
    }));
    assert!(bundle.facts.iter().any(|fact| {
        fact.id == "fact.lockfile.demo_0_1_0.effective_interface_hash"
            && fact.kind == FactKind::SupplyChain
            && fact.evidence[0].file.as_deref() == Some(lock_path.to_string_lossy().as_ref())
    }));
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn report_pr_cli_reads_bundles_and_outputs_all_missing_capabilities() {
    let temp_dir = unique_temp_dir("report-pr");
    let required_path = temp_dir.join("required.json");
    let granted_path = temp_dir.join("granted.json");
    let ci_json_path = temp_dir.join("decision.json");
    let sarif_path = temp_dir.join("decision.sarif");
    let function = Subject {
        kind: SubjectKind::CodeFunction,
        id: "reports::Reports.cleanup_old_reports".to_owned(),
        name: Some("Reports.cleanup_old_reports".to_owned()),
        package: Some("reports".to_owned()),
    };
    let role = Subject {
        kind: SubjectKind::CloudRole,
        id: "arn:aws:iam::123456789012:role/report-uploader".to_owned(),
        name: Some("report-uploader".to_owned()),
        package: Some("reports".to_owned()),
    };
    let required = Bundle {
        facts: vec![
            capability_fact_with_category(
                "fact.reports.cleanup.requires.s3_put_object",
                FactRole::Required,
                function.clone(),
                CapabilityCategory::ObjectStorageWrite,
                "s3:PutObject",
            ),
            capability_fact_with_category(
                "fact.reports.cleanup.requires.s3_delete_object",
                FactRole::Required,
                function,
                CapabilityCategory::ObjectStorageDelete,
                "s3:DeleteObject",
            ),
        ],
        ..Bundle::new()
    };
    let granted = Bundle {
        facts: vec![capability_fact_with_category(
            "fact.reports.role.grants.s3_get_object",
            FactRole::Granted,
            role,
            CapabilityCategory::ObjectStorageRead,
            "s3:GetObject",
        )],
        ..Bundle::new()
    };
    std::fs::write(
        &required_path,
        required
            .to_json()
            .expect("required bundle should serialize"),
    )
    .expect("required bundle should be written");
    std::fs::write(
        &granted_path,
        granted.to_json().expect("granted bundle should serialize"),
    )
    .expect("granted bundle should be written");

    let args = vec![
        "--required".to_owned(),
        required_path.to_string_lossy().into_owned(),
        "--granted".to_owned(),
        granted_path.to_string_lossy().into_owned(),
        "--target".to_owned(),
        "prod".to_owned(),
        "--principal".to_owned(),
        "arn:aws:iam::123456789012:role/report-uploader".to_owned(),
        "--ci-json-out".to_owned(),
        ci_json_path.to_string_lossy().into_owned(),
        "--sarif-out".to_owned(),
        sarif_path.to_string_lossy().into_owned(),
    ];
    let (code, comment) = try_run_report_pr(&args).expect("report-pr should run");
    let ci_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ci_json_path).expect("CI JSON should be written"))
            .expect("CI output should be JSON");
    let sarif: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sarif_path).expect("SARIF should be written"))
            .expect("SARIF output should be JSON");
    let _ = std::fs::remove_dir_all(&temp_dir);

    assert_eq!(code, ExitCode::from(1));
    assert_eq!(ci_json["status"], "fail");
    assert_eq!(sarif["version"], "2.1.0");
    assert!(comment.contains("Status: FAIL"));
    assert!(comment.contains("### Required capabilities needing deployment grant"));
    assert!(comment.contains("object_storage.write aws/s3 s3:PutObject"));
    assert!(comment.contains("object_storage.delete aws/s3 s3:DeleteObject"));
    assert!(comment.contains("s3:PutObject on arn:aws:s3:::reports-prod/*"));
    assert!(comment.contains("s3:DeleteObject on arn:aws:s3:::reports-prod/*"));
}

#[test]
fn collect_cli_rejects_planned_non_rsscript_producers() {
    let args = vec![
        "--producer".to_owned(),
        "k8s".to_owned(),
        "--from".to_owned(),
        "rendered/prod".to_owned(),
    ];

    let error = try_run_collect(&args).expect_err("planned producer should fail");

    match error {
        CliError::Usage(message) => assert_eq!(
            message,
            "`reir collect --producer k8s` is planned; this build supports `rsscript` and `terraform`."
        ),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn collect_terraform_cli_reads_iam_policy_and_writes_granted_bundle() {
    let temp_dir = unique_temp_dir("collect-terraform-cli");
    let out_path = temp_dir.join("terraform.reir.json");
    std::fs::write(
        temp_dir.join("main.tf"),
        r#"resource "aws_iam_role_policy" "report_uploader" {
  role = "report-uploader"
  policy = <<POLICY
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::reports-prod/*"
    }
  ]
}
POLICY
}
"#,
    )
    .expect("Terraform fixture should be written");

    let args = vec![
        "--producer".to_owned(),
        "terraform".to_owned(),
        "--from".to_owned(),
        temp_dir.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out_path.to_string_lossy().into_owned(),
        "--json".to_owned(),
    ];

    let code = try_run_collect(&args).expect("Terraform collect should succeed");
    assert_eq!(code, ExitCode::SUCCESS);

    let bundle_json =
        std::fs::read_to_string(&out_path).expect("collect command should write bundle");
    let bundle = Bundle::from_json(&bundle_json).expect("written bundle should parse");
    let _ = std::fs::remove_dir_all(&temp_dir);

    assert!(bundle.facts.iter().any(|fact| {
        fact.role == Some(FactRole::Granted)
            && fact.value == FactValue::Unknown
            && fact.acquisition_mode == AcquisitionMode::SourceScan
            && fact
                .evidence
                .iter()
                .all(|evidence| evidence.kind == EvidenceKind::SourceTemplatePointer)
            && fact.capability.as_ref().is_some_and(|capability| {
                capability.action.as_deref() == Some("s3:PutObject")
                    && capability.resource.as_deref() == Some("arn:aws:s3:::reports-prod/*")
            })
    }));
}

#[test]
fn collect_cli_rejects_from_for_rsscript_producer() {
    let args = vec![
        "--producer".to_owned(),
        "rsscript".to_owned(),
        "--from".to_owned(),
        "review".to_owned(),
    ];

    let error = try_run_collect(&args).expect_err("--from should not be a RSScript input");

    match error {
        CliError::Usage(message) => assert_eq!(
            message,
            "`--from` is only supported by Terraform/OpenTofu collection; use RSScript JSON input flags with `--producer rsscript`."
        ),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn usage_documents_reconcile_target_for_both_modes() {
    assert!(USAGE.contains(
        "reir reconcile --required required.json --granted granted.json [--target name] [--json]"
    ));
    assert!(USAGE.contains(
        "reir reconcile [--bundle bundle.json] [--target name] [--out reconciled.json] [--json]"
    ));
}

#[test]
fn usage_documents_generic_slice_kind_filter() {
    assert!(USAGE.contains("reir slice --bundle bundle.json [--kind <slice-kind>] [--json]"));
    assert!(matches!(
        parse_slice_kind("package_risk"),
        Ok(SliceKind::PackageRiskSlice)
    ));
    assert!(matches!(
        parse_slice_kind("native_unsafe_slice"),
        Ok(SliceKind::NativeUnsafeSlice)
    ));
}

#[test]
fn merge_bundle_values_rejects_mismatched_ontology() {
    let first = Bundle::new();
    let mut second = Bundle::new();
    second.ontology = "reir.other_ontology.v0".to_owned();

    let error =
        merge_bundle_values(vec![first, second]).expect_err("ontology mismatch should fail");

    assert!(
        matches!(error, CliError::Runtime(message) if message.contains("cannot merge bundle with ontology"))
    );
}

#[test]
fn reconcile_bundle_records_reconciliations_and_rebuilds_slices() {
    let required = capability_fact(
        "fact.required.report_upload",
        FactRole::Required,
        subject("report-uploader"),
        "s3:PutObject",
    );
    let unrelated_grant = capability_fact(
        "fact.granted.report_read",
        FactRole::Granted,
        subject("report-reader"),
        "s3:GetObject",
    );
    let mut bundle = Bundle {
        facts: vec![required.clone(), unrelated_grant],
        ..Bundle::new()
    };

    reconcile_bundle(&mut bundle, Some("prod"));

    assert!(bundle.reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == ReconciliationKind::MissingCapability
            && reconciliation.required_fact.as_deref() == Some(required.id.as_str())
            && reconciliation.target.as_deref() == Some("prod")
    }));
    assert!(bundle.slices.iter().any(|slice| {
        slice.kind == SliceKind::MissingCapabilitySlice && slice.facts.contains(&required.id)
    }));
}

#[test]
fn exit_for_diff_fails_only_when_requested() {
    let unchanged = Diff {
        schema: "reir.diff.v0.1".to_owned(),
        id: "diff.empty".to_owned(),
        items: Vec::new(),
    };
    let changed = Diff {
        schema: "reir.diff.v0.1".to_owned(),
        id: "diff.changed".to_owned(),
        items: vec![DiffItem {
            kind: DiffItemKind::FactAdded,
            id: "fact.added".to_owned(),
            subject: None,
            description: None,
            evidence: Vec::new(),
        }],
    };

    assert_eq!(exit_for_diff(&unchanged, true), ExitCode::SUCCESS);
    assert_eq!(exit_for_diff(&changed, false), ExitCode::SUCCESS);
    assert_eq!(exit_for_diff(&changed, true), ExitCode::from(1));
}

#[test]
fn cli_text_reads_enforce_the_byte_limit() {
    let temp_dir = unique_temp_dir("bounded-read");
    let input = temp_dir.join("input.json");
    std::fs::write(&input, b"12345").expect("fixture should be written");

    let error = read_bounded_text_with_limit(&input, 4).expect_err("oversized input should fail");

    let _ = std::fs::remove_dir_all(&temp_dir);
    assert!(
        matches!(error, CliError::Runtime(message) if message.contains("exceeding the 4 byte limit"))
    );
}

#[test]
fn aggregate_input_budget_applies_across_multiple_files() {
    let temp_dir = unique_temp_dir("aggregate-input-budget");
    let first = temp_dir.join("first.json");
    let second = temp_dir.join("second.json");
    std::fs::write(&first, b"123").expect("first fixture");
    std::fs::write(&second, b"456").expect("second fixture");
    let mut aggregate_bytes = 0;

    read_bounded_text_accounted(first.to_str().expect("UTF-8 path"), &mut aggregate_bytes, 5)
        .expect("first input fits");
    let error = read_bounded_text_accounted(
        second.to_str().expect("UTF-8 path"),
        &mut aggregate_bytes,
        5,
    )
    .expect_err("combined inputs exceed the limit");

    let _ = std::fs::remove_dir_all(&temp_dir);
    assert_eq!(aggregate_bytes, 3);
    assert!(
        matches!(error, CliError::Runtime(message) if message.contains("aggregate input exceeds the 5 byte limit"))
    );
}

#[test]
fn bounded_json_rejects_stdout_payloads_over_the_limit() {
    let error = bounded_json(&"12345", 4).expect_err("JSON exceeds the output limit");
    assert!(
        matches!(error, CliError::Runtime(message) if message.contains("failed to serialize JSON"))
    );
    assert_eq!(
        bounded_json(&"ok", 5).expect("quoted string plus newline fits"),
        b"\"ok\"\n"
    );
}

#[test]
fn merge_rejects_too_many_input_files_before_reading() {
    let paths = vec!["does-not-exist.json".to_owned(); MAX_MERGE_INPUT_FILES + 1];
    let error = merge_bundles(&paths).expect_err("input count must be bounded");
    assert!(
        matches!(error, CliError::Runtime(message) if message.contains("at most 1024 input files"))
    );
}

#[test]
fn atomic_output_replaces_regular_file() {
    let temp_dir = unique_temp_dir("atomic-output");
    let output = temp_dir.join("bundle.json");
    std::fs::write(&output, b"old").expect("fixture should be written");

    atomic_write_no_follow(&output, b"new").expect("regular output should be replaced");

    assert_eq!(std::fs::read(&output).expect("output should exist"), b"new");
    assert!(
        std::fs::read_dir(&temp_dir)
            .expect("directory should be readable")
            .all(|entry| !entry
                .expect("entry should be readable")
                .file_name()
                .to_string_lossy()
                .contains("reir-tmp"))
    );
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[cfg(unix)]
#[test]
fn cli_reads_and_outputs_reject_symlinks() {
    use std::os::unix::fs::symlink;

    let temp_dir = unique_temp_dir("no-follow");
    let target = temp_dir.join("target.json");
    let link = temp_dir.join("link.json");
    std::fs::write(&target, b"unchanged").expect("fixture should be written");
    symlink(&target, &link).expect("symlink should be created");

    let read_error =
        read_bounded_text_with_limit(&link, 64).expect_err("symlink input should be rejected");
    let write_error = atomic_write_no_follow(&link, b"replacement")
        .expect_err("symlink output should be rejected");

    assert!(matches!(read_error, CliError::Runtime(message) if message.contains("symlink input")));
    assert!(
        matches!(write_error, CliError::Runtime(message) if message.contains("symlink output"))
    );
    assert_eq!(
        std::fs::read(&target).expect("target should remain readable"),
        b"unchanged"
    );
    let _ = std::fs::remove_dir_all(&temp_dir);
}
