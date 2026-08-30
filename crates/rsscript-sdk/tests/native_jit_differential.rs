use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    experimental::native_jit::{NativeCostModel, NativeJitOptions},
    operation::{CancellationToken, MonotonicDeadline},
    provider_api::{
        BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
        ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
        ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, ProviderRegistry,
        RUNTIME_ABI_VERSION, ResourceCleanupContract, WireInterpreterFn, WireValue,
    },
    report::{ExecutionEngineTelemetry, ExecutionReport, TerminationReason},
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};

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
        "direct-scalar-call.rss",
        "fn wide(value: Int) -> Int { let a01 = value ^ 1; let a02 = a01 ^ 2; let a03 = a02 ^ 3; let a04 = a03 ^ 4; let a05 = a04 ^ 5; let a06 = a05 ^ 6; let a07 = a06 ^ 7; let a08 = a07 ^ 8; let a09 = a08 ^ 9; let a10 = a09 ^ 10; let a11 = a10 ^ 11; let a12 = a11 ^ 12; let a13 = a12 ^ 13; let a14 = a13 ^ 14; let a15 = a14 ^ 15; let a16 = a15 ^ 16; let a17 = a16 ^ 17; let a18 = a17 ^ 18; let a19 = a18 ^ 19; let a20 = a19 ^ 20; let a21 = a20 ^ 21; let a22 = a21 ^ 22; let a23 = a22 ^ 23; let a24 = a23 ^ 24; let a25 = a24 ^ 25; return a25 } fn main() -> Int { let mut i = 0; let mut total = 0; while i < 2000 { total = wide(value: total); i = i + 1 }; return total }",
    ),
    (
        "static-inline-call.rss",
        "fn small(value: Int) -> Int { return value * value } fn worker(value: Int) -> Int { let a = small(value: value % 97); let b = a + 1; let c = b + 2; let d = c + 3; let e = d + 4; let f = e + 5; return f } fn main() -> Int { let mut i = 0; let mut total = 0; while i < 2000 { total = total + worker(value: i); i = i + 1 }; return total }",
    ),
    (
        "generic-static-instances.rss",
        "fn identity<T>(value: read T) -> T { return value } fn main() -> Int { let ignored = identity<Float>(value: read 1.5); let mut i = 0; while i < 2000 { i = identity<Int>(value: read i + 1) }; return i }",
    ),
    (
        "call-continuation.rss",
        "struct Boxed { value: Int } fn boundary(value: Int) -> Int { let boxed = Boxed(value: value); return boxed.value } fn main() -> Int { let a = 7; let b = a * 3; let c = b + 11; let p = c * 2; let q = p - 5; let r = q + 9; let s = r * 2; let d = boundary(value: s); let e = d * 5; let f = e - 9; let g = f + 2; let h = g * 3; let i = h - 4; let j = i + 6; let k = j * 2; return k }",
    ),
    (
        "branch-continuation.rss",
        "struct BoxedBranch { value: Int } fn branch_boundary(value: Int) -> Int { let boxed = BoxedBranch(value: value); return boxed.value } fn choose(flag: Bool) -> Int { let a = 7; let b = a * 3; let c = b + 11; let p = c * 2; let q = p - 5; let r = q + 9; let s = r * 2; if flag { let d = branch_boundary(value: s); let e = d * 5; let f = e - 9; let g = f + 2; let h = g * 3; let i = h - 4; let j = i + 6; let k = j * 2; return k } else { let h = s * 2; let i = h + 5; let j = i - 1; let k = j * 3; let l = k - 4; let m = l + 6; let n = m * 2; return n } } fn main() -> Int { let left = choose(flag: true); let right = choose(flag: false); return left + right }",
    ),
    (
        "aggregate-continuation.rss",
        "struct AggregateBox { value: Int } fn main() -> Int { let boxed = AggregateBox(value: 13); let extracted = boxed.value; let a = extracted * 3; let b = a + 11; let c = b * 5; let d = c - 9; let e = d + 2; let f = e * 3; let g = f - 4; let h = g + 6; let i = h * 2; return i }",
    ),
    (
        "readonly-helper-continuation.rss",
        "struct ReadBox { value: Int } fn boundary() -> Int { let boxed = ReadBox(value: 1); return boxed.value } fn hot(text: String, limit: Int) -> Int { let seed = boundary(); let mut i = 0; let mut total = seed; while i < limit { total = total + String.len(value: text); i = i + 1 }; return total } fn main() -> Int { return hot(text: \"rsscript\", limit: 2000) }",
    ),
    (
        "await-continuation.rss",
        "async fn boundary(value: Int) -> Int { return value + 4 } async fn main() -> Int { let a = 7; let b = a * 3; let c = b + 11; let p = c * 2; let q = p - 5; let r = q + 9; let s = r * 2; task_group { async let pending = boundary(value: s); let d = await pending; let e = d * 5; let f = e - 9; let g = f + 2; let h = g * 3; let i = h - 4; let j = i + 6; let k = j * 2; return k } }",
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
            .unwrap_or_else(|error| panic!("corpus artifact verifies for {file}: {error:?}"))
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
            cost_model: NativeCostModel::Report,
            collect_telemetry: true,
            // Exercise whole-function call lowering for the two ABI/inlining
            // canaries; the rest of the corpus keeps production OSR enabled.
            enable_auto_osr: false,
            eager_osr: !matches!(*file, "direct-scalar-call.rss" | "static-inline-call.rss"),
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
            continuation_candidate_checks,
            continuation_full_probes,
            continuation_instance_key_builds,
            continuation_yields,
            continuation_compiled_source_instructions,
            interpreted_native_work,
            native_barrier_counts,
            rejected_resident_bytes,
            ..
        } = native.telemetry.engine
        else {
            panic!("native telemetry: {file}");
        };
        assert_eq!(rejected_resident_bytes, 0, "resident rejection: {file}");
        assert!(
            continuation_full_probes <= continuation_candidate_checks,
            "full continuation preparation must be candidate-gated: {file}"
        );
        assert!(
            continuation_instance_key_builds <= continuation_full_probes,
            "instance keys must be built only inside full continuation probes: {file}"
        );
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
                native.usage.steps_consumed,
                interpreter.usage.steps_consumed
            );
            assert_eq!(
                native_calls, 0,
                "whole-function JIT must decline at the call barrier"
            );
            assert_eq!(osr_entries, 0, "the straight-line case must not use OSR");
            assert!(
                continuation_entries >= 2 && continuation_yields >= 2,
                "both sides of the interpreted call must execute as native continuations; entries={continuation_entries}, yields={continuation_yields}, barriers={native_barrier_counts:?}"
            );
        }
        if *file == "branch-continuation.rss" {
            assert_eq!(
                native.usage.steps_consumed,
                interpreter.usage.steps_consumed
            );
            assert!(
                continuation_entries >= 2 && continuation_yields >= 2,
                "a branched region and its post-call continuation must both enter; entries={continuation_entries}, yields={continuation_yields}, barriers={native_barrier_counts:?}"
            );
        }
        if *file == "aggregate-continuation.rss" {
            assert_eq!(
                native.usage.steps_consumed,
                interpreter.usage.steps_consumed
            );
            assert!(
                continuation_entries >= 1 && continuation_yields >= 1,
                "scalar work after aggregate materialization must re-enter native code; entries={continuation_entries}, yields={continuation_yields}, barriers={native_barrier_counts:?}, missed={interpreted_native_work}"
            );
        }
        if *file == "readonly-helper-continuation.rss" {
            assert!(
                continuation_entries >= 1 && continuation_yields >= 1,
                "read-only scalar-result helpers must remain inside a continuation region; entries={continuation_entries}, yields={continuation_yields}, compiled_work={continuation_compiled_source_instructions}, barriers={native_barrier_counts:?}"
            );
            assert!(
                !native_barrier_counts.contains_key("unsupported_intrinsic"),
                "read-only scalar-result helpers must not force a VM barrier"
            );
        }
        if *file == "await-continuation.rss" {
            assert!(
                continuation_entries >= 2 && continuation_yields >= 2,
                "scalar work around await must use native continuations; entries={continuation_entries}, yields={continuation_yields}, barriers={native_barrier_counts:?}"
            );
            assert!(
                native_barrier_counts
                    .get("await")
                    .is_some_and(|count| *count >= 1),
                "await must remain a VM-owned barrier"
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

#[test]
fn tiered_whole_function_with_backedge_starts_optimized() {
    let source = "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 20000 { total = total + i * 3; i = i + 1 }; return total }";
    let built = Compiler
        .compile("tiered-backedge.rss", source)
        .expect("tiered backedge source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("tiered backedge artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("tiered backedge artifact links");
    let interpreter =
        linked.execute(ExecutionRequest::default().limits(RunLimits::unbounded_for_trusted_host()));
    let native = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host())
            .native_jit(NativeJitOptions {
                tier_up_threshold: 1,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(native.outcome(), interpreter.outcome());
    let ExecutionEngineTelemetry::Native {
        baseline_compiles,
        optimized_compiles,
        baseline_calls,
        optimized_calls,
        ..
    } = native.telemetry.engine
    else {
        panic!("tiered native execution must return engine telemetry");
    };
    assert_eq!(
        baseline_compiles, 0,
        "backedge body must skip baseline codegen"
    );
    assert!(
        optimized_compiles > 0,
        "backedge body must compile at speed"
    );
    assert_eq!(
        baseline_calls, 0,
        "backedge body must not enter baseline code"
    );
    assert!(
        optimized_calls > 0,
        "backedge body must execute optimized code"
    );
}

#[test]
fn automatic_osr_waits_enters_at_threshold_and_respects_disable() {
    let source = "struct Boxed { value: Int } fn main() -> Int { let boxed = Boxed(value: 9); let mut i = 0; let mut total = 0; while i < 2000 { total = total + i * 3; i = i + 1 }; return total + boxed.value }";
    let built = Compiler
        .compile("auto-osr-option.rss", source)
        .expect("automatic OSR source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("automatic OSR artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("automatic OSR artifact links");
    let below_threshold = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host())
            .native_jit(NativeJitOptions {
                enable_auto_osr: true,
                eager_osr: false,
                osr_work_threshold: u32::MAX,
                cost_model: NativeCostModel::Report,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    let ExecutionEngineTelemetry::Native {
        osr_entries: below_entries,
        ..
    } = below_threshold.telemetry.engine
    else {
        panic!("below-threshold OSR execution must return native telemetry");
    };
    assert_eq!(below_entries, 0, "automatic OSR must wait below threshold");

    let disabled = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host())
            .native_jit(NativeJitOptions {
                enable_auto_osr: false,
                eager_osr: false,
                cost_model: NativeCostModel::Report,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    let ExecutionEngineTelemetry::Native {
        osr_entries: disabled_entries,
        ..
    } = disabled.telemetry.engine
    else {
        panic!("disabled OSR execution must return native telemetry");
    };
    assert_eq!(disabled_entries, 0, "disabled OSR must never enter");

    let native = linked.execute(
        ExecutionRequest::new(["2000"])
            .limits(RunLimits::unbounded_for_trusted_host())
            .native_jit(NativeJitOptions {
                enable_auto_osr: true,
                eager_osr: false,
                osr_work_threshold: 64,
                cost_model: NativeCostModel::Report,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    let ExecutionEngineTelemetry::Native {
        osr_entries,
        continuation_entries,
        ..
    } = native.telemetry.engine
    else {
        panic!("automatic OSR execution must return native telemetry");
    };
    assert!(
        osr_entries > 0,
        "threshold-driven OSR must enter a transform-only loop; continuation_entries={continuation_entries}"
    );
}

#[test]
fn bounded_step_accounting_matches_across_call_continuations() {
    let source = "struct StepBox { value: Int } fn boundary(value: Int) -> Int { let boxed = StepBox(value: value); return boxed.value } fn main() -> Int { let a = 7; let b = a * 3; let c = b + 11; let p = c * 2; let q = p - 5; let r = q + 9; let s = r * 2; let d = boundary(value: s); let e = d * 5; let f = e - 9; let g = f + 2; let h = g * 3; let i = h - 4; let j = i + 6; let k = j * 2; return k }";
    let built = Compiler
        .compile("bounded-continuation.rss", source)
        .expect("source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("artifact links");
    let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(1_000);
    let interpreter = linked.execute(ExecutionRequest::default().limits(limits.clone()));
    let native = linked.execute(ExecutionRequest::default().limits(limits).native_jit(
        NativeJitOptions {
            cost_model: NativeCostModel::Off,
            collect_telemetry: true,
            ..NativeJitOptions::default()
        },
    ));
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(
        native.usage.steps_consumed,
        interpreter.usage.steps_consumed
    );
    let ExecutionEngineTelemetry::Native {
        continuation_entries,
        ..
    } = native.telemetry.engine
    else {
        panic!("bounded native execution must report native telemetry");
    };
    assert!(continuation_entries >= 2);

    let bounded_interpreter =
        linked.execute(ExecutionRequest::default().limits(RunLimits::bounded()));
    let bounded_native = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded())
            .native_jit(NativeJitOptions {
                cost_model: NativeCostModel::Report,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(bounded_native.outcome(), bounded_interpreter.outcome());
    assert_eq!(
        bounded_native.usage.steps_consumed,
        bounded_interpreter.usage.steps_consumed
    );
    let ExecutionEngineTelemetry::Native {
        continuation_entries,
        ..
    } = bounded_native.telemetry.engine
    else {
        panic!("default bounded execution must report native telemetry");
    };
    assert!(continuation_entries >= 2);

    let deadline_native = linked.execute(
        ExecutionRequest::default()
            .limits(
                RunLimits::bounded()
                    .with_deadline(MonotonicDeadline::after(Duration::from_secs(60))),
            )
            .native_jit(NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(deadline_native.outcome(), interpreter.outcome());
    let ExecutionEngineTelemetry::Native {
        continuation_entries,
        ..
    } = deadline_native.telemetry.engine
    else {
        panic!("deadline-armed execution must report native telemetry");
    };
    assert!(continuation_entries >= 2);

    let cancel = CancellationToken::new();
    let cancel_armed = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host().with_cancellation(cancel))
            .native_jit(NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(cancel_armed.outcome(), interpreter.outcome());
    let ExecutionEngineTelemetry::Native {
        continuation_entries,
        ..
    } = cancel_armed.telemetry.engine
    else {
        panic!("cancel-armed native execution must report native telemetry");
    };
    assert!(continuation_entries >= 2);
}

#[test]
fn native_memory_controls_admit_proved_scalar_work_and_account_osr_growth() {
    let scalar = Compiler
        .compile(
            "native-memory-scalar.rss",
            "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 20000 { total = total + i; i = i + 1 }; return total }",
        )
        .expect("scalar source compiles");
    let scalar = ArtifactVerifier
        .verify(scalar)
        .expect("scalar artifact verifies")
        .admit_trusted_input();
    let scalar = Runtime::new(ProviderRegistry::default())
        .link(&scalar)
        .expect("scalar artifact links");
    let scalar_limits = RunLimits::unbounded_for_trusted_host()
        .with_allocation_budget(0)
        .with_live_memory_limit(1024 * 1024);
    let interpreter = scalar.execute(ExecutionRequest::default().limits(scalar_limits.clone()));
    let native = scalar.execute(
        ExecutionRequest::default()
            .limits(scalar_limits)
            .native_jit(NativeJitOptions {
                collect_telemetry: true,
                cost_model: NativeCostModel::Off,
                enable_auto_osr: false,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(native.usage.allocation_bytes_consumed, 0);
    assert_eq!(
        native.usage.live_memory_bytes_at_return,
        interpreter.usage.live_memory_bytes_at_return
    );
    assert_eq!(
        native.usage.peak_live_memory_bytes,
        interpreter.usage.peak_live_memory_bytes
    );
    let ExecutionEngineTelemetry::Native { native_calls, .. } = native.telemetry.engine else {
        panic!("memory-controlled scalar execution must report native telemetry");
    };
    assert!(
        native_calls > 0,
        "proved no-allocation scalar work should enter native"
    );

    let growing = Compiler
        .compile(
            "native-memory-osr.rss",
            "fn main() -> Int { local values = List<Int>.new(); let mut i = 0; while i < 512 { List.push<Int>(list: mut values, value: i); i = i + 1 }; return List.len<Int>(list: values) }",
        )
        .expect("growing-list source compiles");
    let growing = ArtifactVerifier
        .verify(growing)
        .expect("growing-list artifact verifies")
        .admit_trusted_input();
    let growing = Runtime::new(ProviderRegistry::default())
        .link(&growing)
        .expect("growing-list artifact links");
    let reference = growing.execute(
        ExecutionRequest::default().limits(
            RunLimits::unbounded_for_trusted_host()
                .with_allocation_budget(1024 * 1024)
                .with_live_memory_limit(1024 * 1024),
        ),
    );
    assert_eq!(reference.termination_reason(), TerminationReason::Completed);
    let sufficient = RunLimits::unbounded_for_trusted_host()
        .with_allocation_budget(reference.usage.allocation_bytes_consumed)
        .with_live_memory_limit(reference.usage.peak_live_memory_bytes);
    let native = growing.execute(
        ExecutionRequest::default()
            .limits(sufficient.clone())
            .native_jit(NativeJitOptions {
                collect_telemetry: true,
                cost_model: NativeCostModel::Off,
                eager_osr: true,
                ..NativeJitOptions::default()
            }),
    );
    let interpreter = growing.execute(ExecutionRequest::default().limits(sufficient));
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(
        native.usage.allocation_bytes_consumed,
        interpreter.usage.allocation_bytes_consumed
    );
    assert_eq!(
        native.usage.live_memory_bytes_at_return,
        interpreter.usage.live_memory_bytes_at_return
    );
    assert_eq!(
        native.usage.peak_live_memory_bytes,
        interpreter.usage.peak_live_memory_bytes
    );
    let ExecutionEngineTelemetry::Native { osr_entries, .. } = native.telemetry.engine else {
        panic!("memory-controlled OSR execution must report native telemetry");
    };
    assert!(osr_entries > 0, "accounted List.push loop should enter OSR");

    let insufficient = RunLimits::unbounded_for_trusted_host()
        .with_allocation_budget(reference.usage.allocation_bytes_consumed.saturating_sub(1))
        .with_live_memory_limit(1024 * 1024);
    let interpreter = growing.execute(ExecutionRequest::default().limits(insufficient.clone()));
    let native = growing.execute(ExecutionRequest::default().limits(insufficient).native_jit(
        NativeJitOptions {
            collect_telemetry: true,
            cost_model: NativeCostModel::Off,
            eager_osr: true,
            ..NativeJitOptions::default()
        },
    ));
    assert_eq!(
        native.termination_reason(),
        interpreter.termination_reason()
    );
    assert_eq!(
        native.usage.allocation_bytes_consumed,
        interpreter.usage.allocation_bytes_consumed
    );
}

#[test]
fn continuation_controls_fail_before_codegen_and_match_every_step_boundary() {
    let source = "struct StepGate { value: Int } fn boundary(value: Int) -> Int { let boxed = StepGate(value: value); return boxed.value } fn main() -> Int { let a = 7; let b = a * 3; let c = b + 11; let p = c * 2; let q = p - 5; let r = q + 9; let s = r * 2; let d = boundary(value: s); let e = d * 5; let f = e - 9; let g = f + 2; let h = g * 3; let i = h - 4; let j = i + 6; let k = j * 2; return k }";
    let built = Compiler
        .compile("continuation-controls.rss", source)
        .expect("source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("artifact links");
    let completed = linked.execute(ExecutionRequest::default());

    for budget in 0..=completed.usage.steps_consumed.saturating_add(1) {
        let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
        let interpreter = linked.execute(ExecutionRequest::default().limits(limits.clone()));
        let native = linked.execute(ExecutionRequest::default().limits(limits).native_jit(
            NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            },
        ));
        assert_eq!(
            native.termination_reason(),
            interpreter.termination_reason()
        );
        assert_eq!(
            native.usage.steps_consumed,
            interpreter.usage.steps_consumed
        );
    }

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let report = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host().with_cancellation(cancelled))
            .native_jit(NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(report.termination_reason(), TerminationReason::Cancelled);
    let ExecutionEngineTelemetry::Native {
        continuation_compiled_source_instructions,
        ..
    } = report.telemetry.engine
    else {
        panic!("cancelled native execution must report native telemetry");
    };
    assert_eq!(continuation_compiled_source_instructions, 0);

    let report =
        linked.execute(
            ExecutionRequest::default()
                .limits(RunLimits::unbounded_for_trusted_host().with_deadline(
                    MonotonicDeadline::at(Instant::now() - Duration::from_millis(1)),
                ))
                .native_jit(NativeJitOptions {
                    cost_model: NativeCostModel::Off,
                    collect_telemetry: true,
                    ..NativeJitOptions::default()
                }),
        );
    assert_eq!(
        report.termination_reason(),
        TerminationReason::DeadlineExceeded
    );
    let ExecutionEngineTelemetry::Native {
        continuation_compiled_source_instructions,
        ..
    } = report.telemetry.engine
    else {
        panic!("expired native execution must report native telemetry");
    };
    assert_eq!(continuation_compiled_source_instructions, 0);
}

#[test]
fn cancellation_during_a_closed_native_region_is_observed() {
    let source = "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 2000000000 { total = total + i; i = i + 1 }; return total }";
    let built = Compiler
        .compile("continuation-cancel-mid-region.rss", source)
        .expect("source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("artifact links");

    let token = CancellationToken::new();
    let trigger = token.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        trigger.cancel();
    });
    let report = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host().with_cancellation(token))
            .native_jit(NativeJitOptions::default().with_telemetry()),
    );
    canceller.join().expect("cancellation thread completes");
    assert_eq!(report.termination_reason(), TerminationReason::Cancelled);
    let ExecutionEngineTelemetry::Native {
        compiled,
        continuation_compiled_source_instructions,
        ..
    } = report.telemetry.engine
    else {
        panic!("mid-region cancellation must report native telemetry");
    };
    assert!(
        compiled > 0 || continuation_compiled_source_instructions > 0,
        "whole-function and continuation regions share the bounded native path"
    );
}

#[test]
fn provider_barrier_executes_once_and_reenters_native() {
    const SOURCE: &str = "module app\nuse host.math.*\nfn main() -> Int { let a = 7; let b = a * 3; let c = b + 11; let p = c * 2; let q = p - 5; let r = q + 9; let s = r * 2; let d = adjust(value: read s); let e = d * 5; let f = e - 9; let g = f + 2; let h = g * 3; let i = h - 4; let j = i + 6; let k = j * 2; return k }";
    const INTERFACE: &str = "module host.math\npub fn adjust(value: read Int) -> Int\n";

    let symbol = ExternalSymbol::new("host.math.adjust").expect("test symbol is valid");
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
        provider_id: "jit.test.math".into(),
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
    let mut providers = ProviderRegistry::default();
    providers
        .register(
            &descriptor,
            BTreeMap::from([(
                symbol,
                ProviderFunction {
                    signature,
                    callable: WireInterpreterFn::new(move |args| match args.as_slice() {
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
        .expect("test Provider matches its descriptor");

    let built = Compiler
        .compile_with_interfaces(&[("main.rss", SOURCE)], &[("math.rssi", INTERFACE)])
        .expect("provider continuation source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("provider continuation artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(providers)
        .link(&admitted)
        .expect("test Provider links");

    let interpreter = linked.execute(ExecutionRequest::default().trace(TracePolicy::MetadataOnly));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let native = linked.execute(
        ExecutionRequest::default()
            .trace(TracePolicy::MetadataOnly)
            .native_jit(NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(native.stdout, interpreter.stdout);
    assert_eq!(native.stderr, interpreter.stderr);
    assert_eq!(native.diagnostics, interpreter.diagnostics);
    assert_eq!(
        native.usage.steps_consumed,
        interpreter.usage.steps_consumed
    );
    assert_eq!(native.usage.provider_calls, 1);
    assert_eq!(interpreter.usage.provider_calls, 1);
    assert_eq!(native.provider_call_traces.len(), 1);
    assert_eq!(interpreter.provider_call_traces.len(), 1);
    assert_eq!(
        stable_provider_traces(&native),
        stable_provider_traces(&interpreter),
        "mixed-mode execution must preserve every stable Provider trace field; only call_id and elapsed are run-local"
    );
    let ExecutionEngineTelemetry::Native {
        continuation_entries,
        continuation_yields,
        native_barrier_counts,
        ..
    } = native.telemetry.engine
    else {
        panic!("Provider continuation must report native telemetry");
    };
    assert!(continuation_entries >= 2);
    assert!(continuation_yields >= 2);
    assert!(
        native_barrier_counts
            .get("external_call")
            .is_some_and(|count| *count >= 1)
    );
}
