use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::ProviderRegistry,
    report::ExecutionEngineTelemetry,
    runtime::{ExecutionRequest, NativeJitOptions, RunLimits, Runtime},
};
use std::time::{Duration, Instant};

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
];

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
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
        })
    );
    for case in CASES {
        let built = match Compiler.compile(case.name, case.source) {
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
        let linked = Runtime::new(ProviderRegistry::default())
            .link(&admitted)
            .unwrap_or_else(|error| panic!("{} links: {error}", case.name));
        let limits = RunLimits::unbounded_for_trusted_host();
        let interpreter_request = || ExecutionRequest::new([case.size]).limits(limits.clone());
        let native_request = || interpreter_request().native_jit(NativeJitOptions::default());

        let expected = linked.execute(interpreter_request());
        let mut observed = linked.execute(native_request());
        assert_eq!(observed.outcome(), expected.outcome(), "{}", case.name);
        assert_eq!(observed.stdout, expected.stdout, "{}", case.name);

        for _ in 0..warmup {
            let _ = linked.execute(interpreter_request());
            observed = linked.execute(native_request());
        }
        let mut interpreter_samples = Vec::with_capacity(samples);
        let mut native_samples = Vec::with_capacity(samples);
        let mut latest_native = observed;
        for sample in 0..samples {
            let mut run_interpreter = || {
                let started = Instant::now();
                let report = linked.execute(interpreter_request());
                interpreter_samples.push(started.elapsed());
                assert_eq!(report.outcome(), expected.outcome(), "{}", case.name);
            };
            let mut run_native = || {
                let started = Instant::now();
                latest_native = linked.execute(native_request());
                native_samples.push(started.elapsed());
                assert_eq!(latest_native.outcome(), expected.outcome(), "{}", case.name);
            };
            if sample % 2 == 0 {
                run_interpreter();
                run_native();
            } else {
                run_native();
                run_interpreter();
            }
        }
        let interpreter = median(interpreter_samples);
        let native = median(native_samples);
        let (
            native_calls,
            native_bails,
            osr_entries,
            continuation_entries,
            continuation_compiled_source_instructions,
            compile_nanos,
            resident_code_bytes,
            reserved_arena_bytes,
        ) = match latest_native.telemetry.engine {
            ExecutionEngineTelemetry::Interpreter => (0, 0, 0, 0, 0, 0, 0, 0),
            ExecutionEngineTelemetry::Native {
                native_calls,
                native_bails,
                osr_entries,
                continuation_entries,
                continuation_compiled_source_instructions,
                compile_nanos,
                resident_code_bytes,
                reserved_arena_bytes,
                ..
            } => (
                native_calls,
                native_bails,
                osr_entries,
                continuation_entries,
                continuation_compiled_source_instructions,
                compile_nanos,
                resident_code_bytes,
                reserved_arena_bytes,
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
                "native_ns": native.as_nanos(),
                "speedup": interpreter.as_secs_f64() / native.as_secs_f64(),
                "compile_nanos": compile_nanos,
                "resident_code_bytes": resident_code_bytes,
                "reserved_arena_bytes": reserved_arena_bytes,
                "native_calls": native_calls,
                "native_bails": native_bails,
                "osr_entries": osr_entries,
                "continuation_entries": continuation_entries,
                "continuation_compiled_source_instructions": continuation_compiled_source_instructions,
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
                    "host_helper_calls": null,
                    "bounds_checks": null,
                    "allocations_eliminated": null,
                    "reason": if entered { None } else { Some("native tier declined this workload") }
                },
                "aot": {
                    "status": "not_measured",
                    "execution_ns": null,
                    "compile_ns": null,
                    "transitions": null,
                    "host_helper_calls": null,
                    "bounds_checks": null,
                    "allocations_eliminated": null,
                    "reason": "the experimental AOT backend is intentionally outside the Core SDK scorecard"
                }
            }
        }));
    }
}
