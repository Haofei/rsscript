use super::*;
use provider::{
    BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
    ParameterSignature, ProviderCallMode, ProviderFunctionDescriptor, RUNTIME_ABI_VERSION,
    WireInterpreterFn, WireValue,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

fn admitted(built: BuiltArtifact) -> AdmittedArtifact {
    ArtifactVerifier
        .verify(built)
        .expect("verify artifact")
        .admit_trusted_input()
}

struct RejectAdmission;

impl ArtifactAdmissionPolicy for RejectAdmission {
    fn admit(&self, _artifact: &VerifiedArtifact) -> Result<ArtifactAdmission, AdmissionError> {
        Err(AdmissionError::rejected(
            "test admission policy rejected artifact",
        ))
    }
}

struct TestOriginVerifier;

impl ArtifactOriginVerifier for TestOriginVerifier {
    fn verify_origin(
        &self,
        bundle_digest: &str,
        provenance: &BuildProvenanceV1,
    ) -> Result<String, String> {
        assert!(bundle_digest.starts_with("sha256:"));
        assert!(!provenance.compiler_version.is_empty());
        Ok("sha256:test-origin-evidence".to_string())
    }
}

#[test]
fn verification_and_host_admission_are_distinct_phases() {
    let built = Compiler
        .compile("admission.rss", "fn main() -> Unit { return Unit }")
        .expect("compile");
    let verified = ArtifactVerifier.verify(built).expect("verify");
    let rejection = verified
        .admit(&RejectAdmission)
        .expect_err("host admission policy rejects artifact");
    assert_eq!(
        rejection.to_string(),
        "artifact admission rejected: test admission policy rejected artifact"
    );

    let built = Compiler
        .compile("admission.rss", "fn main() -> Unit { return Unit }")
        .expect("compile");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify")
        .admit(&TrustedInputAdmission)
        .expect("admit");
    assert_eq!(admitted.admission().policy_id(), "trusted_input.v1");
    assert_eq!(admitted.admission().evidence_digest(), None);
    let report = Runtime::default()
        .link(&admitted)
        .expect("link admitted artifact")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
}

#[test]
fn origin_verification_records_evidence_in_the_admitted_phase() {
    let built = Compiler
        .compile("origin.rss", "fn main() -> Unit { return Unit }")
        .expect("compile");
    let verified = ArtifactVerifier.verify(built).expect("verify");
    let policy = OriginVerifiedAdmission::new("detached-signature.v1", TestOriginVerifier)
        .expect("valid policy ID");
    let admitted = verified.admit(&policy).expect("origin accepted");
    assert_eq!(admitted.admission().policy_id(), "detached-signature.v1");
    assert_eq!(
        admitted.admission().evidence_digest(),
        Some("sha256:test-origin-evidence")
    );
}

#[test]
fn execution_profile_enforces_admission_and_owns_runtime_limits() {
    let built = Compiler
        .compile(
            "profile.rss",
            "fn main() -> Unit { while true { } return Unit }",
        )
        .expect("compile");
    let admitted = admitted(built);
    let runtime = Runtime::new(ProviderRegistry::default());

    let wrong_admission = ExecutionProfileV1::new(
        "production.v1",
        RunLimits::bounded(),
        "detached-signature.v1",
        "isolated-local.v1",
    );
    assert!(matches!(
        runtime.link_with_profile(&admitted, wrong_admission),
        Err(LinkError::Profile(_))
    ));

    let profile = ExecutionProfileV1::new(
        "production.v1",
        RunLimits::bounded().with_step_budget(32),
        "trusted_input.v1",
        "isolated-local.v1",
    );
    let linked = runtime
        .link_with_profile(&admitted, profile)
        .expect("matching profile links");
    let report =
        linked.execute(ExecutionRequest::default().limits(RunLimits::unbounded_for_trusted_host()));
    assert_eq!(
        report.termination_reason(),
        TerminationReason::StepBudgetExceeded,
        "profile limits must override a caller request that tries to widen them"
    );
}

#[test]
fn every_script_or_provider_failure_retains_a_report_safe_terminal_outcome() {
    // This is deliberately table-driven rather than a representative-sample
    // test. `LinkedArtifact::execute` converts both immediate VM errors and
    // normal VM outputs through this mapping, so a newly added execution
    // failure cannot accidentally escape through a Result-returning
    // convenience API without this test needing an update.
    let execution_failures = [
        (
            ExecutionFailureKind::Cancelled,
            TerminationReason::Cancelled,
        ),
        (
            ExecutionFailureKind::DeadlineExceeded,
            TerminationReason::DeadlineExceeded,
        ),
        (
            ExecutionFailureKind::StepBudgetExceeded,
            TerminationReason::StepBudgetExceeded,
        ),
        (
            ExecutionFailureKind::AllocationBudgetExceeded,
            TerminationReason::AllocationBudgetExceeded,
        ),
        (
            ExecutionFailureKind::LiveMemoryLimitExceeded,
            TerminationReason::LiveMemoryLimitExceeded,
        ),
        (
            ExecutionFailureKind::OutputLimitExceeded,
            TerminationReason::OutputLimitExceeded,
        ),
        (
            ExecutionFailureKind::IntrinsicBudgetExceeded,
            TerminationReason::IntrinsicBudgetExceeded,
        ),
        (
            ExecutionFailureKind::ProviderBudgetExceeded,
            TerminationReason::ProviderBudgetExceeded,
        ),
        (
            ExecutionFailureKind::ResourceLimitExceeded,
            TerminationReason::ResourceLimitExceeded,
        ),
    ];
    for (kind, reason) in execution_failures {
        let report = ExecutionReport::failed(
            "sha256:test",
            RuntimeError::from_execution(EvalError::execution(kind, "test failure")),
            Vec::new(),
            Duration::ZERO,
            None,
        );
        assert_eq!(report.termination_reason(), reason);
        assert!(matches!(report.outcome(), ExecutionOutcome::Failed(_)));
        assert!(report.failure().is_some());
        assert!(report.value().is_none());
    }

    for (code, reason) in [
        (
            provider::ProviderErrorCode::InvalidArgument,
            TerminationReason::ProviderError,
        ),
        (
            provider::ProviderErrorCode::NotFound,
            TerminationReason::ProviderError,
        ),
        (
            provider::ProviderErrorCode::PermissionDenied,
            TerminationReason::ProviderError,
        ),
        (
            provider::ProviderErrorCode::Cancelled,
            TerminationReason::Cancelled,
        ),
        (
            provider::ProviderErrorCode::DeadlineExceeded,
            TerminationReason::DeadlineExceeded,
        ),
        (
            provider::ProviderErrorCode::ResourceExhausted,
            TerminationReason::ProviderError,
        ),
        (
            provider::ProviderErrorCode::Unavailable,
            TerminationReason::ProviderError,
        ),
        (
            provider::ProviderErrorCode::Internal,
            TerminationReason::ProviderError,
        ),
    ] {
        let report = ExecutionReport::failed(
            "sha256:test",
            RuntimeError::from_execution(EvalError::Provider(provider::ProviderError::new(
                code,
                "provider failure",
            ))),
            Vec::new(),
            Duration::ZERO,
            None,
        );
        assert_eq!(report.termination_reason(), reason);
        assert!(matches!(report.outcome(), ExecutionOutcome::Failed(_)));
        assert!(report.failure().is_some());
    }

    let diagnostics = vec![Diagnostic::error(
        "E_TEST",
        "test diagnostic",
        Span::default(),
        "test label",
    )];
    let report = ExecutionReport::failed(
        "sha256:test",
        RuntimeError::from_execution(EvalError::Diagnostics(diagnostics.clone())),
        diagnostics,
        Duration::ZERO,
        None,
    );
    assert_eq!(
        report.termination_reason(),
        TerminationReason::VerificationFailure
    );
    assert!(matches!(report.outcome(), ExecutionOutcome::Failed(_)));

    let report = ExecutionReport::failed(
        "sha256:test",
        RuntimeError::from_execution(EvalError::Runtime("script failure".to_string())),
        Vec::new(),
        Duration::ZERO,
        None,
    );
    assert_eq!(report.termination_reason(), TerminationReason::ScriptError);
    assert!(matches!(report.outcome(), ExecutionOutcome::Failed(_)));
}

#[cfg(feature = "project")]
#[test]
fn project_loader_capture_feeds_the_pure_frontend_compiler_boundary() {
    let directory = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(directory.path().join("src")).expect("source directory");
    std::fs::create_dir(directory.path().join("interfaces")).expect("interface directory");
    std::fs::write(
            directory.path().join("rsspkg.toml"),
            "[package]\nname = \"captured-project\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("manifest");
    std::fs::write(
        directory.path().join("src/main.rss"),
        "fn main() -> Int { return 42 }\n",
    )
    .expect("source");
    std::fs::write(
        directory.path().join("interfaces/host.rssi"),
        "module host\npub fn version() -> Int\n",
    )
    .expect("interface");

    let project = project::ProjectCompiler::new();
    let captured = project
        .capture_frontend_from(directory.path(), std::path::Path::new("."))
        .expect("explicit-base loader capture");
    assert!(captured.content_digest().starts_with("sha256:"));
    assert!(
        captured
            .files()
            .iter()
            .all(|file| !file.logical_path.starts_with('/')),
        "compiler-facing snapshot identity must not contain host-absolute paths"
    );
    assert!(
        captured
            .frontend()
            .sources()
            .files()
            .iter()
            .any(|file| file.path() == "root/src/main.rss")
    );
    assert!(
        captured
            .frontend()
            .interfaces()
            .files()
            .iter()
            .any(|file| file.path() == "root/interfaces/host.rssi")
    );
    let built = project
        .build_captured(&captured)
        .expect("pure compiler accepts the loader-captured input");
    assert!(!built.artifact_bytes().is_empty());
    assert_eq!(built.snapshot_digest(), captured.frontend_digest());
    let convenience = project
        .compile_package(directory.path())
        .expect("package convenience path captures once then uses the pure compiler");
    assert_eq!(convenience.artifact_bytes(), built.artifact_bytes());
    assert_eq!(convenience.snapshot_digest(), captured.frontend_digest());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = project
        .build_captured_with_operation(
            &captured,
            &OperationContext {
                cancellation: Some(cancellation),
                ..OperationContext::default()
            },
        )
        .expect_err("cancelled captured build");
    assert_eq!(cancelled.code(), CompileErrorCode::Cancelled);
    let loader_cancel = CancellationToken::new();
    loader_cancel.cancel();
    let cancelled = project
        .compile_package_with_operation(
            directory.path(),
            &OperationContext {
                cancellation: Some(loader_cancel),
                ..OperationContext::default()
            },
        )
        .expect_err("cancelled package capture");
    assert_eq!(cancelled.code(), CompileErrorCode::Cancelled);
}

#[cfg(feature = "project")]
#[test]
fn project_capture_builds_with_dependency_interface_inputs() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path().join("root");
    let dependency = workspace.path().join("dependency");
    for directory in [root.join("src"), dependency.join("interfaces")] {
        std::fs::create_dir_all(directory).expect("package directory");
    }
    std::fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[dependencies]\ndependency = { path = \"../dependency\" }\n",
        )
        .expect("root manifest");
    std::fs::write(
        root.join("src/main.rss"),
        "fn main() -> Int { return Dependency.value() }\n",
    )
    .expect("root source");
    std::fs::write(
        dependency.join("rsspkg.toml"),
        "[package]\nname = \"dependency\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .expect("dependency manifest");
    std::fs::write(
        dependency.join("interfaces/dependency.rssi"),
        "pub fn Dependency.value() -> Int\n",
    )
    .expect("dependency interface");

    let project = project::ProjectCompiler::new();
    let captured = project
        .capture_frontend_from(workspace.path(), std::path::Path::new("root"))
        .expect("capture dependency interfaces");
    assert!(captured.frontend().interfaces().files().iter().any(|file| {
        file.path().starts_with("dependency/")
            && file.path().ends_with("/interfaces/dependency.rssi")
    }));
    let direct = project.build_captured(&captured).expect("pure build");
    let convenience = project.compile_package(&root).expect("convenience build");
    assert_eq!(direct.artifact_bytes(), convenience.artifact_bytes());
    assert_eq!(direct.snapshot_digest(), captured.frontend_digest());
}

#[test]
fn stable_facade_compiles_serializes_loads_and_runs() {
    let compiler = Compiler;
    let package = compiler
        .compile("main.rss", "fn main() -> Unit { return Unit }")
        .expect("compile");
    let bundle_bytes = package.bundle_bytes().expect("bundle");
    let loaded = ArtifactVerifier
        .verify_bytes(&bundle_bytes)
        .expect("load verified")
        .admit_trusted_input();
    let runtime = Runtime::default();
    let report = runtime
        .link(&loaded)
        .expect("link")
        .execute(ExecutionRequest::default());
    assert!(matches!(
        report.outcome(),
        ExecutionOutcome::Completed {
            wire_value,
            display_value,
        } if wire_value == &Some(provider::WireValue::Unit) && display_value == "Unit"
    ));
    assert_eq!(report.value(), Some("Unit"));
    assert_eq!(report.wire_value(), Some(&provider::WireValue::Unit));
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert_eq!(report.artifact_digest, loaded.module_digest());
    assert!(report.usage.steps_consumed > 0);
    assert_eq!(report.termination_reason().as_str(), "completed");
    let json = serde_json::to_value(&report).expect("serialize execution report");
    assert_eq!(json["schema"], EXECUTION_REPORT_SCHEMA);
    assert_eq!(json["outcome"]["kind"], "completed");
    assert_eq!(json["outcome"]["wire_value"]["kind"], "unit");
    assert!(json["usage"]["steps_consumed"].as_u64().unwrap() > 0);
    assert_eq!(
        CompileErrorCode::PackageSnapshot.as_str(),
        "package_snapshot"
    );
    assert!(!RunLimits::bounded().blocking_provider_calls_allowed());
    assert!(RunLimits::unbounded_for_trusted_host().blocking_provider_calls_allowed());
}

#[test]
fn stable_facade_exposes_scalar_results_as_canonical_wire_values() {
    let built = Compiler
        .compile("main.rss", "fn main() -> Int { return 42 }")
        .expect("compile scalar result");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify scalar result")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link scalar result")
        .execute(ExecutionRequest::default());

    assert_eq!(
        report.wire_value(),
        Some(&provider::WireValue::Int { value: 42 })
    );
    assert_eq!(report.value(), Some("42"));
    assert_eq!(report.display_value(), Some("42"));
}

#[test]
fn stable_facade_exposes_v1_record_results_as_canonical_wire_values() {
    let built = Compiler
        .compile(
            "main.rss",
            "struct Point { x: Int }\nfn main() -> Point { return Point(x: 42) }",
        )
        .expect("compile record result");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify record result")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link record result")
        .execute(ExecutionRequest::default());

    assert!(matches!(
        report.wire_value(),
        Some(provider::WireValue::Record { fields, .. })
            if fields == &vec![provider::WireValue::Int { value: 42 }]
    ));
}

