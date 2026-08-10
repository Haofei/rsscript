use base64::{Engine as _, engine::general_purpose::STANDARD};
use rsscript_sdk::{
    ArtifactVerifier, CancellationToken, Compiler, ExecutionRequest, RunLimits, Runtime,
    TerminationReason, format_diagnostics_json,
};
use sha2::{Digest, Sha256};

const BASELINE_SOURCE: &str = r#"
fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 32 {
        total = total + index
        index = index + 1
    }
    return total
}
"#;

const LOOP_SOURCE: &str = r#"
fn main() -> Int {
    let mut value = 0
    while true {
        value = value + 1
    }
    return value
}
"#;

fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

#[test]
fn canonical_compilation_and_diagnostics_are_migration_baselines() {
    let compiler = Compiler;
    let first = compiler
        .compile("migration-baseline.rss", BASELINE_SOURCE)
        .expect("baseline source compiles");
    let second = compiler
        .compile("migration-baseline.rss", BASELINE_SOURCE)
        .expect("baseline source compiles repeatedly");
    let first_bytes = first.bundle_bytes().expect("bundle encoding");
    let second_bytes = second.bundle_bytes().expect("bundle encoding");
    assert_eq!(
        first_bytes, second_bytes,
        "the same immutable input must produce byte-identical bundle bytes"
    );
    assert_eq!(
        sha256(&first_bytes),
        "93a00989c3fa3441511b97cc65d92eae4a96befb8aab0e154d8e59f3a1a2b1a0",
        "an intentional Artifact encoding or lowering change must update this digest"
    );

    let diagnostics = compiler.check(
        "migration-diagnostic.rss",
        "fn main() -> Int { return true }",
    );
    let diagnostics = format_diagnostics_json(&diagnostics);
    assert_eq!(
        sha256(diagnostics.as_bytes()),
        "b21cd30fa2e516596a5141d47cbde0099ec934e06e0f6a291a57f9082b21cb32",
        "an intentional diagnostic code/span change must update this digest"
    );
}

#[test]
fn verified_execution_outcomes_are_migration_baselines() {
    let compiler = Compiler;
    let verified = ArtifactVerifier
        .verify(
            compiler
                .compile("migration-baseline.rss", BASELINE_SOURCE)
                .expect("baseline source compiles"),
        )
        .expect("baseline Artifact verifies");
    let report = Runtime::default()
        .link(&verified)
        .expect("baseline Artifact links")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason, TerminationReason::Completed);
    assert_eq!(report.value, "496");
    assert!(report.failure.is_none());
    assert!(report.usage.steps_consumed > 0);

    let loop_artifact = ArtifactVerifier
        .verify(
            compiler
                .compile("migration-loop.rss", LOOP_SOURCE)
                .expect("loop source compiles"),
        )
        .expect("loop Artifact verifies");
    let runtime = Runtime::default();
    let linked = runtime.link(&loop_artifact).expect("loop Artifact links");
    let budget_report = linked
        .execute(ExecutionRequest::default().limits(RunLimits::bounded().with_step_budget(32)));
    assert_eq!(
        budget_report.termination_reason,
        TerminationReason::StepBudgetExceeded
    );
    assert!(budget_report.failure.is_some());

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled_report = linked.execute(
        ExecutionRequest::default().limits(RunLimits::bounded().with_cancellation(cancellation)),
    );
    assert_eq!(
        cancelled_report.termination_reason,
        TerminationReason::Cancelled
    );
    assert!(cancelled_report.failure.is_some());
}

#[test]
fn checked_in_v1_bundle_remains_read_only_verifiable_and_executable() {
    // This fixture is intentionally decoded from checked-in text rather than
    // regenerated from its companion source. It protects the deployed v1
    // reader as the v2 writer evolves.
    let bundle = STANDARD
        .decode(
            include_str!("../../rsscript-bytecode/fixtures/v1/reference.rssbundle.base64").trim(),
        )
        .expect("checked-in v1 compatibility bundle uses valid base64");
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../rsscript-bytecode/fixtures/v1/reference.report.json"
    ))
    .expect("checked-in v1 expected report is JSON");
    let verified = ArtifactVerifier
        .verify_bytes(&bundle)
        .expect("checked-in v1 bundle remains verifiable");
    let report = Runtime::default()
        .link(&verified)
        .expect("checked-in v1 bundle links without Providers")
        .execute(ExecutionRequest::default());

    assert_eq!(
        report.termination_reason,
        TerminationReason::Completed,
        "v1 compatibility bundle must retain its terminal result"
    );
    assert_eq!(report.value, expected["value"].as_str().unwrap());
    assert_eq!(
        format!("{:?}", report.termination_reason).to_lowercase(),
        expected["termination_reason"].as_str().unwrap()
    );
}
