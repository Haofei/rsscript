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
    size: &'static str,
    source: &'static str,
}

const CASES: &[ScorecardCase] = &[
    ScorecardCase {
        name: "scalar-loop",
        pass: "baseline",
        size: "200000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_scalar_loop.rss"),
    },
    ScorecardCase {
        name: "native-call-chain",
        pass: "inlining/native-call",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_call_chain.rss"),
    },
    ScorecardCase {
        name: "option-scalar-replacement",
        pass: "scalar-replacement",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_option_scalar_replace.rss"),
    },
    ScorecardCase {
        name: "result-scalar-replacement",
        pass: "scalar-replacement",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_result_scalar_replace.rss"),
    },
    ScorecardCase {
        name: "struct-scalar-replacement",
        pass: "scalar-replacement",
        size: "150000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/native_struct_scalar_replace.rss"),
    },
    ScorecardCase {
        name: "variant-scalar-replacement",
        pass: "scalar-replacement",
        size: "150000",
        source: include_str!(
            "../../../benchmarks/vm-jit/kernels/native_variant_scalar_replace.rss"
        ),
    },
    ScorecardCase {
        name: "profile-closure-pic",
        pass: "profile/PIC",
        size: "100000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/profile_closure_pic.rss"),
    },
    ScorecardCase {
        name: "profile-branch-side-exit",
        pass: "profile/side-exit",
        size: "100000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/profile_branch_side_exits.rss"),
    },
    ScorecardCase {
        name: "osr-scalar-loop",
        pass: "osr",
        size: "200000",
        source: include_str!("../../../benchmarks/vm-jit/kernels/osr_scalar_loop.rss"),
    },
];

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Non-blocking full scorecard used by the weekly hardening workflow. The scalar
/// release smoke remains the small PR performance gate; this test produces the
/// evidence used to retain or prune complex passes.
#[test]
#[ignore = "full native-JIT performance scorecard"]
fn native_jit_pass_scorecard() {
    println!(
        "JIT_SCORECARD {}",
        serde_json::json!({
            "schema": "rsscript.native_jit_scorecard.v1",
            "profile": "release",
            "samples": 5,
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
        let observed = linked.execute(native_request());
        assert_eq!(observed.outcome(), expected.outcome(), "{}", case.name);
        assert_eq!(observed.stdout, expected.stdout, "{}", case.name);

        let mut interpreter_samples = Vec::with_capacity(5);
        let mut native_samples = Vec::with_capacity(5);
        let mut latest_native = observed;
        for _ in 0..5 {
            let started = Instant::now();
            let report = linked.execute(interpreter_request());
            interpreter_samples.push(started.elapsed());
            assert_eq!(report.outcome(), expected.outcome(), "{}", case.name);

            let started = Instant::now();
            latest_native = linked.execute(native_request());
            native_samples.push(started.elapsed());
            assert_eq!(latest_native.outcome(), expected.outcome(), "{}", case.name);
        }
        let interpreter = median(interpreter_samples);
        let native = median(native_samples);
        let (
            native_calls,
            native_bails,
            osr_entries,
            compile_nanos,
            resident_code_bytes,
            reserved_arena_bytes,
        ) = match latest_native.telemetry.engine {
            ExecutionEngineTelemetry::Interpreter => (0, 0, 0, 0, 0, 0),
            ExecutionEngineTelemetry::Native {
                native_calls,
                native_bails,
                osr_entries,
                compile_nanos,
                resident_code_bytes,
                reserved_arena_bytes,
                ..
            } => (
                native_calls,
                native_bails,
                osr_entries,
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
                "status": if native_calls > 0 || osr_entries > 0 { "entered" } else { "declined" },
                "interpreter_ns": interpreter.as_nanos(),
                "native_ns": native.as_nanos(),
                "speedup": interpreter.as_secs_f64() / native.as_secs_f64(),
                "compile_nanos": compile_nanos,
                "resident_code_bytes": resident_code_bytes,
                "reserved_arena_bytes": reserved_arena_bytes,
                "native_calls": native_calls,
                "native_bails": native_bails,
                "osr_entries": osr_entries,
            })
        );
    }
}
