use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    provider_api::ProviderRegistry,
    report::ExecutionEngineTelemetry,
    runtime::{ExecutionRequest, NativeCostModel, NativeJitOptions, RunLimits, Runtime},
};

const CASES: &[(&str, &str)] = &[
    (
        "arithmetic.rss",
        "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 10000 { total = total + i * 3 - i / 2; i = i + 1 }; return total }",
    ),
    (
        "branches.rss",
        "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 5000 { if i % 2 == 0 { total = total + i } else { total = total - 1 }; i = i + 1 }; return total }",
    ),
    (
        "calls.rss",
        "fn square(value: Int) -> Int { return value * value } fn main() -> Int { let mut i = 0; let mut total = 0; while i < 2000 { total = total + square(value: i % 97); i = i + 1 }; return total }",
    ),
    (
        "call-continuation.rss",
        "struct Boxed { value: Int } fn boundary(value: Int) -> Int { let boxed = Boxed(value: value); return boxed.value } fn main() -> Int { let a = 7; let b = a * 3; let c = b + 11; let d = boundary(value: c); let e = d * 5; let f = e - 9; let g = f + 2; return g }",
    ),
    (
        "native-list-write.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_list_write_loop.rss"),
    ),
    (
        "native-map-match.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_map_get_match_loop.rss"),
    ),
    (
        "native-option.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_option_scalar_replace.rss"),
    ),
    (
        "native-result.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_result_scalar_replace.rss"),
    ),
    (
        "native-struct.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_struct_scalar_replace.rss"),
    ),
    (
        "native-variant.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_variant_scalar_replace.rss"),
    ),
    (
        "native-string.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_string_concat_len.rss"),
    ),
    (
        "native-bytes.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_bytes_slice_len_loop.rss"),
    ),
    (
        "native-osr.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/osr_scalar_loop.rss"),
    ),
];

#[test]
fn native_engine_matches_the_verified_interpreter_corpus() {
    let mut cases_with_native_entry = 0usize;
    for (file, source) in CASES {
        let built = Compiler
            .compile(file, source)
            .expect("corpus source compiles");
        let admitted = ArtifactVerifier
            .verify(built)
            .expect("corpus artifact verifies")
            .admit_trusted_input();
        let linked = Runtime::new(ProviderRegistry::default())
            .link(&admitted)
            .expect("corpus artifact links");

        let limits = RunLimits::unbounded_for_trusted_host();
        // Benchmark kernels accept their loop bound as argv[0]. A small explicit
        // value keeps this correctness gate fast while preserving the same IR
        // shapes as the full performance corpus.
        let request = || ExecutionRequest::new(["2000"]).limits(limits.clone());
        let interpreter = linked.execute(request());
        let native = linked.execute(request().native_jit(NativeJitOptions {
            cost_model: NativeCostModel::Off,
            ..NativeJitOptions::default()
        }));

        assert_eq!(native.outcome(), interpreter.outcome(), "outcome: {file}");
        assert_eq!(native.stdout, interpreter.stdout, "stdout: {file}");
        assert_eq!(native.stderr, interpreter.stderr, "stderr: {file}");
        assert_eq!(
            native.provider_call_traces, interpreter.provider_call_traces,
            "provider trace: {file}"
        );
        assert_eq!(
            native.usage.resources_live_at_return, interpreter.usage.resources_live_at_return,
            "resource state: {file}"
        );
        assert_eq!(
            native.usage.resource_cleanup_failures, interpreter.usage.resource_cleanup_failures,
            "resource cleanup: {file}"
        );
        let ExecutionEngineTelemetry::Native {
            native_calls,
            osr_entries,
            continuation_entries,
            continuation_yields,
            interpreted_native_work,
            native_barrier_counts,
            rejected_resident_bytes,
            ..
        } = native.telemetry.engine
        else {
            panic!("native telemetry: {file}");
        };
        assert_eq!(rejected_resident_bytes, 0, "resident rejection: {file}");
        if *file == "native-struct.rss" {
            assert_eq!(
                (native_calls, osr_entries),
                (0, 0),
                "struct scalar replacement must remain outside the stable native-jit path"
            );
            assert!(
                interpreted_native_work > 0,
                "missed-work telemetry must expose native-capable work around barriers"
            );
            assert!(
                native_barrier_counts
                    .get("aggregate_operation")
                    .is_some_and(|count| *count > 0),
                "aggregate barriers must be reported structurally"
            );
        }
        if *file == "call-continuation.rss" {
            assert_eq!(
                native_calls, 0,
                "whole-function JIT must decline at the call barrier"
            );
            assert_eq!(osr_entries, 0, "the straight-line case must not use OSR");
            assert!(
                continuation_entries >= 2 && continuation_yields >= 2,
                "both sides of the interpreted call must execute as native continuations"
            );
        }
        cases_with_native_entry +=
            usize::from(native_calls > 0 || osr_entries > 0 || continuation_entries > 0);
    }
    assert!(
        cases_with_native_entry >= 6,
        "differential corpus must exercise native execution broadly; only {cases_with_native_entry} cases entered"
    );
}