#[test]
fn stable_facade_exposes_v1_named_variant_results_as_canonical_wire_values() {
    let built = Compiler
            .compile(
                "main.rss",
                "sum ResultValue { Empty, Value(count: Int) }\nfn main() -> ResultValue { return Value(count: 42) }",
            )
            .expect("compile named variant result");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("verify named variant result")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link named variant result")
        .execute(ExecutionRequest::default());

    assert!(matches!(
        report.wire_value(),
        Some(provider::WireValue::Variant {
            variant_id,
            payload: Some(payload),
            ..
        }) if *variant_id == provider::WireVariantId::new(1)
            && payload.as_ref() == &provider::WireValue::Int { value: 42 }
    ));
}

#[test]
fn source_artifacts_carry_resolved_call_facts_for_semantic_diff() {
    let compiler = Compiler;
    let old = compiler
        .compile(
            "call-facts.rss",
            "fn main() -> Int { return helper() }\nfn helper() -> Int { return 1 }",
        )
        .expect("baseline source compiles");
    let new = compiler
        .compile_with_interfaces(
            &[(
                "call-facts.rss",
                "fn main() -> Int { return helper() }\nfn helper() -> Int { return Host.value() }",
            )],
            &[("host.rssi", "fn Host.value() -> Int")],
        )
        .expect("external-call source compiles");

    let analysis = new
        .source_analysis()
        .expect("source build carries typed source analysis");
    assert!(
        analysis
            .call_edges
            .iter()
            .any(|edge| edge.caller == "main" && edge.callee == "helper")
    );
    assert!(
        analysis
            .call_edges
            .iter()
            .any(|edge| edge.caller == "helper" && edge.callee == "Host.value")
    );
    assert_eq!(analysis.external_calls.len(), 1);
    assert_eq!(analysis.external_calls[0].function, "helper");
    assert_eq!(analysis.external_calls[0].symbol, "Host.value");

    let diff = SemanticDiffV2::between(old.bundle(), new.bundle());
    assert!(
        diff.call_edges
            .added
            .iter()
            .any(|edge| edge.caller == "helper" && edge.callee == "Host.value")
    );
    assert!(
        diff.external_calls
            .added
            .iter()
            .any(|call| call.function == "helper" && call.symbol == "Host.value")
    );
}

