mod common;

use std::collections::HashMap;
use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reir::{
    AcquisitionMode, Capability, CapabilityCategory, Confidence, ConfidenceLevel, Evidence,
    EvidenceKind, Fact, FactKind, FactRole, FactValue, Precision, ReconciliationKind, Subject,
    SubjectKind,
};
use rsscript::{
    lower_sources_to_rust_package_with_options, package_lowering_input, review_package_dir,
    write_generated_rust_package,
};
use serde::Deserialize;

const OBJECTS: usize = 6;
const PAYLOAD_BYTES: usize = 256 * 1024;
const SERVER_DELAY_MS: u64 = 200;

#[test]
#[ignore = "release/demo e2e; run from rss/test-runner/manifests/demo-e2e.rsstest.toml"]
fn s3_iam_reir_demo_fails_preflight_then_passes_and_shows_async_io_gain() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let demo_dir = repo.join("demos/s3-iam-reir");
    let temp_dir = common::unique_temp_dir("rsscript-s3-iam-demo-e2e");
    fs::create_dir_all(&temp_dir).expect("e2e temp dir should be created");

    let required_facts = required_facts_for_demo(&demo_dir);

    let missing_reconciliations = reir::reconcile_capabilities_for_target(
        &required_facts,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-missing.json"),
        Some("prod"),
    );
    assert!(missing_reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == ReconciliationKind::MissingCapability
            && reconciliation.target.as_deref() == Some("prod")
            && reconciliation
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
            && reconciliation
                .required_fact
                .as_deref()
                .is_some_and(|fact_id| fact_id.contains("Reports_upload_batch"))
    }));

    let upload_report_requirement = required_facts
        .iter()
        .find(|fact| {
            fact.subject.id == "rss-s3-uploader::function::upload_report"
                && fact
                    .capability
                    .as_ref()
                    .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
        })
        .expect("upload_report should carry propagated S3 PutObject requirement");
    assert!(upload_report_requirement.evidence.iter().any(|evidence| {
        evidence.file.as_deref() == Some("src/upload.rss")
            && evidence.line == Some(8)
            && evidence
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("upload_report -> S3.put_object"))
    }));
    let upload_batch_requirement = required_facts
        .iter()
        .find(|fact| {
            fact.subject.id == "rss-s3-uploader::function::Reports.upload_batch"
                && fact
                    .capability
                    .as_ref()
                    .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
        })
        .expect("Reports.upload_batch should carry propagated S3 PutObject requirement");
    assert!(upload_batch_requirement.evidence.iter().any(|evidence| {
        evidence.reason.as_deref().is_some_and(|reason| {
            reason.contains("Reports.upload_batch -> upload_report -> S3.put_object")
        })
    }));

    let fixed_reconciliations = reir::reconcile_capabilities_for_target(
        &required_facts,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-fixed.json"),
        Some("prod"),
    );
    assert!(
        fixed_reconciliations
            .iter()
            .all(|reconciliation| reconciliation.kind != ReconciliationKind::MissingCapability),
        "fixed IAM grants should cover all required capabilities: {fixed_reconciliations:#?}"
    );

    let excess_reconciliations = reir::reconcile_capabilities_for_target(
        &required_facts,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-excess.json"),
        Some("prod"),
    );
    assert!(excess_reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == ReconciliationKind::ExcessCapability
            && reconciliation
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:DeleteObject"))
    }));

    let addr = available_local_addr();
    let native_manifest = demo_dir.join("native/rust/Cargo.toml");
    cargo_build(&native_manifest, &["--bin", "mock_s3_server"]);
    cargo_build(&native_manifest, &["--bin", "sync_s3_client"]);

    let server_log = temp_dir.join("mock-s3-server.log");
    let _server = MockServer::start(&demo_dir, addr, &server_log);

    let generated_dir = temp_dir.join("generated-rss-s3-uploader");
    generate_rss_package(&demo_dir, repo, &generated_dir);
    cargo_build(&generated_dir.join("Cargo.toml"), &[]);

    let async_bin = binary_path(&generated_dir.join("target/debug"), "rss-s3-uploader");
    let sync_bin = binary_path(&demo_dir.join("native/rust/target/debug"), "sync_s3_client");

    run_client(&async_bin, addr, &[]);
    let async_elapsed = timed_run(&async_bin, addr, &[]);
    let sync_elapsed = timed_run(
        &sync_bin,
        addr,
        &[("RSS_S3_DEMO_OBJECTS", OBJECTS.to_string())],
    );

    let log = fs::read_to_string(&server_log).expect("mock server log should be readable");
    let async_max_in_flight = max_in_flight_for(&log, "reports/summary.json")
        .max(max_in_flight_for(&log, "reports/security.json"));
    let sync_max_in_flight = max_in_flight_for(&log, "reports/sync-");

    assert!(
        async_max_in_flight > 1,
        "async RSS client should overlap requests; log:\n{log}"
    );
    assert_eq!(
        sync_max_in_flight, 1,
        "sync client should upload sequentially; log:\n{log}"
    );
    assert!(
        async_elapsed < sync_elapsed,
        "async client should be faster than sync client after builds are excluded; async={async_elapsed:?}, sync={sync_elapsed:?}\n{log}"
    );

    println!(
        "s3 iam demo e2e: async={}ms sync={}ms async_max_in_flight={} sync_max_in_flight={}",
        async_elapsed.as_millis(),
        sync_elapsed.as_millis(),
        async_max_in_flight,
        sync_max_in_flight
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn s3_iam_reir_demo_preflight_reports_missing_fixed_and_excess() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let demo_dir = repo.join("demos/s3-iam-reir");
    let required_facts = required_facts_for_demo(&demo_dir);

    let missing_reconciliations = reir::reconcile_capabilities_for_target(
        &required_facts,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-missing.json"),
        Some("prod"),
    );
    assert!(missing_reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == ReconciliationKind::MissingCapability
            && reconciliation
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
            && reconciliation.evidence.iter().any(|evidence| {
                evidence.file.as_deref() == Some("src/upload.rss")
                    && evidence.line == Some(8)
                    && evidence
                        .reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains("upload_report -> S3.put_object"))
            })
    }));
    assert!(missing_reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == ReconciliationKind::MissingCapability
            && reconciliation
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
            && reconciliation.evidence.iter().any(|evidence| {
                evidence.reason.as_deref().is_some_and(|reason| {
                    reason.contains("Reports.upload_batch -> upload_report -> S3.put_object")
                })
            })
    }));

    let fixed_reconciliations = reir::reconcile_capabilities_for_target(
        &required_facts,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-fixed.json"),
        Some("prod"),
    );
    assert!(
        fixed_reconciliations
            .iter()
            .all(|reconciliation| reconciliation.kind != ReconciliationKind::MissingCapability),
        "fixed fixture should cover every required capability: {fixed_reconciliations:#?}"
    );

    let excess_reconciliations = reir::reconcile_capabilities_for_target(
        &required_facts,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-excess.json"),
        Some("prod"),
    );
    let excess = excess_reconciliations
        .iter()
        .find(|reconciliation| {
            reconciliation.kind == ReconciliationKind::ExcessCapability
                && reconciliation
                    .capability
                    .as_ref()
                    .is_some_and(|capability| {
                        capability.action.as_deref() == Some("s3:DeleteObject")
                    })
        })
        .expect("excess fixture should warn on unused S3 DeleteObject grant");
    assert!(
        excess
            .evidence
            .iter()
            .any(|evidence| evidence.file.as_deref() == Some("infra/mock-iam-excess.json")),
        "excess capability should point back to the mock IAM fixture: {excess:#?}"
    );

    println!("s3 iam preflight: missing=s3:PutObject fixed=covered excess=s3:DeleteObject");
}

