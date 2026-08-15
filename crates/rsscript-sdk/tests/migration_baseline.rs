use base64::{Engine as _, engine::general_purpose::STANDARD};
use rsscript_sdk::{
    ArtifactBundle, ArtifactVerifier, BytecodeArtifact, BytecodeLimits, BytecodeVerifier,
    CancellationToken, Compiler, ExecutionRequest, RunLimits, Runtime, TerminationReason,
    format_diagnostics_json,
};
use sha2::{Digest, Sha256};

#[derive(Debug, serde::Deserialize)]
struct MalformedBoundaryFixtures {
    case: Vec<MalformedBoundaryFixture>,
}

#[derive(Debug, serde::Deserialize)]
struct MalformedBoundaryFixture {
    name: String,
    scope: String,
    operation: String,
    offset: usize,
    value: u64,
    expected: String,
}

#[derive(Debug, serde::Deserialize)]
struct V2MalformedFixtures {
    case: Vec<V2MalformedFixture>,
}

#[derive(Debug, serde::Deserialize)]
struct V2MalformedFixture {
    name: String,
    operation: String,
    offset: usize,
    value: u64,
    expected: String,
}

#[derive(Debug, serde::Deserialize)]
struct V2ArtifactBoundaryFixtures {
    case: Vec<V2ArtifactBoundaryFixture>,
}

#[derive(Debug, serde::Deserialize)]
struct V2ArtifactBoundaryFixture {
    name: String,
    operation: String,
    offset: usize,
    value: u64,
    expected: String,
}

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
        "f49759f58b8b4afa81bc39a4a5fc2af79de94a9d79bf80e0439cc5744059a1ed",
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
    let admitted = ArtifactVerifier
        .verify(
            compiler
                .compile("migration-baseline.rss", BASELINE_SOURCE)
                .expect("baseline source compiles"),
        )
        .expect("baseline Artifact verifies")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("baseline Artifact links")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert_eq!(report.value(), Some("496"));
    assert!(report.failure().is_none());
    assert!(report.usage.steps_consumed > 0);

    let loop_artifact = ArtifactVerifier
        .verify(
            compiler
                .compile("migration-loop.rss", LOOP_SOURCE)
                .expect("loop source compiles"),
        )
        .expect("loop Artifact verifies")
        .admit_trusted_input();
    let runtime = Runtime::default();
    let linked = runtime.link(&loop_artifact).expect("loop Artifact links");
    let budget_report = linked
        .execute(ExecutionRequest::default().limits(RunLimits::bounded().with_step_budget(32)));
    assert_eq!(
        budget_report.termination_reason(),
        TerminationReason::StepBudgetExceeded
    );
    assert!(budget_report.failure().is_some());

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled_report = linked.execute(
        ExecutionRequest::default().limits(RunLimits::bounded().with_cancellation(cancellation)),
    );
    assert_eq!(
        cancelled_report.termination_reason(),
        TerminationReason::Cancelled
    );
    assert!(cancelled_report.failure().is_some());
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
    let admitted = ArtifactVerifier
        .verify_bytes(&bundle)
        .expect("checked-in v1 bundle remains verifiable")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("checked-in v1 bundle links without Providers")
        .execute(ExecutionRequest::default());
    let serialized = serde_json::to_value(&report).expect("execution report serializes");

    assert_eq!(
        report.termination_reason(),
        TerminationReason::Completed,
        "v1 compatibility bundle must retain its terminal result"
    );
    assert_eq!(report.value(), expected["value"].as_str());
    assert_eq!(
        serialized["termination_reason"].as_str(),
        expected["termination_reason"].as_str(),
        "the v1 report projection retains its structured termination reason"
    );
}

