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
use rsscript_vm::NativeExecutionEngineTelemetry;

fn native_telemetry(report: &ExecutionReport) -> &NativeExecutionEngineTelemetry {
    let ExecutionEngineTelemetry::Native(telemetry) = &report.telemetry.engine else {
        panic!("execution must report native telemetry");
    };
    telemetry
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
        let &NativeExecutionEngineTelemetry {
            native_calls,
            osr_entries,
            continuation_entries,
            continuation_candidate_checks,
            continuation_full_probes,
            continuation_instance_key_builds,
            continuation_yields,
            continuation_compiled_source_instructions,
            interpreted_native_work,
            ref native_barrier_counts,
            rejected_resident_bytes,
            ..
        } = native_telemetry(&native);
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
    let &NativeExecutionEngineTelemetry {
        baseline_compiles,
        optimized_compiles,
        baseline_calls,
        optimized_calls,
        ..
    } = native_telemetry(&native);
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
    let &NativeExecutionEngineTelemetry {
        osr_entries: below_entries,
        ..
    } = native_telemetry(&below_threshold);
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
    let &NativeExecutionEngineTelemetry {
        osr_entries: disabled_entries,
        ..
    } = native_telemetry(&disabled);
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
    let &NativeExecutionEngineTelemetry {
        osr_entries,
        continuation_entries,
        ..
    } = native_telemetry(&native);
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
    let &NativeExecutionEngineTelemetry {
        continuation_entries,
        ..
    } = native_telemetry(&native);
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
    let &NativeExecutionEngineTelemetry {
        continuation_entries,
        ..
    } = native_telemetry(&bounded_native);
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
    let &NativeExecutionEngineTelemetry {
        continuation_entries,
        ..
    } = native_telemetry(&deadline_native);
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
    let &NativeExecutionEngineTelemetry {
        continuation_entries,
        ..
    } = native_telemetry(&cancel_armed);
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
    let native_calls = native_telemetry(&native).native_calls;
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
    let osr_entries = native_telemetry(&native).osr_entries;
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
    let &NativeExecutionEngineTelemetry {
        continuation_compiled_source_instructions,
        ..
    } = native_telemetry(&report);
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
    let &NativeExecutionEngineTelemetry {
        continuation_compiled_source_instructions,
        ..
    } = native_telemetry(&report);
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
    let &NativeExecutionEngineTelemetry {
        compiled,
        continuation_compiled_source_instructions,
        ..
    } = native_telemetry(&report);
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
    let &NativeExecutionEngineTelemetry {
        continuation_entries,
        continuation_yields,
        ref native_barrier_counts,
        ..
    } = native_telemetry(&native);
    assert!(continuation_entries >= 2);
    assert!(continuation_yields >= 2);
    assert!(
        native_barrier_counts
            .get("external_call")
            .is_some_and(|count| *count >= 1)
    );
}