#[test]
fn s3_iam_reir_demo_scenarios_report_code_capability_change() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let demo_dir = repo.join("demos/s3-iam-reir");
    let fixed_dir = demo_dir.join("scenarios/00-fixed");
    let adds_delete_dir = demo_dir.join("scenarios/03-code-adds-delete");

    let fixed_required = required_facts_for_demo(&fixed_dir);
    let adds_delete_required = required_facts_for_demo(&adds_delete_dir);

    assert!(required_actions(&fixed_required).contains("s3:PutObject"));
    assert!(!required_actions(&fixed_required).contains("s3:DeleteObject"));
    assert!(required_actions(&adds_delete_required).contains("s3:PutObject"));
    assert!(required_actions(&adds_delete_required).contains("s3:DeleteObject"));

    let delete_requirement = adds_delete_required
        .iter()
        .find(|fact| {
            fact.capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:DeleteObject"))
        })
        .expect("code-adds-delete scenario should require S3 DeleteObject");
    assert!(delete_requirement.evidence.iter().any(|evidence| {
        evidence.file.as_deref() == Some("src/upload.rss")
            && evidence.reason.as_deref().is_some_and(|reason| {
                reason.contains("Reports.cleanup_old_reports -> S3.delete_object")
            })
    }));

    let fixed_iam_reconciliations = reir::reconcile_capabilities_for_target(
        &adds_delete_required,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-fixed.json"),
        Some("prod"),
    );
    assert!(fixed_iam_reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == ReconciliationKind::MissingCapability
            && reconciliation
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:DeleteObject"))
    }));

    let excess_iam_reconciliations = reir::reconcile_capabilities_for_target(
        &adds_delete_required,
        &deployment_grants_from_fixtures(&demo_dir, "mock-iam-excess.json"),
        Some("prod"),
    );
    assert!(
        excess_iam_reconciliations
            .iter()
            .all(|reconciliation| reconciliation.kind != ReconciliationKind::MissingCapability),
        "excess fixture grants DeleteObject and should cover the code-adds-delete scenario: {excess_iam_reconciliations:#?}"
    );

    let package_diff =
        rsscript::diff_package_dirs(&fixed_dir, &adds_delete_dir).expect("scenario diff succeeds");
    assert!(
        package_diff
            .reasons
            .iter()
            .any(|reason| reason.contains("package version changed"))
            || package_diff
                .interface_changes
                .iter()
                .any(|change| change.file.contains("s3.rssi")),
        "package diff should still expose the PR surface change: {package_diff:#?}"
    );

    println!(
        "s3 iam scenarios: fixed=PutObject code-change-adds=DeleteObject fixed-iam=missing-delete excess-iam=covers-delete"
    );
}