#[test]
fn checked_in_v1_failure_bundle_retains_its_complete_execution_report() {
    // This fixture is deliberately a prebuilt failing program. The reader and
    // VM must preserve script-level termination evidence even though the
    // current compiler is not involved in loading it.
    let bundle = STANDARD
        .decode(
            include_str!(
                "../../rsscript-bytecode/fixtures/v1/step-budget-exhausted.rssbundle.base64"
            )
            .trim(),
        )
        .expect("checked-in v1 failure bundle uses valid base64");
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../rsscript-bytecode/fixtures/v1/step-budget-exhausted.report.json"
    ))
    .expect("checked-in v1 expected failure report is JSON");
    let admitted = ArtifactVerifier
        .verify_bytes(&bundle)
        .expect("checked-in v1 failure bundle remains verifiable")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("checked-in v1 failure bundle links without Providers")
        .execute(ExecutionRequest::default().limits(RunLimits::bounded().with_step_budget(32)));

    assert_eq!(
        report.termination_reason(),
        TerminationReason::StepBudgetExceeded,
        "v1 failure bundle must retain the script-level termination reason"
    );
    let serialized = serde_json::to_value(&report).expect("execution report serializes");
    assert_eq!(
        serialized["value"].as_str(),
        expected["value"].as_str(),
        "the v1 report projection retains its structured failure value"
    );
    assert_eq!(
        serialized["termination_reason"].as_str(),
        expected["termination_reason"].as_str(),
        "the v1 report projection retains its structured termination reason"
    );
    assert!(report.failure().is_some());
}

#[test]
fn checked_in_v1_trailing_byte_fixture_remains_fail_closed() {
    let mut bundle = STANDARD
        .decode(
            include_str!("../../rsscript-bytecode/fixtures/v1/reference.rssbundle.base64").trim(),
        )
        .expect("checked-in v1 compatibility bundle uses valid base64");
    let trailing = u8::from_str_radix(
        include_str!("../../rsscript-bytecode/fixtures/v1/reference.trailing-byte.hex").trim(),
        16,
    )
    .expect("checked-in malformed fixture byte is hexadecimal");
    bundle.push(trailing);
    let error = ArtifactVerifier
        .verify_bytes(&bundle)
        .expect_err("a v1 bundle with a checked-in trailing byte must fail closed");
    assert!(
        error.to_string().contains("trailing"),
        "malformed fixture must fail at the bundle boundary: {error}"
    );
}

#[test]
fn checked_in_v1_malformed_boundary_manifest_remains_fail_closed() {
    // The table is intentionally checked in beside the deployed v1 fixture.
    // It keeps boundary cases reviewable without reconstructing an Artifact
    // from current compiler output, which would weaken old-reader coverage.
    let fixtures: MalformedBoundaryFixtures = toml::from_str(include_str!(
        "../../rsscript-bytecode/fixtures/v1/malformed-boundaries.toml"
    ))
    .expect("malformed v1 fixture manifest is valid TOML");
    let bundle = STANDARD
        .decode(
            include_str!("../../rsscript-bytecode/fixtures/v1/reference.rssbundle.base64").trim(),
        )
        .expect("checked-in v1 compatibility bundle uses valid base64");
    let artifact = ArtifactBundle::from_bytes(&bundle)
        .expect("reference Bundle remains structurally readable")
        .artifact_bytes()
        .to_vec();

    for fixture in fixtures.case {
        let error = match fixture.scope.as_str() {
            "bundle" => {
                let mut bytes = bundle.clone();
                apply_static_boundary_mutation(&mut bytes, &fixture);
                ArtifactVerifier
                    .verify_bytes(&bytes)
                    .expect_err("static malformed Bundle must fail closed")
                    .to_string()
            }
            "bytecode" if fixture.operation == "verify_artifact_limit" => {
                BytecodeVerifier::new(BytecodeLimits {
                    max_artifact_bytes: fixture.value as usize,
                    ..BytecodeLimits::default()
                })
                .verify(&artifact)
                .expect_err("static v1 bytecode must honor a configured Artifact size limit")
                .to_string()
            }
            "bytecode" => {
                let mut bytes = artifact.clone();
                apply_static_boundary_mutation(&mut bytes, &fixture);
                BytecodeArtifact::from_bytes(&bytes)
                    .expect_err("static malformed bytecode must fail closed")
                    .to_string()
            }
            other => panic!("unknown malformed-fixture scope `{other}`"),
        };
        assert!(
            error.contains(&fixture.expected),
            "{} expected error containing {:?}, got {error}",
            fixture.name,
            fixture.expected,
        );
    }
}