#[test]
fn source_artifacts_carry_ownership_and_retention_contracts_for_semantic_diff() {
    let compiler = Compiler;
    let old = compiler
        .compile(
            "contracts.rss",
            r#"
struct Payload { value: Int }
fn process(value: mut Payload) -> Unit { return Unit }
fn main() -> Unit { return Unit }
"#,
        )
        .expect("baseline ownership contract compiles");
    let new = compiler
        .compile(
            "contracts.rss",
            r#"
struct Payload { value: Int }
fn process(value: read Payload) -> Unit retains(value) { return Unit }
fn main() -> Unit { return Unit }
"#,
        )
        .expect("retention contract compiles");

    let analysis = new
        .source_analysis()
        .expect("source build carries typed source analysis");
    let process = analysis
        .exports
        .iter()
        .find(|export| export.name == "process")
        .expect("function contract is recorded");
    assert_eq!(process.parameters[0].effect, "read");
    assert!(process.parameters[0].retained);
    assert_eq!(process.retained_params, ["value"]);
    assert!(
        process
            .semantic_facts
            .iter()
            .any(|fact| fact == "retains(value)")
    );

    let diff = SemanticDiffV2::between(old.bundle(), new.bundle());
    let changed = diff
        .exports
        .changed
        .iter()
        .find(|change| change.old.name == "process")
        .expect("ownership contract change is diffed");
    assert_eq!(changed.old.parameters[0].effect, "mut");
    assert_eq!(changed.new.parameters[0].effect, "read");
    assert_eq!(changed.new.retained_params, ["value"]);
}

#[test]
fn execution_usage_reports_structured_task_lifecycle() {
    let source = r#"
async fn work(value: Int) -> Result<Int, String> {
    return Ok(value)
}

fn main() -> Result<Unit, String> {
    task_group {
        async let first = work(value: 1)
        async let second = work(value: 2)
        let first_value = await first?
        let second_value = await second?
        let total = first_value + second_value
    }
    return Ok(Unit)
}
"#;
    let package = admitted(Compiler.compile("tasks.rss", source).expect("compile"));
    let report = Runtime::default()
        .link(&package)
        .expect("link")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert_eq!(report.usage.tasks_created, 3);
    assert_eq!(report.usage.tasks_completed, 3);
    assert_eq!(report.usage.tasks_cancelled, 0);
    assert_eq!(report.usage.tasks_peak_live, 3);
    assert_eq!(report.usage.tasks_live_at_return, 0);
}

#[test]
fn mir_result_try_short_circuits_through_verified_bytecode() {
    let source = r#"
fn fail() -> Result<Int, String> {
    return Err("boom")
}

fn main() -> Result<Int, String> {
    let value = fail()?
    return Ok(value)
}
"#;
    let package = admitted(Compiler.compile("result-try.rss", source).expect("compile"));
    let report = Runtime::default()
        .link(&package)
        .expect("link")
        .execute(ExecutionRequest::default());
    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert!(report.value().is_some_and(|value| value.contains("Err")));
    assert!(report.value().is_some_and(|value| value.contains("boom")));
}

