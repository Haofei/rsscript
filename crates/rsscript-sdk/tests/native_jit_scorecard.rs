use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    experimental::native_jit::NativeJitOptions,
    provider_api::{
        BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
        ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
        ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, ProviderRegistry,
        RUNTIME_ABI_VERSION, ResourceCleanupContract, WireInterpreterFn, WireValue,
    },
    report::{ExecutionEngineTelemetry, ExecutionReport},
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const PROVIDER_INTERFACE: &str = "module host.matrix\npub fn adjust(value: read Int) -> Int\n";
const PROVIDER_SOURCE: &str = r#"
module app
use host.matrix.*

fn bench_size(args: read List<String>, default: Int) -> Int {
    let raw = Arguments.get_or_default(args: read args, index: 0, default: String.from_int(value: default))
    match String.parse_int(value: raw) {
        Some(value) => { return value }
        None => { return default }
    }
}

fn scalar(limit: Int, seed: Int) -> Int {
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + i * 3 - i / 2 + 7
        i = i + 1
    }
    return total
}

fn main(args: read List<String>) -> Unit {
    let limit = bench_size(args: read args, default: 2000)
    let before = scalar(limit, seed: 11)
    let adjusted = adjust(value: read before)
    let after = scalar(limit, seed: adjusted)
    Output.write(message: String.from_int(value: after))
    return Unit
}
"#;

struct ScorecardCase {
    name: &'static str,
    pass: &'static str,
    workload: &'static str,
    size: &'static str,
    source: &'static str,
}

const CASES: &[ScorecardCase] = &[
    ScorecardCase {
        name: "scalar-loop",
        pass: "baseline",
        workload: "pure_scalar",
        size: "200000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_scalar_loop.rss"),
    },
    ScorecardCase {
        name: "native-call-chain",
        pass: "inlining/native-call",
        workload: "static_calls",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_call_chain.rss"),
    },
    ScorecardCase {
        name: "mixed-mode-continuation",
        pass: "continuation",
        workload: "struct",
        size: "2000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/mixed_mode_continuation.rss"),
    },
    ScorecardCase {
        name: "option-scalar-replacement",
        pass: "scalar-replacement",
        workload: "option_result",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_option_scalar_replace.rss"),
    },
    ScorecardCase {
        name: "result-scalar-replacement",
        pass: "scalar-replacement",
        workload: "option_result",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_result_scalar_replace.rss"),
    },
    ScorecardCase {
        name: "struct-scalar-replacement",
        pass: "scalar-replacement",
        workload: "struct",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_struct_scalar_replace.rss"),
    },
    ScorecardCase {
        name: "variant-scalar-replacement",
        pass: "scalar-replacement",
        workload: "variant",
        size: "150000",
        source: include_str!(
            "../../../benchmarks/vm-jit/kernels/native_variant_scalar_replace.rss"
        ),
    },
    ScorecardCase {
        name: "profile-closure-pic",
        pass: "profile/PIC",
        workload: "closure",
        size: "100000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/profile_closure_pic.rss"),
    },
    ScorecardCase {
        name: "profile-branch-side-exit",
        pass: "profile/side-exit",
        workload: "pure_scalar",
        size: "100000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/profile_branch_side_exits.rss"),
    },
    ScorecardCase {
        name: "osr-scalar-loop",
        pass: "osr",
        workload: "osr",
        size: "200000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/osr_scalar_loop.rss"),
    },
    ScorecardCase {
        name: "list-read-loop",
        pass: "runtime-helper/list",
        workload: "list",
        size: "100000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_read_heap.rss"),
    },
    ScorecardCase {
        name: "map-get-loop",
        pass: "runtime-helper/map",
        workload: "map",
        size: "50000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_map_get_match_loop.rss"),
    },
    ScorecardCase {
        name: "string-processing",
        pass: "runtime-helper/string",
        workload: "string",
        size: "2000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/string_text_processing.rss"),
    },
    ScorecardCase {
        name: "async-call-loop",
        pass: "continuation/async",
        workload: "async",
        size: "2000",
        source: include_str!("../../../benchmarks/micro/async_call_loop.rss"),
    },
    ScorecardCase {
        name: "generic-mailbox",
        pass: "static-generic",
        workload: "generic_calls",
        size: "2000",
        source: include_str!("../../../benchmarks/micro/selfhost_mailbox_bench.rss"),
    },
    ScorecardCase {
        name: "provider-mixed-mode",
        pass: "continuation/provider",
        workload: "provider",
        size: "2000",
        source: PROVIDER_SOURCE,
    },
];