#[test]
fn s3_iam_reir_demo_native_risk_scenario_reports_review_boundary() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let native_risk_dir = repo.join("demos/s3-iam-reir/scenarios/04-native-risk");
    let review = review_package_dir(&native_risk_dir).expect("native-risk review should succeed");

    assert!(
        review
            .reasons
            .iter()
            .any(|reason| reason == "native Rust wrapper enabled")
    );
    assert!(
        review
            .reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
    );
    assert!(
        review
            .reasons
            .iter()
            .any(|reason| reason == "native Rust unsafe policy requires review")
    );

    println!("s3 iam native-risk: native-wrapper build-scripts unsafe-policy require review");
}

fn required_facts_for_demo(demo_dir: &Path) -> Vec<Fact> {
    let review = review_package_dir(demo_dir).expect("demo package review should succeed");
    let bundle: reir::Bundle =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("demo package review REIR should parse");
    bundle
        .facts
        .into_iter()
        .filter(|fact| fact.role == Some(FactRole::Required))
        .collect()
}

fn required_actions(required_facts: &[Fact]) -> std::collections::BTreeSet<&str> {
    required_facts
        .iter()
        .filter_map(|fact| fact.capability.as_ref()?.action.as_deref())
        .collect()
}

#[derive(Deserialize)]
struct RuntimeFixture {
    service: String,
    grants: Vec<GrantFixture>,
}

#[derive(Deserialize)]
struct IamFixture {
    role: String,
    grants: Vec<GrantFixture>,
}

#[derive(Deserialize)]
struct GrantFixture {
    category: String,
    provider: Option<String>,
    service: Option<String>,
    action: Option<String>,
    resource: Option<String>,
}