#[test]
fn cancelled_execution_reports_request_to_observation_latency() {
    let package = admitted(
        Compiler
            .compile(
                "cancel.rss",
                "fn main() -> Unit { while true {} return Unit }",
            )
            .expect("compile"),
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let runtime = Runtime::new(ProviderRegistry::default());
    let report = runtime.link(&package).expect("link").execute(
        ExecutionRequest::default().limits(RunLimits::bounded().with_cancellation(cancellation)),
    );
    assert_eq!(report.termination_reason(), TerminationReason::Cancelled);
    assert!(matches!(
        report.outcome(),
        ExecutionOutcome::Failed(error) if error.reason == TerminationReason::Cancelled
    ));
    assert!(report.telemetry.cancellation_latency_ns.is_some());
    assert!(report.telemetry.execution_duration_ns > 0);
}

#[test]
fn pre_cancelled_short_artifact_never_reaches_successful_completion() {
    // A cancellation check only at the steady-state poll interval would
    // incorrectly let a small Artifact finish before the first poll. This
    // is the public SDK regression for the VM's first-instruction gate.
    let package = admitted(
        Compiler
            .compile("short-cancel.rss", "fn main() -> Unit { return Unit }")
            .expect("compile"),
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let report = Runtime::default().link(&package).expect("link").execute(
        ExecutionRequest::default().limits(RunLimits::bounded().with_cancellation(cancellation)),
    );

    assert_eq!(report.termination_reason(), TerminationReason::Cancelled);
    assert!(report.failure().is_some());
    assert!(report.value().is_none());
    assert_eq!(report.usage.steps_consumed, 1);
}

#[test]
fn compiler_and_loader_observe_shared_operation_control() {
    let compiler = Compiler;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = OperationContext {
        cancellation: Some(cancellation),
        ..OperationContext::default()
    };
    let error = compiler
        .check_with_operation(
            "cancelled.rss",
            "fn main() -> Unit { return Unit }",
            &cancelled,
        )
        .expect_err("cancelled check");
    assert_eq!(error.code(), CompileErrorCode::Cancelled);
    let error = compiler
        .compile_with_operation(
            "cancelled.rss",
            "fn main() -> Unit { return Unit }",
            &cancelled,
        )
        .expect_err("cancelled compile");
    assert_eq!(error.code(), CompileErrorCode::Cancelled);

    let package = compiler
        .compile("main.rss", "fn main() -> Unit { return Unit }")
        .expect("compile fixture");
    let expired = OperationContext {
        deadline: Some(MonotonicDeadline::at(
            std::time::Instant::now() - std::time::Duration::from_millis(1),
        )),
        ..OperationContext::default()
    };
    let error = ArtifactVerifier
        .verify_bytes_with_operation(&package.bundle_bytes().unwrap(), &expired)
        .expect_err("expired verifier deadline");
    assert!(matches!(
        error,
        VerifyError::Operation(OperationAbort::DeadlineExceeded)
    ));
    assert!(error.to_string().contains("deadline exceeded"));

    let bytes = package.bundle_bytes().expect("bundle bytes");
    let ordinary = ArtifactVerifier
        .verify_bytes(&bytes)
        .expect("ordinary verification");
    let operation_aware = ArtifactVerifier
        .verify_bytes_with_operation(&bytes, &OperationContext::default())
        .expect("operation-aware verification");
    assert_eq!(ordinary.module_digest(), operation_aware.module_digest());
    assert_eq!(
        ordinary.bytecode_artifact().header.executable_hash,
        operation_aware.bytecode_artifact().header.executable_hash
    );
}

#[test]
fn frontend_snapshot_is_the_shared_check_and_compile_input() {
    let input = FrontendInputSnapshot::from_sources(
        [(
            "main.rss",
            "module app\nuse host.*\nfn main() -> Int { return value() }\n",
        )],
        [("host.rssi", "module host\npub fn value() -> Int\n")],
    );
    let compiler = Compiler;
    assert!(compiler.check_snapshot(&input).is_empty());
    let artifact = compiler
        .compile_snapshot(&input)
        .expect("the checked snapshot should compile");
    assert_eq!(
        artifact.analysis_envelope().payload()["snapshot_digest"],
        artifact.snapshot_digest()
    );
}

#[test]
fn exceptional_frontend_fixtures_preserve_direct_analyzer_diagnostics() {
    let compiler = Compiler;
    let empty_path =
        FrontendInputSnapshot::single("", "fn main() -> Int { return Missing.value }\n");
    assert_eq!(
        legacy_frontend_fixtures::snapshot_reason(&empty_path),
        Some(legacy_frontend_fixtures::SnapshotReason::Empty)
    );
    assert_eq!(
        diagnostic_fingerprint(compiler.check_snapshot(&empty_path)),
        diagnostic_fingerprint(analyze_sources_with_interfaces(
            &[("", "fn main() -> Int { return Missing.value }\n")],
            &[],
        )),
        "empty-path fixture must preserve its historical analyzer result"
    );

    let duplicate_interface = FrontendInputSnapshot::from_sources(
        [(
            "main.rss",
            "module app\nuse host.*\nfn main() -> Unit { ping(); return Unit }\n",
        )],
        [
            ("host.rssi", "module host\npub fn ping() -> Unit\n"),
            ("host.rssi", "module host\npub fn ping() -> Unit\n"),
        ],
    );
    assert_eq!(
        legacy_frontend_fixtures::snapshot_reason(&duplicate_interface),
        Some(legacy_frontend_fixtures::SnapshotReason::DuplicateInterface)
    );
    assert_eq!(
        diagnostic_fingerprint(compiler.check_snapshot(&duplicate_interface)),
        diagnostic_fingerprint(analyze_sources_with_interfaces(
            &[(
                "main.rss",
                "module app\nuse host.*\nfn main() -> Unit { ping(); return Unit }\n",
            )],
            &[
                ("host.rssi", "module host\npub fn ping() -> Unit\n"),
                ("host.rssi", "module host\npub fn ping() -> Unit\n"),
            ],
        )),
        "duplicate-interface fixture must not be silently overwritten by the session store"
    );

    let ordinary =
        FrontendInputSnapshot::single("ordinary.rss", "fn main() -> Unit { return Unit }\n");
    assert_eq!(legacy_frontend_fixtures::snapshot_reason(&ordinary), None);
    assert!(compiler.check_snapshot(&ordinary).is_empty());
}

fn diagnostic_fingerprint(diagnostics: Vec<Diagnostic>) -> Vec<(String, String)> {
    diagnostics
        .into_iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.summary))
        .collect()
}

#[test]
fn frontend_snapshot_file_enumeration_does_not_change_artifact_bytes() {
    let first = FrontendInputSnapshot::from_sources(
        [
            ("helper.rss", "fn helper() -> Int { return 41 }\n"),
            ("main.rss", "fn main() -> Int { return helper() + 1 }\n"),
        ],
        std::iter::empty(),
    );
    let second = FrontendInputSnapshot::from_sources(
        [
            ("main.rss", "fn main() -> Int { return helper() + 1 }\n"),
            ("helper.rss", "fn helper() -> Int { return 41 }\n"),
        ],
        std::iter::empty(),
    );
    let compiler = Compiler;
    let first = compiler.compile_snapshot(&first).expect("first snapshot");
    let second = compiler.compile_snapshot(&second).expect("second snapshot");
    assert_eq!(first.snapshot_digest(), second.snapshot_digest());
    assert_eq!(
        first.bundle_bytes().unwrap(),
        second.bundle_bytes().unwrap()
    );
}

#[test]
fn module_interface_keeps_stable_external_symbol_and_preflights_signature() {
    let compiler = Compiler;
    let input = FrontendInputSnapshot::from_sources(
        [(
            "main.rss",
            "module app\nuse host.log.*\nfn main() -> Unit { emit(message: read \"ok\"); return Unit }",
        )],
        [(
            "log.rssi",
            "module host.log\npub fn emit(message: read String) -> Unit\n",
        )],
    );
    let package = compiler
        .compile_snapshot(&input)
        .expect("compile external call");
    assert_eq!(package.external_imports().len(), 1);
    assert_eq!(package.external_imports()[0].symbol, "host.log.emit");

    let incompatible = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "message".into(),
            effect: DataEffect::Take,
            ty: "String".into(),
            retained: false,
        }],
        result: "Unit".into(),
        asynchronous: false,
    };
    let symbol = ExternalSymbol::new("host.log.emit").expect("symbol");
    let descriptor = ProviderDescriptor {
        provider_id: "test.log".into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: incompatible.clone(),
            entry: "emit".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: provider::ResourceCleanupContract::None,
            error_mapping: provider::ProviderErrorMapping::StructuredV1,
        }],
    };
    let called = Arc::new(AtomicBool::new(false));
    let called_by_provider = Arc::clone(&called);
    let mut providers = ProviderRegistry::default();
    providers
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature: incompatible,
                    callable: WireInterpreterFn::new(move |_| {
                        called_by_provider.store(true, Ordering::SeqCst);
                        Ok(WireValue::Unit)
                    }),
                },
            )]),
        )
        .expect("provider descriptor and implementation should match");

    let package = admitted(package);
    let runtime = Runtime::new(providers);
    let error = match runtime.link(&package) {
        Ok(_) => panic!("import signature must fail before execution"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ImportSignatureMismatch"));
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn provider_calls_have_a_budget_separate_from_intrinsics() {
    let compiler = Compiler;
    let package = compiler
            .compile_with_interfaces(
                &[(
                    "main.rss",
                    "module app\nuse host.log.*\nfn main() -> Unit { emit(message: read \"one\"); emit(message: read \"two\"); return Unit }",
                )],
                &[(
                    "log.rssi",
                    "module host.log\npub fn emit(message: read String) -> Unit\n",
                )],
            )
            .expect("compile external calls");
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "message".into(),
            effect: DataEffect::Read,
            ty: "String".into(),
            retained: false,
        }],
        result: "Unit".into(),
        asynchronous: false,
    };
    let symbol = ExternalSymbol::new("host.log.emit").expect("symbol");
    let descriptor = ProviderDescriptor {
        provider_id: "test.log".into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "emit".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: provider::ResourceCleanupContract::None,
            error_mapping: provider::ProviderErrorMapping::StructuredV1,
        }],
    };
    let mut providers = ProviderRegistry::default();
    providers
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new(|_| Ok(WireValue::Unit)),
                },
            )]),
        )
        .expect("register provider");
    let limits = RunLimits::default().with_provider_call_budget(1);

    let package = admitted(package);
    let runtime = Runtime::new(providers);
    let report = runtime.link(&package).expect("link providers").execute(
        ExecutionRequest::default()
            .limits(limits)
            .trace(TracePolicy::MetadataOnly),
    );
    assert_eq!(
        report.termination_reason(),
        TerminationReason::ProviderBudgetExceeded
    );
    assert_eq!(report.usage.provider_calls, 2);
    assert_eq!(report.provider_call_traces.len(), 1);
    assert!(
        report
            .failure()
            .is_some_and(|error| error.message.contains("provider call budget exceeded"))
    );

    let failure_symbol = descriptor.functions[0].symbol.clone();
    let failure_signature = descriptor.functions[0].signature.clone();
    let mut failing_providers = ProviderRegistry::default();
    failing_providers
        .register(
            &descriptor,
            BTreeMap::from([(
                failure_symbol,
                ProviderFunction {
                    signature: failure_signature,
                    callable: WireInterpreterFn::new(|_| {
                        Err(provider::ProviderError::invalid_argument(
                            "rejected by provider",
                        ))
                    }),
                },
            )]),
        )
        .expect("register failing provider");
    let runtime = Runtime::new(failing_providers);
    let report = runtime
        .link(&package)
        .expect("link failing provider")
        .execute(ExecutionRequest::default().trace(TracePolicy::MetadataOnly));
    assert_eq!(
        report.termination_reason(),
        TerminationReason::ProviderError
    );
    assert_eq!(report.provider_call_traces.len(), 1);
    assert_eq!(
        report.provider_call_traces[0].result,
        Err(provider::ProviderErrorCode::InvalidArgument)
    );
    assert!(
        report
            .failure()
            .is_some_and(|error| error.message == "provider call failed (invalid_argument)")
    );
}