fn provider_registry() -> (ProviderRegistry, Arc<AtomicU64>) {
    let symbol = ExternalSymbol::new("host.matrix.adjust").expect("valid matrix symbol");
    let signature = FunctionSignature {
        parameters: vec![ParameterSignature {
            name: "value".into(),
            effect: DataEffect::Read,
            ty: "Int".into(),
            retained: false,
        }],
        result: "Int".into(),
        asynchronous: false,
    };
    let descriptor = ProviderDescriptor {
        provider_id: "jit.matrix.provider".into(),
        provider_version: "1".into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        record_layouts: Vec::new(),
        variant_layouts: Vec::new(),
        functions: vec![ProviderFunctionDescriptor {
            symbol: symbol.clone(),
            signature: signature.clone(),
            entry: "adjust".into(),
            call_mode: ProviderCallMode::Sync,
            blocking: BlockingBehavior::NonBlocking,
            cancellation: CancellationBehavior::NotApplicable,
            thread_safe: true,
            reentrant: true,
            resource_cleanup: ResourceCleanupContract::None,
            error_mapping: ProviderErrorMapping::StructuredV1,
        }],
    };
    let calls = Arc::new(AtomicU64::new(0));
    let provider_calls = Arc::clone(&calls);
    let mut registry = ProviderRegistry::default();
    registry
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new(move |arguments| match arguments.as_slice() {
                        [WireValue::Int { value }] => {
                            provider_calls.fetch_add(1, Ordering::SeqCst);
                            Ok(WireValue::Int { value: value + 4 })
                        }
                        _ => Err(ProviderError::invalid_argument(
                            "adjust expects one Int argument",
                        )),
                    }),
                },
            )]),
        )
        .expect("matrix Provider must match its descriptor");
    (registry, calls)
}

fn stable_provider_traces(report: &ExecutionReport) -> Vec<serde_json::Value> {
    report
        .provider_call_traces
        .iter()
        .map(|trace| {
            serde_json::json!({
                "provider_id": trace.provider_id,
                "provider_version": trace.provider_version,
                "symbol": trace.symbol,
                "request_bytes": trace.request_bytes,
                "response_bytes": trace.response_bytes,
                "result": format!("{:?}", trace.result),
            })
        })
        .collect()
}

fn assert_semantic_report_matches(
    observed: &ExecutionReport,
    expected: &ExecutionReport,
    case: &ScorecardCase,
) {
    assert_eq!(
        observed.outcome(),
        expected.outcome(),
        "outcome: {}",
        case.name
    );
    assert_eq!(observed.stdout, expected.stdout, "stdout: {}", case.name);
    assert_eq!(observed.stderr, expected.stderr, "stderr: {}", case.name);
    assert_eq!(
        observed.diagnostics, expected.diagnostics,
        "diagnostics: {}",
        case.name
    );
    // Physical engine accounting is deliberately not normalized here: an
    // unbounded native region batches source steps and may eliminate source
    // allocations. Compare the stable semantic/report projection instead.
    assert_eq!(
        observed.usage.provider_calls, expected.usage.provider_calls,
        "Provider calls: {}",
        case.name
    );
    assert_eq!(
        observed.usage.resources_live_at_return, expected.usage.resources_live_at_return,
        "resource state: {}",
        case.name
    );
    assert_eq!(
        stable_provider_traces(observed),
        stable_provider_traces(expected),
        "stable Provider traces: {}",
        case.name
    );
    if case.workload == "provider" {
        assert_eq!(
            observed.usage.provider_calls, 1,
            "exactly-once Provider usage"
        );
        assert_eq!(
            observed.provider_call_traces.len(),
            1,
            "exactly-once Provider trace"
        );
    }
}