fn apply_static_boundary_mutation(bytes: &mut [u8], fixture: &MalformedBoundaryFixture) {
    match fixture.operation.as_str() {
        "set_byte" => bytes[fixture.offset] = fixture.value as u8,
        "xor_byte" => bytes[fixture.offset] ^= fixture.value as u8,
        "set_be_u64" => {
            bytes[fixture.offset..fixture.offset + 8].copy_from_slice(&fixture.value.to_be_bytes());
        }
        other => panic!("unknown malformed-fixture operation `{other}`"),
    }
}

#[test]
fn checked_in_v2_payload_and_malformed_instruction_fixture_remain_fail_closed() {
    use rsscript_bytecode::v2::BytecodeV2Verifier;

    let fixtures: V2MalformedFixtures = toml::from_str(include_str!(
        "../../rsscript-bytecode/fixtures/v2/malformed-boundaries.toml"
    ))
    .expect("malformed v2 fixture manifest is valid TOML");
    let payload = STANDARD
        .decode(include_str!("../../rsscript-bytecode/fixtures/v2/reference.payload.base64").trim())
        .expect("checked-in v2 payload uses valid base64");
    BytecodeV2Verifier::default()
        .verify_payload(&payload)
        .expect("checked-in v2 payload remains structurally verifiable");

    for fixture in fixtures.case {
        let mut malformed = payload.clone();
        match fixture.operation.as_str() {
            "set_byte" => malformed[fixture.offset] = fixture.value as u8,
            other => panic!("unknown v2 malformed-fixture operation `{other}`"),
        }
        let error = BytecodeV2Verifier::default()
            .verify_payload(&malformed)
            .expect_err("static malformed v2 payload must fail closed")
            .to_string();
        assert!(
            error.contains(&fixture.expected),
            "{} expected error containing {:?}, got {error}",
            fixture.name,
            fixture.expected,
        );
    }
}

#[test]
fn checked_in_v2_artifact_boundary_fixture_remains_fail_closed() {
    use rsscript_bytecode::v2::BytecodeV2ArtifactVerifier;

    let fixtures: V2ArtifactBoundaryFixtures = toml::from_str(include_str!(
        "../../rsscript-bytecode/fixtures/v2/artifact-boundaries.toml"
    ))
    .expect("malformed v2 Artifact fixture manifest is valid TOML");
    let artifact = STANDARD
        .decode(
            include_str!("../../rsscript-bytecode/fixtures/v2/reference.artifact.base64")
                .lines()
                .collect::<String>(),
        )
        .expect("checked-in v2 Artifact uses base64");

    for fixture in fixtures.case {
        let error = if fixture.operation == "verify_artifact_limit" {
            BytecodeV2ArtifactVerifier::new(BytecodeLimits {
                max_artifact_bytes: fixture.value as usize,
                ..BytecodeLimits::default()
            })
            .verify(&artifact)
            .expect_err("static v2 Artifact must honor its configured size limit")
            .to_string()
        } else {
            let mut malformed = artifact.clone();
            match fixture.operation.as_str() {
                "set_byte" => malformed[fixture.offset] = fixture.value as u8,
                "xor_byte" => malformed[fixture.offset] ^= fixture.value as u8,
                other => panic!("unknown v2 Artifact malformed-fixture operation `{other}`"),
            }
            BytecodeV2ArtifactVerifier::default()
                .verify(&malformed)
                .expect_err("static malformed v2 Artifact must fail closed")
                .to_string()
        };
        assert!(
            error.contains(&fixture.expected),
            "{} expected error containing {:?}, got {error}",
            fixture.name,
            fixture.expected,
        );
    }
}