#[test]
fn default_reports_redact_provider_controlled_failure_text_and_payloads() {
    let compiler = Compiler;
    let package = compiler
        .compile_with_interfaces(
            &[(
                "main.rss",
                "module app\nuse host.test.*\nfn main() -> Unit { fail(); return Unit }",
            )],
            &[("test.rssi", "module host.test\npub fn fail() -> Unit\n")],
        )
        .expect("compile package");
    let package = admitted(package);
    let signature = FunctionSignature {
        parameters: vec![],
        result: "Unit".into(),
        asynchronous: false,
    };
    let symbol = ExternalSymbol::new("host.test.fail").expect("symbol");
    let descriptor = ProviderDescriptor {
        provider_id: "test.failure".into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "fail".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: provider::ResourceCleanupContract::None,
            error_mapping: provider::ProviderErrorMapping::StructuredV1,
        }],
    };
    let mut providers = ProviderRegistry::default();
    providers
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new(|_| {
                        let mut error =
                            provider::ProviderError::invalid_argument("secret-token=do-not-report");
                        error.details = Some(provider::WireValue::String {
                            value: "credential=do-not-report".into(),
                        });
                        Err(error)
                    }),
                },
            )]),
        )
        .expect("register provider");

    let report = Runtime::new(providers)
        .link(&package)
        .expect("link provider")
        .execute(ExecutionRequest::default());
    assert_eq!(
        report.termination_reason(),
        TerminationReason::ProviderError
    );
    assert!(report.provider_call_traces.is_empty());
    assert_eq!(report.telemetry.provider_functions.len(), 1);
    let failure = report.failure().expect("provider failure evidence");
    assert_eq!(failure.message, "provider call failed (invalid_argument)");
    let serialized = serde_json::to_string(&report).expect("serialize report");
    assert!(!serialized.contains("secret-token"));
    assert!(!serialized.contains("credential"));
}

#[test]
fn provider_host_context_and_trace_reach_the_execution_report() {
    let compiler = Compiler;
    let package = compiler
            .compile_with_interfaces(
                &[(
                    "main.rss",
                    "module app\nuse host.log.*\nfn main() -> Unit { emit(message: read \"ok\"); return Unit }",
                )],
                &[(
                    "log.rssi",
                    "module host.log\npub fn emit(message: read String) -> Unit\n",
                )],
            )
            .expect("compile external call");
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "message".into(),
            effect: DataEffect::Read,
            ty: "String".into(),
            retained: false,
        }],
        result: "Unit".into(),
        asynchronous: false,
    };
    let symbol = ExternalSymbol::new("host.log.emit").expect("symbol");
    let descriptor = ProviderDescriptor {
        provider_id: "test.log".into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "emit".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: provider::ResourceCleanupContract::None,
            error_mapping: provider::ProviderErrorMapping::StructuredV1,
        }],
    };
    let mut providers = ProviderRegistry::default();
    providers.set_host_call_context(provider::HostCallContext::with_labels(["log.emit"]));
    providers
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new_contextual(|context, _| {
                        assert!(context.host_context.has_label("log.emit"));
                        assert_eq!(context.provider_id, "test.log");
                        assert_eq!(context.symbol, "host.log.emit");
                        Ok(WireValue::Unit)
                    }),
                },
            )]),
        )
        .expect("register provider");

    let package = admitted(package);
    let runtime = Runtime::new(providers);
    let report = runtime
        .link(&package)
        .expect("link provider")
        .execute(ExecutionRequest::default().trace(TracePolicy::MetadataOnly));
    assert_eq!(report.provider_call_traces.len(), 1);
    let trace = &report.provider_call_traces[0];
    assert_eq!(trace.provider_id, "test.log");
    assert_eq!(trace.provider_version, "1");
    assert_eq!(trace.symbol, "host.log.emit");
    assert_eq!(trace.request_bytes, 2);
    assert_eq!(trace.response_bytes, 0);
    assert_eq!(trace.result, Ok(()));
    assert_eq!(report.telemetry.provider_functions.len(), 1);
    let summary = &report.telemetry.provider_functions[0];
    assert_eq!(summary.provider_id, "test.log");
    assert_eq!(summary.symbol, "host.log.emit");
    assert_eq!(summary.calls, 1);
    assert_eq!(summary.failures, 0);
    assert_eq!(summary.request_bytes, 2);
    assert_eq!(summary.response_bytes, 0);
    assert_eq!(summary.total_duration_ns, summary.max_duration_ns);
}