/// One interpreter/native pair for the same program and the same armed limits.
///
/// The interpreter is the accounting oracle: `usage.steps_consumed` and the
/// termination reason it reports are what native execution must reproduce.
fn accounting_pair(
    name: &str,
    source: &str,
    limits: RunLimits,
    options: NativeJitOptions,
) -> (ExecutionReport, ExecutionReport) {
    let built = Compiler.compile(name, source).expect("source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("artifact links");
    let interpreter = linked.execute(ExecutionRequest::default().limits(limits.clone()));
    let native = linked.execute(
        ExecutionRequest::default()
            .limits(limits)
            .native_jit(options),
    );
    (interpreter, native)
}

fn native_region_entries(report: &ExecutionReport) -> u64 {
    let telemetry = native_telemetry(report);
    telemetry
        .native_calls
        .saturating_add(telemetry.osr_entries)
        .saturating_add(telemetry.continuation_entries)
}

/// Program shapes whose natively executed regions must account source steps
/// exactly. `native_under_limits` records whether the shape can still reach
/// generated code once a preemption control is armed: a region whose cost cannot
/// be attributed exactly declines to the interpreter instead of under-reporting,
/// and that decline is itself part of the contract.
///
/// The shapes whose callee owns a loop are deliberately too large for the leaf
/// inliner (`controlled_static_inline_candidate` refuses a callee containing a
/// backedge), so they can only reach generated code through a compiled
/// native-to-native call edge. Pinning `native_under_limits: true` for them is
/// therefore a pin on that edge being built and metered under an armed control.
struct StepParityCase {
    name: &'static str,
    source: &'static str,
    native_under_limits: bool,
}

const STEP_PARITY_CASES: &[StepParityCase] = &[
    StepParityCase {
        name: "call-free-loop.rss",
        source: "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 3000 { total = total + i * 3 - i / 2; i = i + 1 }; return total }",
        native_under_limits: true,
    },
    // The callee owns the loop, so it is too big to dissolve through the leaf
    // inliner and the caller reaches it over a real native-to-native call edge.
    // The callee is compiled with the caller's controls and charges its 57k source
    // steps against the caller's limits cell, so the edge now runs natively under
    // an armed budget instead of declining.
    StepParityCase {
        name: "callee-owns-the-loop.rss",
        source: "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < 3000 { total = total + i * 3 - i / 2; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 3000) }",
        native_under_limits: true,
    },
    // A two-deep compiled-callee chain: `main` -> `outer` -> `inner`, with the
    // innermost frame owning the loop. Each frame flushes its running count to the
    // shared cell before its edge and adopts the callee's count on return, so the
    // whole chain reports one interpreter-equivalent step stream.
    StepParityCase {
        name: "nested-compiled-callee.rss",
        source: "fn inner(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2; i = i + 1 }; return total } fn outer(limit: Int) -> Int { let mut r = 0; let mut sum = 0; while r < 3 { sum = sum + inner(limit: limit); r = r + 1 }; return sum } fn main() -> Int { return outer(limit: 400) }",
        native_under_limits: true,
    },
    // A guard deopts *inside* the callee reached over the edge. The interpreter
    // re-executes the caller's whole call instruction, so the region must report
    // the count as of the instruction before the call — the callee's own charge
    // is rolled back by the caller's bail write-back. The region is entered and
    // metered, but the overflow is fatal on both engines, so it never *completes*
    // natively and contributes no completed-region entry.
    StepParityCase {
        name: "deopt-inside-compiled-callee.rss",
        source: "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 1; while i < limit { total = total * 3 + 1; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 2000) }",
        native_under_limits: false,
    },
    // Repeated whole-function native entry: each call's own region is metered and
    // the interpreter carries the count across entries.
    StepParityCase {
        name: "repeated-native-entry.rss",
        source: "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2; i = i + 1 }; return total } fn main() -> Int { let mut out = 0; let mut r = 0; while r < 50 { out = hot(limit: 200); r = r + 1 }; return out }",
        native_under_limits: true,
    },
    // A leaf call inside the hot loop: the inliner dissolves it, so every spliced
    // callee instruction owns its own interpreter step.
    StepParityCase {
        name: "inlined-leaf-in-loop.rss",
        source: "fn square(v: Int) -> Int { return v * v } fn main() -> Int { let mut i = 0; let mut total = 0; while i < 3000 { total = total + square(v: i % 97); i = i + 1 }; return total }",
        native_under_limits: true,
    },
];

/// Step budgets chosen to land before, inside and past each shape's native
/// regions, including values that fall exactly on a region entry/exit boundary
/// (the reported counts for these programs are 101, 1001, 10001 and their
/// completion totals), so an off-by-one in segment reservation or deopt roll-back
/// changes the reported count.
const STEP_PARITY_BUDGETS: &[u64] = &[
    1, 2, 3, 12, 99, 100, 101, 102, 511, 512, 513, 1_000, 1_001, 1_002, 10_000, 57_011, 57_012,
    57_013, 10_000_000,
];

#[test]
fn native_step_accounting_matches_the_interpreter_under_an_armed_budget() {
    for case in STEP_PARITY_CASES {
        let mut native_regions = 0_u64;
        for &budget in STEP_PARITY_BUDGETS {
            let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
            let (interpreter, native) = accounting_pair(
                case.name,
                case.source,
                limits,
                NativeJitOptions {
                    cost_model: NativeCostModel::Off,
                    collect_telemetry: true,
                    ..NativeJitOptions::default()
                },
            );
            assert_eq!(
                native.outcome(),
                interpreter.outcome(),
                "{} at step budget {budget} must terminate for the same reason as the interpreter",
                case.name
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{} at step budget {budget} must report the interpreter's step count",
                case.name
            );
            native_regions = native_regions.saturating_add(native_region_entries(&native));
        }
        assert_eq!(
            native_regions > 0,
            case.native_under_limits,
            "{} native engagement under an armed step budget changed",
            case.name
        );
    }
}

#[test]
fn an_unbounded_native_run_reports_the_interpreter_step_count() {
    // `steps_consumed` is a reported fact, not only a ceiling. With nothing armed
    // a natively executed whole-function region used to report zero while the
    // interpreter reported the true count, so source-step accounting is now on for
    // every whole-function entry and the limits cell simply carries `i64::MAX` as
    // its budget.
    //
    // Run with the production tiering defaults, automatic OSR included, because an
    // unbounded hot loop reaches generated code mostly through OSR rather than
    // whole-function entry: before OSR was armed, `call-free-loop.rss` reported
    // 729 of the interpreter's 57012 steps.
    for case in STEP_PARITY_CASES {
        let (interpreter, native) = accounting_pair(
            case.name,
            case.source,
            RunLimits::unbounded_for_trusted_host(),
            NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            },
        );
        assert_eq!(
            native.outcome(),
            interpreter.outcome(),
            "{} must terminate for the same reason as the interpreter with no limit armed",
            case.name
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "{} must report the interpreter's step count with no limit armed",
            case.name
        );
        assert!(
            native_region_entries(&native) > 0 || native_telemetry(&native).native_bails > 0,
            "{} must actually reach generated code for the count above to mean anything",
            case.name
        );
    }
}