fn deployment_grants_from_fixtures(demo_dir: &Path, iam_file: &str) -> Vec<Fact> {
    let runtime_path = demo_dir.join("infra/mock-runtime.json");
    let iam_path = demo_dir.join("infra").join(iam_file);
    let runtime: RuntimeFixture = read_json(&runtime_path);
    let iam: IamFixture = read_json(&iam_path);

    let mut facts = runtime
        .grants
        .into_iter()
        .enumerate()
        .map(|(index, grant)| {
            capability_grant(
                &format!("fact.mock_runtime.{index}"),
                SubjectKind::Service,
                &runtime.service,
                "report-uploader",
                capability_from_grant(grant),
                AcquisitionMode::RenderedManifest,
                EvidenceKind::RenderedManifestPointer,
                "infra/mock-runtime.json",
            )
        })
        .collect::<Vec<_>>();
    facts.extend(iam.grants.into_iter().enumerate().map(|(index, grant)| {
        capability_grant(
            &format!("fact.mock_iam.{index}"),
            SubjectKind::CloudRole,
            &iam.role,
            "report-uploader",
            capability_from_grant(grant),
            AcquisitionMode::CloudPolicy,
            EvidenceKind::CloudPolicyPointer,
            &format!("infra/{iam_file}"),
        )
    }));
    facts
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn capability_from_grant(grant: GrantFixture) -> Capability {
    Capability {
        category: CapabilityCategory::try_from(grant.category)
            .expect("capability category conversion should be infallible"),
        provider: grant.provider,
        service: grant.service,
        action: grant.action,
        resource: grant.resource,
        constraints: HashMap::new(),
    }
}

fn capability_grant(
    id: &str,
    subject_kind: SubjectKind,
    subject_id: &str,
    subject_name: &str,
    capability: Capability,
    acquisition_mode: AcquisitionMode,
    evidence_kind: EvidenceKind,
    evidence_file: &str,
) -> Fact {
    let action = capability.action.clone();
    let precision = if capability.resource.is_some() {
        Precision::ResourceScoped
    } else {
        Precision::Category
    };
    Fact {
        schema: "reir.fact.v0.1".to_string(),
        id: id.to_string(),
        kind: FactKind::Capability,
        role: Some(FactRole::Granted),
        subject: Subject {
            kind: subject_kind,
            id: subject_id.to_string(),
            name: Some(subject_name.to_string()),
            package: None,
        },
        capability: Some(capability),
        value: FactValue::True,
        confidence: Confidence {
            level: ConfidenceLevel::Authoritative,
            source: Some("mock_deployment".to_string()),
        },
        acquisition_mode,
        precision,
        evidence: vec![Evidence {
            kind: evidence_kind,
            file: Some(evidence_file.to_string()),
            line: None,
            column: None,
            length: None,
            symbol: None,
            reason: None,
            resource: None,
            provider: None,
            value: None,
            json_pointer: None,
            event_id: None,
            time: None,
            source: Some("s3_iam_reir_demo_e2e".to_string()),
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action,
        }],
        unknown_reason: None,
    }
}

fn generate_rss_package(demo_dir: &Path, repo: &Path, out_dir: &Path) {
    let input = package_lowering_input(demo_dir).expect("demo package should lower");
    let runtime_path = repo.join("runtime").display().to_string();
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        &runtime_path,
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("demo package Rust lowering should succeed");
    write_generated_rust_package(out_dir, &package)
        .expect("generated RSS package should be written");
}

fn cargo_build(manifest: &Path, extra_args: &[&str]) {
    let output = Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(manifest)
        .args(extra_args)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("cargo build should run");
    assert!(
        output.status.success(),
        "cargo build failed for {}:\nstdout:\n{}\nstderr:\n{}",
        manifest.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_client(binary: &Path, addr: SocketAddr, extra_env: &[(&str, String)]) {
    let output = Command::new(binary)
        .env("RSS_S3_DEMO_ENDPOINT", addr.to_string())
        .env("RSS_S3_DEMO_PAYLOAD_BYTES", PAYLOAD_BYTES.to_string())
        .envs(extra_env.iter().map(|(key, value)| (*key, value)))
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", binary.display()));
    assert!(
        output.status.success(),
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        binary.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn timed_run(binary: &Path, addr: SocketAddr, extra_env: &[(&str, String)]) -> Duration {
    let started = Instant::now();
    run_client(binary, addr, extra_env);
    started.elapsed()
}

fn available_local_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral port should bind");
    let addr = listener.local_addr().expect("local addr should be known");
    drop(listener);
    addr
}

fn wait_for_server(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("mock S3 server did not start on {addr}");
}

fn binary_path(target_dir: &Path, name: &str) -> PathBuf {
    target_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn max_in_flight_for(log: &str, path_fragment: &str) -> usize {
    log.lines()
        .filter(|line| line.contains(path_fragment))
        .filter_map(|line| {
            line.split("in_flight=")
                .nth(1)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0)
}

struct MockServer {
    child: Child,
}

impl MockServer {
    fn start(demo_dir: &Path, addr: SocketAddr, log_path: &Path) -> Self {
        let server_bin = binary_path(&demo_dir.join("native/rust/target/debug"), "mock_s3_server");
        let log = File::create(log_path).expect("mock server log should be created");
        let child = Command::new(&server_bin)
            .env("RSS_S3_DEMO_ADDR", addr.to_string())
            .env("RSS_S3_DEMO_SERVER_DELAY_MS", SERVER_DELAY_MS.to_string())
            .stdout(Stdio::from(log))
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start {}: {error}", server_bin.display()));
        wait_for_server(addr);
        Self { child }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