/// A tuple is a synthetic `__TupleN<A, B, ..>` generic struct, so every tuple
/// value is a generic instance whose type arguments have to be substituted
/// before they reach the typed executable facts. Until they were, a program
/// that returned a tuple compiled and then failed bytecode verification with
/// `__Tuple2<A, B>` — the declaration's parameter names — where the concrete
/// element types belonged. The same fixture is checked by
/// `tests/fixtures/pass/tuple-values-round-trip.rss`; this runs it.
#[test]
fn tuple_values_survive_verification_and_execute() {
    const SOURCE: &str = r#"
struct Point {
    location: (Int, Int)
}

fn labelled() -> (Int, String) {
    return (1, "one")
}

fn widened(value: Int) -> (Int, Int, Bool) {
    return (value, value * 2, true)
}

fn sum(pair: read (Int, Int)) -> Int {
    return pair.item0 + pair.item1
}

fn picked(values: read List<Int>) -> (Int, Int) {
    return (values[0], values[1] + 1)
}

fn main() -> String {
    local pair = labelled()
    let (base, doubled, flag) = widened(value: pair.item0)
    local point = Point(location: (base, doubled))
    let total = sum(pair: point.location)
    local values = [4, 5]
    let (head, next) = picked(values: values)
    return String.concat(
        left: pair.item1,
        right: Int.to_string(value: total + head + next),
    )
}
"#;

    let built = Compiler
        .compile("tuples.rss", SOURCE)
        .expect("a tuple-returning program compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("substituted tuple type arguments pass bytecode verification")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link tuple program")
        .execute(ExecutionRequest::default());

    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    // `labelled()` is `(1, "one")`; `widened(value: 1)` is `(1, 2, true)`, so
    // the stored `Point.location` is `(1, 2)` and `sum` is 3; `picked` reads
    // `(4, 6)` off the list. 3 + 4 + 6 = 13.
    assert_eq!(report.value(), Some("one13"));
}

/// Tuple arity is not capped by the generated type-parameter names: the
/// synthetic parameters are unique for any arity, and substitution is keyed by
/// declared name rather than by spelling, so a wide tuple resolves its
/// elements and runs like a narrow one.
#[test]
fn wide_tuples_keep_distinct_type_parameters_and_execute() {
    const ARITY: usize = 30;
    let element_types = std::iter::repeat_n("Int", ARITY - 1)
        .chain(std::iter::once("String"))
        .collect::<Vec<_>>()
        .join(", ");
    let values = (0..ARITY - 1)
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        r#"
fn wide() -> ({element_types}) {{
    return ({values}, "last")
}}

fn main() -> String {{
    local tuple = wide()
    return String.concat(
        left: Int.to_string(value: tuple.item0 + tuple.item{last}),
        right: tuple.item{tail},
    )
}}
"#,
        last = ARITY - 2,
        tail = ARITY - 1,
    );

    let built = Compiler
        .compile("wide-tuple.rss", &source)
        .expect("an arity-30 tuple compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("an arity-30 tuple passes bytecode verification")
        .admit_trusted_input();
    let report = Runtime::default()
        .link(&admitted)
        .expect("link wide tuple program")
        .execute(ExecutionRequest::default());

    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    // `item0` is 0 and `item28` is 28.
    assert_eq!(report.value(), Some("28last"));
}

/// Tuple patterns in `match` reach the typed MIR control-flow subset.
///
/// A tuple pattern is the synthetic struct pattern `__TupleN { item0: …, … }`
/// (`syntax/parser/pattern.rs`), so lowering tests the refutable elements as a
/// short-circuiting branch ladder and binds the rest by field projection. The
/// program exercises every element form the subset accepts — a literal, a
/// binding, `_`, and a nested tuple pattern — in both the statement and the
/// expression form of `match`.
#[test]
fn tuple_match_patterns_lower_and_execute() {
    const SOURCE: &str = r#"
fn classify(p: (Int, String)) -> fresh String {
    match read p {
        (0, "zero") => { return "both" }
        (0, name) => { return String.concat(left: "zero-", right: name) }
        (n, _) => { return String.from_int(value: n) }
    }
}

fn nested(p: ((Int, Int), String)) -> fresh String {
    match read p {
        ((1, b), name) => { return String.concat(left: String.from_int(value: b), right: name) }
        ((a, _), _) => { return String.from_int(value: a) }
        _ => { return "?" }
    }
}

fn corner(p: (Int, Int)) -> fresh String {
    return match read p {
        (0, 0) => { "origin" }
        (0, y) => { String.from_int(value: y) }
        (x, _) => { String.from_int(value: x) }
    }
}

fn main() -> fresh String {
    let parts = [
        classify(p: (0, "zero")),
        classify(p: (0, "q")),
        classify(p: (5, "q")),
        nested(p: ((1, 9), "x")),
        nested(p: ((2, 9), "x")),
        corner(p: (0, 0)),
        corner(p: (0, 4)),
        corner(p: (7, 4)),
    ]
    return String.join(parts: read parts, separator: "|")
}
"#;

    let built = Compiler
        .compile("tuple-patterns.rss", SOURCE)
        .expect("tuple patterns in `match` compile");
    let admitted = admitted(built);
    let report = Runtime::default()
        .link(&admitted)
        .expect("link tuple pattern program")
        .execute(ExecutionRequest::default());

    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert_eq!(
        report.value(),
        Some("both|zero-q|5|9x|2|origin|4|7"),
        "each arm is selected by its own element tests"
    );
}

/// A generic construction is as provable as its arguments' types, and the
/// expression forms below now carry one.
///
/// `lower_record_constructor` refuses to lower a generic record whose type
/// arguments the checker could not prove, so each form here used to be either a
/// spurious type error naming the declaration's own parameter (`__rss_T0`) or a
/// `rss build` refusal after a clean `rss check`. Every one of them now builds a
/// `Tagged<T>` and runs.
#[test]
fn generic_constructions_from_newly_typed_expressions_execute() {
    const SOURCE: &str = r#"
struct Tagged<T: Struct> {
    value: T
    label: String
}

fn lookup(key: String) -> Option<Int> {
    return Some(String.len(value: key))
}

// `?` on a typed `Option<T>` proves `T`.
fn from_option(key: String) -> Option<fresh Tagged<Int>> {
    let n = lookup(key: key)?
    return Some(Tagged(value: n, label: "opt"))
}

// A `let` bound to one of the literals the surface spells as an identifier.
fn from_literal_ident() -> fresh Tagged<Bool> {
    let flag = false
    return Tagged(value: flag, label: "bool")
}

// A `match` used as a value whose first arm takes its own value from a nested
// `if`: the arm proves nothing by itself, its branches do.
fn from_branching_match(n: Int) -> fresh Tagged<Int> {
    let score = match n {
        0 => {
            let base = 1
            if base > 0 { 5 } else { 6 }
        }
        _ => { 20 }
    }
    return Tagged(value: score, label: "match")
}

fn option_label(key: String) -> fresh String {
    match from_option(key: key) {
        Some(tagged) => { return String.from_int(value: tagged.value) }
        None => { return "none" }
    }
}

fn main() -> fresh String {
    let parts = [
        option_label(key: "abcd"),
        String.from_int(value: from_branching_match(n: 0).value),
        String.from_int(value: from_branching_match(n: 7).value),
        String.from_bool(value: from_literal_ident().value),
    ]
    return String.join(parts: read parts, separator: "|")
}
"#;

    let built = Compiler
        .compile("inferred-generic-construction.rss", SOURCE)
        .expect("every construction proves its type arguments");
    let admitted = admitted(built);
    let report = Runtime::default()
        .link(&admitted)
        .expect("link generic construction program")
        .execute(ExecutionRequest::default());

    assert_eq!(report.termination_reason(), TerminationReason::Completed);
    assert_eq!(report.value(), Some("4|5|20|false"));
}

/// Where a type argument really is undetermined, `rss check` says so.
///
/// A bare `None` leaves `Option<?>` open on purpose, so the tuple built from it
/// has no provable instance. That used to check clean and then fail the build
/// with "generic record constructor without a proved type instance"; it is now
/// an `RS0034` at the construction itself.
#[test]
fn an_unprovable_generic_construction_is_a_check_error() {
    const SOURCE: &str = r#"
fn main() -> Unit {
    let nothing = None
    let p = (nothing, 1)
    Output.write(message: String.from_int(value: p.item1))
    return Unit
}
"#;

    let error = Compiler
        .compile("unprovable-construction.rss", SOURCE)
        .expect_err("an unprovable generic construction does not compile");
    let CompileError::Diagnostics(diagnostics) = &error else {
        panic!("expected checker diagnostics, got {error}");
    };
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0034"),
        "expected RS0034 at the construction, got {codes:?}"
    );
}

