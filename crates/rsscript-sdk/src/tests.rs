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