#[test]
fn native_step_accounting_matches_the_interpreter_for_osr_entered_loops() {
    for case in STEP_PARITY_CASES {
        for &budget in &[100_u64, 1_000, 10_000, 10_000_000] {
            let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
            let (interpreter, native) = accounting_pair(
                case.name,
                case.source,
                limits,
                NativeJitOptions {
                    cost_model: NativeCostModel::Off,
                    eager_osr: true,
                    collect_telemetry: true,
                    ..NativeJitOptions::default()
                },
            );
            assert_eq!(
                native.outcome(),
                interpreter.outcome(),
                "{} under eager OSR at step budget {budget} must match the interpreter outcome",
                case.name
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{} under eager OSR at step budget {budget} must report the interpreter's steps",
                case.name
            );
        }
    }
}

#[test]
fn native_step_accounting_is_exact_when_a_guard_deopts() {
    // The interpreter charges a step *before* executing an instruction, so the
    // failing multiply is counted. Generated code reserves its segment up front,
    // so the count it reports on a guard bail must exclude the instruction the
    // interpreter is about to re-execute.
    let cases: &[(&str, &str, bool)] = &[
        (
            "guard-deopt-inline-free.rss",
            "fn main() -> Int { let mut i = 0; let mut total = 1; while i < 200 { total = total * 3 + 1; i = i + 1 }; return total }",
            false,
        ),
        (
            "guard-deopt-through-callee.rss",
            "fn step(v: Int) -> Int { return v * 3 + 1 } fn main() -> Int { let mut i = 0; let mut total = 1; while i < 200 { total = step(v: total); i = i + 1 }; return total }",
            false,
        ),
        // The overflowing loop lives in a callee reached over a compiled
        // native-to-native edge, so the guard bails inside the child frame. The
        // caller's bail write-back must roll the child's charge back to the count
        // as of the call instruction the interpreter re-executes.
        (
            "guard-deopt-inside-compiled-callee.rss",
            "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 1; while i < limit { total = total * 3 + 1; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 2000) }",
            true,
        ),
    ];
    for (name, source, expect_native_bail) in cases {
        for eager_osr in [false, true] {
            let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(10_000);
            let (interpreter, native) = accounting_pair(
                name,
                source,
                limits,
                NativeJitOptions {
                    cost_model: NativeCostModel::Off,
                    eager_osr,
                    collect_telemetry: true,
                    ..NativeJitOptions::default()
                },
            );
            assert_eq!(
                native.outcome(),
                interpreter.outcome(),
                "{name} (eager_osr={eager_osr}) must report the same overflow failure"
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{name} (eager_osr={eager_osr}) must report the interpreter's step count"
            );
            if *expect_native_bail {
                // The loop lives behind a compiled native-to-native edge, so the
                // only way this shape can bail out of generated code is by running
                // it first. Pinning the bail keeps the case from silently degrading
                // into "the region declined and the interpreter did everything",
                // which would make the step equality above vacuous.
                assert!(
                    native_telemetry(&native).native_bails > 0,
                    "{name} (eager_osr={eager_osr}) must actually enter and bail out of generated code"
                );
            }
        }
    }
}