fn median(samples: &[Duration]) -> Duration {
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn median_absolute_deviation(samples: &[Duration], center: Duration) -> Duration {
    let deviations = samples
        .iter()
        .map(|sample| sample.abs_diff(center))
        .collect::<Vec<_>>();
    median(&deviations)
}

fn validate_and_print_engine_matrix(record: serde_json::Value) {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../benchmarks/vm-jit/aot-jit-matrix.schema.json"
    ))
    .expect("AOT/JIT matrix schema is valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("AOT/JIT matrix schema compiles");
    let errors = validator
        .iter_errors(&record)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "matrix schema errors: {errors:#?}");
    println!("AOT_JIT_MATRIX {record}");
}

/// Non-blocking full scorecard used by the weekly hardening workflow. The scalar
/// release smoke remains the small PR performance gate; this test produces the
/// evidence used to retain or prune complex passes.
#[test]
#[ignore = "full native-JIT performance scorecard"]
fn native_jit_pass_scorecard() {
    let case_filter = std::env::var("RSS_JIT_CASE").ok();
    let controlled = std::env::var("RSS_JIT_CONTROLLED").as_deref() == Ok("1");
    let samples = std::env::var("RSS_JIT_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .clamp(3, 100);
    let warmup = std::env::var("RSS_JIT_WARMUP")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 20);
    println!(
        "JIT_SCORECARD {}",
        serde_json::json!({
            "schema": "rsscript.native_jit_scorecard.v1",
            "profile": "release",
            "samples": samples,
            "warmup": warmup,
            "order": "alternating",
            "controlled": controlled,
        })
    );
    let mut cases_run = 0usize;
    for case in CASES {
        if case_filter
            .as_deref()
            .is_some_and(|filter| filter != case.name)
        {
            continue;
        }
        cases_run += 1;
        let built = match if case.workload == "provider" {
            Compiler.compile_with_interfaces(
                &[(case.name, case.source)],
                &[("matrix.rssi", PROVIDER_INTERFACE)],
            )
        } else {
            Compiler.compile(case.name, case.source)
        } {
            Ok(built) => built,
            Err(error) => {
                println!(
                    "JIT_SCORECARD {}",
                    serde_json::json!({
                        "case": case.name,
                        "pass": case.pass,
                        "size": case.size,
                        "status": "unsupported_by_canonical_compiler",
                        "reason": error.to_string(),
                    })
                );
                continue;
            }
        };
        let admitted = ArtifactVerifier
            .verify(built)
            .unwrap_or_else(|error| panic!("{} verifies: {error}", case.name))
            .admit_trusted_input();
        let (providers, provider_calls) = if case.workload == "provider" {
            let (registry, calls) = provider_registry();
            (registry, Some(calls))
        } else {
            (ProviderRegistry::default(), None)
        };
        let linked = Runtime::new(providers)
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{} links: {error}", case.name));
        let limits = RunLimits::unbounded_for_trusted_host();
        let interpreter_request = || {
            let request = ExecutionRequest::new([case.size]).limits(limits.clone());
            if case.workload == "provider" {
                request.trace(TracePolicy::MetadataOnly)
            } else {
                request
            }
        };
        let native_request = || interpreter_request().native_jit(NativeJitOptions::default());

        let expected = linked.execute(interpreter_request());
        let mut observed = linked.execute(native_request());
        assert_semantic_report_matches(&observed, &expected, case);

        for _ in 0..warmup {
            let report = linked.execute(interpreter_request());
            assert_semantic_report_matches(&report, &expected, case);
            observed = linked.execute(native_request());
            assert_semantic_report_matches(&observed, &expected, case);
        }
        let mut interpreter_samples = Vec::with_capacity(samples);
        let mut native_samples = Vec::with_capacity(samples);
        for sample in 0..samples {
            let mut run_interpreter = || {
                let started = Instant::now();
                let report = linked.execute(interpreter_request());
                interpreter_samples.push(started.elapsed());
                assert_semantic_report_matches(&report, &expected, case);
            };
            let mut run_native = || {
                let started = Instant::now();
                let report = linked.execute(native_request());
                native_samples.push(started.elapsed());
                assert_semantic_report_matches(&report, &expected, case);
            };
            if sample % 2 == 0 {
                run_interpreter();
                run_native();
            } else {
                run_native();
                run_interpreter();
            }
        }
        let interpreter = median(&interpreter_samples);
        let native = median(&native_samples);
        let interpreter_mad = median_absolute_deviation(&interpreter_samples, interpreter);
        let native_mad = median_absolute_deviation(&native_samples, native);
        let interpreter_samples_ns = interpreter_samples
            .iter()
            .map(Duration::as_nanos)
            .collect::<Vec<_>>();
        let native_samples_ns = native_samples
            .iter()
            .map(Duration::as_nanos)
            .collect::<Vec<_>>();
        // Timed native samples intentionally use production defaults with
        // telemetry disabled. Collect structural evidence in a separate run so
        // `Instant::now()` and counter traffic are not part of the cold E2E timing.
        let diagnostic_native =
            linked.execute(interpreter_request().native_jit(NativeJitOptions::diagnostic()));
        assert_semantic_report_matches(&diagnostic_native, &expected, case);
        let (
            native_calls,
            native_bails,
            osr_entries,
            continuation_entries,
            continuation_candidate_checks,
            continuation_full_probes,
            continuation_instance_key_builds,
            continuation_compiled_source_instructions,
            translation_nanos,
            validation_nanos,
            codegen_nanos,
            finalize_nanos,
            compile_nanos,
            diagnostic_native_run_nanos,
            resident_code_bytes,
            reserved_arena_bytes,
            runtime_helper_call_sites,
            readonly_licm_sites,
            bounds_check_sites,
            bounds_checks_elided,
        ) = match diagnostic_native.telemetry.engine {
            ExecutionEngineTelemetry::Interpreter => {
                (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
            }
            ExecutionEngineTelemetry::Native(engine) => (
                engine.native_calls,
                engine.native_bails,
                engine.osr_entries,
                engine.continuation_entries,
                engine.continuation_candidate_checks,
                engine.continuation_full_probes,
                engine.continuation_instance_key_builds,
                engine.continuation_compiled_source_instructions,
                engine.translation_nanos,
                engine.validation_nanos,
                engine.codegen_nanos,
                engine.finalize_nanos,
                engine.compile_nanos,
                engine.run_nanos,
                engine.resident_code_bytes,
                engine.reserved_arena_bytes,
                engine.runtime_helper_call_sites,
                engine.readonly_licm_sites,
                engine.direct_list_bounds_check_sites,
                engine.direct_list_bounds_checks_elided,
            ),
        };
        println!(
            "JIT_SCORECARD {}",
            serde_json::json!({
                "case": case.name,
                "pass": case.pass,
                "size": case.size,
                "status": if native_calls > 0 || osr_entries > 0 || continuation_entries > 0 { "entered" } else { "declined" },
                "interpreter_ns": interpreter.as_nanos(),
                "cold_e2e_native_ns": native.as_nanos(),
                "interpreter_samples_ns": interpreter_samples_ns,
                "cold_e2e_native_samples_ns": native_samples_ns,
                "interpreter_mad_ns": interpreter_mad.as_nanos(),
                "cold_e2e_native_mad_ns": native_mad.as_nanos(),
                "warm_native_instrumented_ns": diagnostic_native_run_nanos,
                "instrumented_native_nanos_per_entry": diagnostic_native_run_nanos
                    / u128::from(native_calls.saturating_add(osr_entries).saturating_add(continuation_entries).max(1)),
                "speedup": interpreter.as_secs_f64() / native.as_secs_f64(),
                "translation_nanos": translation_nanos,
                "validation_nanos": validation_nanos,
                "codegen_nanos": codegen_nanos,
                "finalize_nanos": finalize_nanos,
                "compile_nanos": compile_nanos,
                "resident_code_bytes": resident_code_bytes,
                "reserved_arena_bytes": reserved_arena_bytes,
                "native_calls": native_calls,
                "native_bails": native_bails,
                "osr_entries": osr_entries,
                "continuation_entries": continuation_entries,
                "continuation_candidate_checks": continuation_candidate_checks,
                "continuation_full_probes": continuation_full_probes,
                "continuation_instance_key_builds": continuation_instance_key_builds,
                "continuation_compiled_source_instructions": continuation_compiled_source_instructions,
                "runtime_helper_call_sites": runtime_helper_call_sites,
                "readonly_licm_sites": readonly_licm_sites,
                "bounds_check_sites": bounds_check_sites,
                "bounds_checks_elided": bounds_checks_elided,
                "semantic_match": true,
                "controlled": controlled,
                "retention_threshold_met": controlled
                    && native_bails == 0
                    && interpreter.as_secs_f64() / native.as_secs_f64() >= 1.15,
            })
        );
        let entered = native_calls > 0 || osr_entries > 0 || continuation_entries > 0;
        validate_and_print_engine_matrix(serde_json::json!({
            "schema": "rsscript.aot_jit_matrix.v1",
            "workload": case.workload,
            "semantic_match": true,
            "engines": {
                "interpreter": {
                    "status": "measured",
                    "execution_ns": interpreter.as_nanos(),
                    "compile_ns": 0,
                    "transitions": 0,
                    "host_helper_calls": null,
                    "bounds_checks": null,
                    "allocations_eliminated": null,
                    "reason": null
                },
                "jit": {
                    "status": if entered { "measured" } else { "declined" },
                    "execution_ns": native.as_nanos(),
                    "compile_ns": compile_nanos,
                    "transitions": native_calls
                        .saturating_add(osr_entries)
                        .saturating_add(continuation_entries),
                    "host_helper_calls": runtime_helper_call_sites,
                    "bounds_checks": bounds_check_sites.saturating_sub(bounds_checks_elided),
                    "allocations_eliminated": null,
                    "reason": if entered { None } else { Some("native tier declined this workload") }
                },
                "aot": {
                    "status": if case.workload == "provider" { "unsupported" } else { "not_measured" },
                    "execution_ns": null,
                    "compile_ns": null,
                    "transitions": null,
                    "host_helper_calls": null,
                    "bounds_checks": null,
                    "allocations_eliminated": null,
                    "reason": if case.workload == "provider" {
                        "the experimental AOT harness has no versioned Provider ABI binding"
                    } else {
                        "the experimental AOT backend is intentionally outside the Core SDK scorecard"
                    }
                }
            }
        }));
        if let Some(provider_calls) = provider_calls {
            let expected_runs = 2_u64
                .saturating_mul(
                    1_u64
                        .saturating_add(warmup as u64)
                        .saturating_add(samples as u64),
                )
                .saturating_add(1);
            assert_eq!(
                provider_calls.load(Ordering::SeqCst),
                expected_runs,
                "every interpreter/native matrix run must invoke the Provider exactly once"
            );
        }
    }
    assert!(
        cases_run > 0,
        "RSS_JIT_CASE did not match a canonical scorecard case"
    );
}