/// Closure programs build, verify, and run.
///
/// An unannotated closure parameter has no proved type — the checker leaves it
/// unresolved — so the typed executable facts report it `Unknown`. Publishing
/// the unresolved marker as a `Known` nominal type named `?` instead made every
/// one of these shapes fail Artifact verification with "typed call parameter
/// disagrees with its argument register" before execution, so this covers the
/// whole build → verify → run path rather than the checker alone.
#[test]
fn closure_programs_verify_and_run() {
    // A closure bound with `local` and called in a loop: the shape of
    // `benchmarks/vm-jit/kernels/native_closure_sinking.rss`.
    const CALLED_IN_A_LOOP: &str = r#"
fn main() -> Int {
    local f = |x| { return x * 2 + 1 }
    let mut total = 0
    let mut i = 0
    while i < 10 {
        total = total + f(i)
        i = i + 1
    }
    return total
}
"#;

    // The same call site reached through a helper, so the closure's own
    // synthetic function is entered from a second frame.
    const CALLED_FROM_A_HELPER: &str = r#"
fn hot(limit: Int) -> Int {
    let mut i = 0
    let mut total = 0
    while i < limit {
        local f = |x| { return x * 2 + 1 }
        total = total + f(i)
        i = i + 1
    }
    return total
}

fn main() -> Int {
    return hot(limit: 10)
}
"#;

    // An explicit-capture closure over a local. Its captured register carries a
    // proved type while its parameter does not, so the two must be reported
    // independently.
    const CAPTURES_A_LOCAL: &str = r#"
fn main() -> Int {
    let base = 40
    local add = fn(x) captures(read base) { return x + base }
    return add(2)
}
"#;

    // The closure's result is a record: the call site's result fact stays
    // `Unknown` (MIR v1 retains no closure return type) while the constructor's
    // own facts remain proved, and the field reads still verify.
    const RETURNS_A_STRUCT: &str = r#"
struct Point {
    x: Int
    y: Int
}

fn main() -> Int {
    local make = |x| { return Point(x: x, y: x + 1) }
    let p = make(20)
    return p.x + p.y
}
"#;

    for (file, source, expected) in [
        ("closure-called-in-a-loop.rss", CALLED_IN_A_LOOP, "100"),
        (
            "closure-called-from-a-helper.rss",
            CALLED_FROM_A_HELPER,
            "100",
        ),
        ("closure-captures-a-local.rss", CAPTURES_A_LOCAL, "42"),
        ("closure-returns-a-struct.rss", RETURNS_A_STRUCT, "41"),
    ] {
        let built = Compiler
            .compile(file, source)
            .unwrap_or_else(|error| panic!("{file} compiles: {error}"));
        let admitted = ArtifactVerifier
            .verify(built)
            .unwrap_or_else(|error| panic!("{file} verifies: {error}"))
            .admit_trusted_input();
        let report = Runtime::default()
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{file} links: {error}"))
            .execute(ExecutionRequest::default());

        assert_eq!(
            report.termination_reason(),
            TerminationReason::Completed,
            "{file} runs to completion"
        );
        assert_eq!(report.value(), Some(expected), "{file} result");
    }
}

/// A closure call site publishes what it proves and nothing more.
///
/// Arity and parameter effects are real facts and stay `Known`; the unannotated
/// parameter type is not, and is reported `Unknown`. Pinning the *absence* here
/// stops the false `Known(Named "?")` from coming back as a "more precise"
/// fact, which a native tier would read as a record layout that does not exist.
#[test]
fn a_closure_call_site_reports_no_parameter_type_it_cannot_prove() {
    let built = Compiler
        .compile(
            "closure-facts.rss",
            "fn main() -> Int { local f = |x| { return x * 2 }; return f(21) }",
        )
        .expect("closure program compiles");
    let artifact = artifact::BytecodeArtifact::from_bytes(built.artifact_bytes())
        .expect("the built artifact decodes");
    let bytes = artifact
        .typed_executable_facts
        .clone()
        .expect("the build carries typed executable facts");
    let bound = rsscript_bytecode::TypedExecutableFactsVerifierV1::new(
        rsscript_bytecode::BytecodeLimits::default().into(),
    )
    .verify(&bytes, &artifact)
    .expect("the typed facts verify against their executable");

    let call = bound
        .facts()
        .functions
        .iter()
        .flat_map(|function| &function.call_sites)
        .find(|call| call.target == rsscript_bytecode::TypedCallTargetV1::Closure)
        .expect("the closure call site is published");

    // Arity and the parameter's data effect are proved; its type is not.
    assert_eq!(call.parameters.len(), 1);
    assert_eq!(
        call.parameter_effects,
        vec![rsscript_bytecode::TypedDataEffectV1::Read]
    );
    assert_eq!(
        call.parameters,
        vec![rsscript_bytecode::TypedFactTypeV1::Unknown],
        "an unannotated closure parameter must be reported as unproved"
    );
}