#[test]
fn cancellation_and_deadline_stop_native_execution_with_the_interpreter_reason() {
    let source = "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 100000000 { total = total + i * 3 - i / 2; i = i + 1 }; return total }";
    let options = NativeJitOptions {
        cost_model: NativeCostModel::Off,
        collect_telemetry: true,
        ..NativeJitOptions::default()
    };

    let deadline = RunLimits::unbounded_for_trusted_host()
        .with_deadline(MonotonicDeadline::after(Duration::from_millis(50)));
    let (interpreter, native) = accounting_pair("native-deadline.rss", source, deadline, options);
    assert_eq!(
        native.outcome(),
        interpreter.outcome(),
        "a deadline reached inside a native region must report the interpreter's failure"
    );
    assert!(
        native.usage.steps_consumed > 0,
        "a deadline bail must still report the source steps the native region paid for"
    );

    let cancel = CancellationToken::new();
    let watchdog = cancel.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        watchdog.cancel();
    });
    let cancelled = RunLimits::unbounded_for_trusted_host().with_cancellation(cancel);
    let (interpreter, native) = accounting_pair("native-cancel.rss", source, cancelled, options);
    canceller.join().expect("watchdog thread joins");
    assert_eq!(
        native.outcome(),
        interpreter.outcome(),
        "a cancellation observed inside a native region must report the interpreter's failure"
    );
    assert!(
        native.usage.steps_consumed > 0,
        "a cancellation bail must still report the source steps the native region paid for"
    );
}

/// Every native kernel in the differential corpus must report the interpreter's
/// step count once a step budget is armed.
///
/// This is the regression guard for the whole source-cost model rather than for
/// one shape: an inlined callee body, a native-to-native call edge, a guard
/// deopt, or an interpreter charge that is *not* one step per instruction (the
/// `Int` map-key hash `RegVm::charge_work` bills) each show up here as a drifting
/// count on a workload the engine is expected to accelerate.
#[test]
fn native_kernel_corpus_reports_the_interpreter_step_count_under_a_budget() {
    let mut drift = Vec::new();
    for (name, source) in CASES {
        for &budget in &[1_000_u64, 100_000, 100_000_000] {
            let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
            let (interpreter, native) = accounting_pair(
                name,
                source,
                limits,
                NativeJitOptions {
                    cost_model: NativeCostModel::Off,
                    collect_telemetry: true,
                    ..NativeJitOptions::default()
                },
            );
            if interpreter.usage.steps_consumed != native.usage.steps_consumed
                || interpreter.outcome() != native.outcome()
            {
                drift.push(format!(
                    "{name} at step budget {budget}: interpreter {} {:?} vs native {} {:?} ({} native regions)",
                    interpreter.usage.steps_consumed,
                    interpreter.outcome(),
                    native.usage.steps_consumed,
                    native.outcome(),
                    native_region_entries(&native),
                ));
            }
        }
    }
    assert!(
        drift.is_empty(),
        "native step accounting drifted from the interpreter:\n{}",
        drift.join("\n")
    );
}