/// A payload-free enum case written as a bare name builds, verifies, and runs.
///
/// `None`, and a user `sum` case declared without fields, are constructions,
/// not value bindings. They reached MIR lowering as plain identifiers, so the
/// lowerer looked for a local named `None` (or `Red`), found none, and refused
/// the whole program with "unknown checked HIR local" — after `rss check` had
/// reported the file clean. The split only showed up at `rss build`, so this
/// covers build → verify → run rather than the checker alone.
#[test]
fn bare_enum_case_programs_verify_and_run() {
    // `return None` after a loop: the shape a search function ends with.
    const NONE_AFTER_A_LOOP: &str = r#"
fn first_even(values: List<Int>) -> Option<Int> {
    let mut i = 0
    while i < List.len(list: values) {
        let v = List.get(list: values, index: i)
        if v % 2 == 0 {
            return Some(v)
        }
        i = i + 1
    }
    return None
}

fn main() -> Int {
    match first_even(values: [1, 3, 4]) {
        Some(n) => { return n }
        None => { return 0 }
    }
}
"#;

    // `return None` inside an `if`, where the surrounding function still ends
    // with a `Some`.
    const NONE_INSIDE_AN_IF: &str = r#"
fn negative_is_nothing(value: Int) -> Option<Int> {
    if value < 0 {
        return None
    }
    return Some(value)
}

fn main() -> Int {
    match negative_is_nothing(value: 0 - 1) {
        Some(n) => { return n }
        None => { return 7 }
    }
}
"#;

    // `None` in an `else` branch, so the construction is the only statement on
    // that edge.
    const NONE_IN_AN_ELSE: &str = r#"
fn even_or_nothing(value: Int) -> Option<Int> {
    if value % 2 == 0 {
        return Some(value)
    } else {
        return None
    }
}

fn main() -> Int {
    match even_or_nothing(value: 7) {
        Some(n) => { return n }
        None => { return 7 }
    }
}
"#;

    // `Ok`/`Err` in the same positions. These already lowered — they are
    // written call-like — and are covered so the two variant families cannot
    // drift apart.
    const RESULT_IN_BRANCHES: &str = r#"
fn checked(value: Int) -> Result<Int, String> {
    if value < 0 {
        return Err("negative")
    }
    return Ok(value)
}

fn main() -> Int {
    let mut total = 0
    match checked(value: 5) {
        Ok(v) => { total = total + v }
        Err(e) => { total = total + 100 }
    }
    match checked(value: 0 - 3) {
        Ok(v) => { total = total + v }
        Err(e) => { total = total + 2 }
    }
    return total
}
"#;

    // A user `sum` case declared without fields. `None` is not a special case
    // of the language; it is the builtin instance of this shape, and both had
    // to stop being lowered as identifier reads.
    const NULLARY_SUM_CASE: &str = r#"
sum Color {
    Red
    Green
    Blue
}

fn rank(color: Color) -> Int {
    match color {
        Red => { return 1 }
        Green => { return 2 }
        Blue => { return 3 }
    }
}

fn main() -> Int {
    let c = Green
    return rank(color: c) + rank(color: Blue)
}
"#;

    // A local shadows the case name, so the bare mention is a value binding
    // again and must read the local.
    const A_LOCAL_SHADOWS_THE_CASE: &str = r#"
fn main() -> Int {
    let None = 4
    return None + 3
}
"#;

    for (file, source, expected) in [
        ("none-after-a-loop.rss", NONE_AFTER_A_LOOP, "4"),
        ("none-inside-an-if.rss", NONE_INSIDE_AN_IF, "7"),
        ("none-in-an-else.rss", NONE_IN_AN_ELSE, "7"),
        ("result-in-branches.rss", RESULT_IN_BRANCHES, "7"),
        ("nullary-sum-case.rss", NULLARY_SUM_CASE, "5"),
        ("local-shadows-the-case.rss", A_LOCAL_SHADOWS_THE_CASE, "7"),
    ] {
        let built = Compiler
            .compile(file, source)
            .unwrap_or_else(|error| panic!("{file} compiles: {error}"));
        let admitted = ArtifactVerifier
            .verify(built)
            .unwrap_or_else(|error| panic!("{file} verifies: {error}"))
            .admit_trusted_input();
        let report = Runtime::default()
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{file} links: {error}"))
            .execute(ExecutionRequest::default());

        assert_eq!(
            report.termination_reason(),
            TerminationReason::Completed,
            "{file} runs to completion"
        );
        assert_eq!(report.value(), Some(expected), "{file} result");
    }

    // `None()` is still the malformed call form: resolving the bare name to a
    // construction must not make the call spelling legal.
    let codes = Compiler
        .check(
            "none-call-form.rss",
            "fn bad() -> Option<Int> {\n    return None()\n}\n",
        )
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"RS0015".to_string()),
        "`None()` must stay RS0015, got {codes:?}"
    );
}

/// A closure that captures a local without saying so builds, verifies, and
/// runs.
///
/// `local f = |x| { return x + base }` carried no capture list into HIR, so the
/// closure's own lowered function had no place for `base` and the program died
/// with "unknown checked HIR local" — while the explicit
/// `fn(x) captures(read base)` spelling of the same program worked. The capture
/// set now comes from the checker's own capture rule, so the two spellings
/// agree by construction.
#[test]
fn implicitly_capturing_closures_verify_and_run() {
    // One capture, read by the body.
    const CAPTURES_A_LOCAL: &str = r#"
fn main() -> Int {
    let base = 40
    local add = |x| { return x + base }
    return add(2)
}
"#;

    // The implicit spelling of the existing explicit-capture fixture: the same
    // program, the same answer.
    const THE_EXPLICIT_SPELLING_AGREES: &str = r#"
fn implicit() -> Int {
    let base = 40
    local add = |x| { return x + base }
    return add(2)
}

fn explicit() -> Int {
    let base = 40
    local add = fn(x) captures(read base) { return x + base }
    return add(2)
}

fn main() -> Int {
    return implicit() - explicit() + 42
}
"#;

    // Two captures, one of them the enclosing function's parameter.
    const CAPTURES_A_PARAMETER_AND_A_LOCAL: &str = r#"
fn scaled(factor: Int) -> Int {
    let offset = 3
    local f = |x| { return x * factor + offset }
    return f(4)
}

fn main() -> Int {
    return scaled(factor: 10)
}
"#;

    // A captured local read on every iteration, so the capture is passed on
    // each call rather than once.
    const CAPTURE_READ_IN_A_LOOP: &str = r#"
fn main() -> Int {
    let base = 7
    local f = |x| { return x + base }
    let mut i = 0
    let mut total = 0
    while i < 3 {
        total = total + f(i)
        i = i + 1
    }
    return total
}
"#;

    // A captured record, reached through a field. The capture is the whole
    // local, not the projection.
    const CAPTURES_A_STRUCT: &str = r#"
struct Point {
    x: Int
    y: Int
}

fn main() -> Int {
    let p = Point(x: 1, y: 2)
    local f = |n| { return n + p.x + p.y }
    return f(10)
}
"#;

    for (file, source, expected) in [
        ("closure-captures-a-local.rss", CAPTURES_A_LOCAL, "42"),
        (
            "closure-spellings-agree.rss",
            THE_EXPLICIT_SPELLING_AGREES,
            "42",
        ),
        (
            "closure-captures-two.rss",
            CAPTURES_A_PARAMETER_AND_A_LOCAL,
            "43",
        ),
        (
            "closure-capture-in-a-loop.rss",
            CAPTURE_READ_IN_A_LOOP,
            "24",
        ),
        ("closure-captures-a-struct.rss", CAPTURES_A_STRUCT, "13"),
    ] {
        let built = Compiler
            .compile(file, source)
            .unwrap_or_else(|error| panic!("{file} compiles: {error}"));
        let admitted = ArtifactVerifier
            .verify(built)
            .unwrap_or_else(|error| panic!("{file} verifies: {error}"))
            .admit_trusted_input();
        let report = Runtime::default()
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{file} links: {error}"))
            .execute(ExecutionRequest::default());

        assert_eq!(
            report.termination_reason(),
            TerminationReason::Completed,
            "{file} runs to completion"
        );
        assert_eq!(report.value(), Some(expected), "{file} result");
    }
}

/// A closure that writes to a local it captured is a checker error, in both
/// spellings.
///
/// A capture reaches the closure's lowered function by value and is never
/// written back: the enclosing function keeps its old value, and the closure
/// does not even carry the write to its own next call. Before implicit captures
/// were inferred, the implicit spelling failed to lower and said so; it would
/// now lower and quietly return the wrong answer, which is why the checker
/// refuses it. The explicit spelling lowers the same way and had been returning
/// the wrong answer all along, so it is refused too.
#[test]
fn a_closure_that_writes_to_its_capture_is_rejected() {
    const IMPLICIT: &str = r#"
fn main() -> Int {
    let mut total = 0
    local bump = |n| { total = total + n }
    bump(5)
    return total
}
"#;

    const EXPLICIT: &str = r#"
fn main() -> Int {
    let mut total = 0
    local bump = fn(n) captures(mut total) {
        total = total + n
    }
    bump(5)
    return total
}
"#;

    for (file, source) in [
        ("implicit-capture-write.rss", IMPLICIT),
        ("explicit-capture-write.rss", EXPLICIT),
    ] {
        let diagnostics = Compiler.check(file, source);
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();
        assert!(
            codes.contains(&"RS0805"),
            "{file} must report RS0805, got {codes:?}"
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("writes to captured local `total`")),
            "{file} must name the mutated capture"
        );
    }

    // Reading a capture is still fine: the rule is about the write, not about
    // capturing a `mut` local.
    assert!(
        Compiler
            .check(
                "capture-read-of-a-mut-local.rss",
                "fn main() -> Int {\n    let mut base = 1\n    base = base + 1\n    local f = |n| { return n + base }\n    return f(40)\n}\n",
            )
            .is_empty(),
        "reading a capture is not a write"
    );
}
