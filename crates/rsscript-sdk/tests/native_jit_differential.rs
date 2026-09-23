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
    // The `xs[i] = value` spelling of `List.set`: index assignment lowers to
    // the same `ListSet` operation the namespaced call does, so generated code
    // must reproduce the interpreter's element writes and bounds behaviour.
    (
        "index-assignment-loop.rss",
        "fn hot(limit: Int, values: mut List<Int>) -> Int { let mut i = 0; let mut total = 0; while i < limit { let slot = i % 8; values[slot] = i; total = total + values[slot]; i = i + 1 }; return total } fn main() -> Int { let mut values: List<Int> = [0, 0, 0, 0, 0, 0, 0, 0]; return hot(limit: 20000, values: mut values) }",
    ),
    // The receiver-call spelling of a mutating core intrinsic. Lowering
    // normalizes the receiver into parameter zero before an intrinsic is
    // chosen, so this must reach exactly the `ListPush`/`ListGet` native
    // lowering the namespaced spelling reaches.
    (
        "receiver-call-loop.rss",
        "fn hot(limit: Int) -> Int { let mut values = List<Int>.new(); let mut i = 0; let mut total = 0; while i < limit { mut values.push(i); total = total + values.len(); i = i + 1 }; return total } fn main() -> Int { return hot(limit: 2000) }",
    ),
    // A list pattern's length test and element projections are `ListLen`,
    // `ListGet` and integer compares, all of which are in the native subset.
    (
        "list-pattern-loop.rss",
        "fn classify(xs: read List<Int>) -> Int { match read xs { [] => { return 0 } [0, _] => { return 1 } [a, b] => { return a + b } _ => { return 9 } } } fn main() -> Int { let pair: List<Int> = [3, 4]; let zero: List<Int> = [0, 4]; let long: List<Int> = [1, 2, 3]; let mut i = 0; let mut total = 0; while i < 5000 { total = total + classify(xs: read pair) + classify(xs: read zero) + classify(xs: read long); i = i + 1 }; return total }",
    ),
    // A guarded `match` arm lowers to its pattern's tests followed by the
    // guard's compare and a plain `Branch` whose false edge is the next arm's
    // test, so a false guard's fall-through is the only new control shape.
    (
        "guarded-match-loop.rss",
        "fn classify(xs: read List<Int>) -> Int { match read xs { [a, b] if a > b => { return a - b } [a, b] => { return a + b } [first, ..] if first == 0 => { return 7 } _ => { return 9 } } } fn scale(n: Int) -> Int { match n { 0 => { return 1 } _ if n % 3 == 0 => { return 3 } _ => { return n } } } fn main() -> Int { let desc: List<Int> = [9, 4]; let asc: List<Int> = [1, 2]; let zero: List<Int> = [0, 1, 2]; let long: List<Int> = [5, 1, 2]; let mut i = 0; let mut total = 0; while i < 5000 { total = total + classify(xs: read desc) + classify(xs: read asc) + classify(xs: read zero) + classify(xs: read long) + scale(n: i % 5); i = i + 1 }; return total }",
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
    // A `local` closure allocated and called inside the hot loop. Until the
    // typed facts stopped publishing the checker's unresolved marker as a
    // proved parameter type, this kernel failed Artifact verification before it
    // could run on either engine, so it could not be in this corpus at all.
    (
        "native-closure-sinking.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/native_closure_sinking.rss"),
    ),
    // A `Bool` match whose scrutinee is a comparison, plus an `if` expression
    // used for a value. Before `HirExpr::Binary` carried its result type the
    // checker rejected both shapes, so neither could be in this corpus at all.
    // Whole-function native lowering still declines both — an `if` used for a
    // value lowers to a `match`, and a `match` is outside the stable native
    // subset, unlike the `if` *statement* in `branches.rss` — so today this is
    // a parity gate over the interpreted arms and the native continuations
    // around them, and it starts covering generated arm selection for free when
    // the subset grows.
    (
        "bool-match-comparison.rss",
        "fn classify(a: Int, b: Int) -> Int { match a < b { true => { return 1 } false => { return 0 } } } fn pick(n: Int) -> Int { let value = if n % 3 == 0 { 5 } else { 7 }; return value } fn main() -> Int { let mut i = 0; let mut total = 0; while i < 5000 { total = total + classify(a: i % 7, b: 3) + pick(n: i); i = i + 1 }; return total }",
    ),
    // Labelled arguments written out of declaration order, with an omitted
    // defaulted parameter declared between two supplied ones (ADR 0244).
    // Lowering places each argument by the parameter it names, so the call
    // instruction generated code sees is already in declaration order; this
    // pins that the native engine consumes that order exactly as the
    // interpreter does. `labelled_argument_order_runs_natively_with_the_declared_binding`
    // below also pins the value both must compute.
    ("labelled-argument-order.rss", LABELLED_ARGUMENT_ORDER),
];

const LABELLED_ARGUMENT_ORDER: &str = "fn mix(left: Int, middle: Int = 7, right: Int) -> Int { return left * 100 + middle * 10 + right } fn sub(left: Int, right: Int) -> Int { return left - right } fn main() -> Int { let mut i = 0; let mut total = 0; while i < 2000 { total = total + mix(right: i % 10, left: 3) - mix(left: 3, right: i % 10) + mix(right: 1, middle: 2, left: 0) + sub(right: 3, left: 10); i = i + 1 }; return total }";

#[test]
fn labelled_argument_order_runs_natively_with_the_declared_binding() {
    let built = Compiler
        .compile("labelled-argument-order.rss", LABELLED_ARGUMENT_ORDER)
        .expect("labelled argument source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("labelled argument artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("labelled argument artifact links");
    let limits = RunLimits::unbounded_for_trusted_host();
    let interpreter = linked.execute(ExecutionRequest::default().limits(limits.clone()));
    let native = linked.execute(ExecutionRequest::default().limits(limits).native_jit(
        NativeJitOptions {
            cost_model: NativeCostModel::Off,
            collect_telemetry: true,
            ..NativeJitOptions::default()
        },
    ));
    // Each iteration adds `mix(0, 2, 1) = 21` and `sub(10, 3) = 7`; the two
    // `mix` calls with the default cancel. A binding by written order would
    // give `sub(3, 10) = -7` and shift `right` into `middle`.
    let expected = WireValue::Int { value: 2000 * 28 };
    assert_eq!(interpreter.wire_value(), Some(&expected));
    assert_eq!(native.wire_value(), Some(&expected));
    assert!(
        native_telemetry(&native).compiled > 0,
        "the hot loop runs as generated code"
    );
}

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
        if *file == "guarded-match-loop.rss" {
            // `main`, `classify`, and `scale` each compile: a guarded arm is a
            // compare and a `Branch`, both inside the native subset, so the
            // guard runs in generated code rather than forcing a barrier.
            assert!(
                native_telemetry(&native).compiled >= 3 && osr_entries > 0,
                "guarded match arms must reach generated code; compiled={}, osr={osr_entries}, barriers={native_barrier_counts:?}",
                native_telemetry(&native).compiled
            );
            assert!(
                !native_barrier_counts.contains_key("static_call"),
                "calls into guarded matches must not be barriers: {native_barrier_counts:?}"
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

#[test]
fn an_armed_provider_call_budget_no_longer_refuses_native_dispatch() {
    // A Provider call is `RegInstr::CallExternal`, a barrier generated code never
    // lowers, so the interpreter performs and charges every one of them. An armed
    // `provider_call_budget` therefore has no reason to refuse native dispatch, and
    // the native run must report the interpreter's counts and its termination
    // reason both under and over the budget.
    const SOURCE: &str = "module app\nuse host.math.*\nfn work(seed: Int) -> Int { let mut i = 0; let mut total = seed; while i < 400 { total = total + i * 3 - i / 2; i = i + 1 }; return total }\nfn main() -> Int { let mut round = 0; let mut acc = 1; while round < 4 { let w = work(seed: acc); acc = adjust(value: read w); round = round + 1 }; return acc }";
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

    // Two budgets: one the program stays under, one it trips partway through.
    for (budget, expect_native) in [(8_u64, true), (2, true)] {
        let mut providers = ProviderRegistry::default();
        providers
            .register(
                &descriptor,
                BTreeMap::from([(
                    symbol.clone(),
                    ProviderFunction {
                        signature: signature.clone(),
                        callable: WireInterpreterFn::new(|args| match args.as_slice() {
                            [WireValue::Int { value }] => Ok(WireValue::Int {
                                value: value % 1_000 + 4,
                            }),
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
            .expect("provider budget source compiles");
        let admitted = ArtifactVerifier
            .verify(built)
            .expect("provider budget artifact verifies")
            .admit_trusted_input();
        let linked = Runtime::new(providers)
            .link(&admitted)
            .expect("test Provider links");

        let limits = RunLimits::unbounded_for_trusted_host().with_provider_call_budget(budget);
        let interpreter = linked.execute(ExecutionRequest::default().limits(limits.clone()));
        let native = linked.execute(ExecutionRequest::default().limits(limits).native_jit(
            NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            },
        ));

        assert_eq!(
            native.outcome(),
            interpreter.outcome(),
            "provider budget {budget} must terminate for the interpreter's reason"
        );
        assert_eq!(
            native.usage.provider_calls, interpreter.usage.provider_calls,
            "provider budget {budget} must report the interpreter's Provider call count"
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "provider budget {budget} must report the interpreter's step count"
        );
        if expect_native {
            // Continuation regions were already reachable under an armed Provider
            // budget; whole-function and OSR dispatch were the refused ones, so
            // pin those specifically or this would pass without the change.
            let telemetry = native_telemetry(&native);
            assert!(
                telemetry.native_calls + telemetry.osr_entries > 0,
                "provider budget {budget} must no longer refuse whole-function or OSR dispatch"
            );
        }
    }
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

fn accounting_pair_with_args(
    name: &str,
    source: &str,
    args: &[&str],
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
    let interpreter =
        linked.execute(ExecutionRequest::new(args.iter().copied()).limits(limits.clone()));
    let native = linked.execute(
        ExecutionRequest::new(args.iter().copied())
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

/// OSR loops whose exact source cost the OSR pass chain has to compose.
///
/// Each shape puts its loop in a function that also writes output, so
/// whole-function native entry declines and the loop can reach generated code
/// only through OSR. Between them they exercise every hop the chain has to get
/// right: a call the leaf inliner dissolves into the loop (which used to make
/// `RegVm::build_osr_plan` decline the region outright), the string length-law
/// fold, which replaces the source instruction it folds, and the Option/Result
/// combinator expansion, which turns one interpreter `CallIntrinsic` into several
/// items *before* the inliner runs and whose scalar-replacement passes then
/// dissolve the Option/Result it built.
///
/// The last two also pin the bug the composition fixes rather than merely an
/// optimization it unlocks: before the chain carried a cost vector, the
/// combinator shape ran natively and reported 51053 of the interpreter's 54014
/// steps and 41 of its 3002 intrinsic calls.
const OSR_INLINE_PARITY_CASES: &[(&str, &str)] = &[
    (
        "osr-inlined-leaf-call.rss",
        "fn square(v: Int) -> Int { return v * v } fn main() -> Unit { let mut i = 0; let mut total = 0; while i < 3000 { total = total + square(v: i % 97); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
    (
        "osr-inlined-call-string-fold.rss",
        "fn bump(v: Int) -> Int { return v + 1 } fn main() -> Unit { let mut i = 0; let mut total = 0; while i < 3000 { total = total + String.len(value: String.concat(left: \"ab\", right: \"cde\")) + bump(v: i); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
    (
        "osr-option-combinator-sr.rss",
        "fn main() -> Unit { let mut i = 0; let mut total = 0; while i < 3000 { total = total + Option.unwrap_or<Int>(value: Some(i * 2), default: 0); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
    (
        "osr-result-combinator-sr.rss",
        "fn checked(v: Int) -> Result<Int, String> { return Ok(v * 2) } fn main() -> Unit { let mut i = 0; let mut total = 0; while i < 3000 { total = total + Result.unwrap_or<Int, String>(value: checked(v: i), default: 0); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
];

/// Loops whose body holds internal conditional control flow, which MIR lays out
/// with the loop's own exit block *between* two of the loop's blocks.
///
/// Loop recognition read a loop as the contiguous interval `[header, exit)`, so
/// `detect_natural_loops` returned **no loop at all** for these: no region was
/// generated, `RegVm::try_osr` was never reached, and the interpreter owned every
/// step. `native_normalize_osr_loop_layout` relocates the exit block past the
/// loop before the OSR pass chain runs. That is a pure permutation of the
/// function's instructions — none added, removed or duplicated, only branch
/// targets remapped — so each source instruction still owns exactly one item and
/// exactly one interpreter step, which is what the budgets below check.
const OSR_MATCH_LAYOUT_PARITY_CASES: &[(&str, &str)] = &[
    // The kernel the native-jit contract's gap list named, at a test-sized
    // iteration count through its own `bench_size` argument.
    (
        "osr_option_loop.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/osr_option_loop.rss"),
    ),
    // The `Result` sibling: a hand-written `match` over a `Result` a dissolvable
    // leaf call returns, so the layout normalization, the leaf inliner and the
    // Result scalar replacement all have to compose on one region.
    (
        "osr-result-match-loop.rss",
        "fn checked(v: Int) -> Result<Int, String> { return Ok(v * 2) } fn hot(limit: Int) -> Int { Output.write(message: \"begin\"); let mut i = 0; let mut total = 0; while i < limit { match checked(v: i) { Ok(value) => { total = total + value } Err(_) => { total = total + 1 } }; i = i + 1 }; Output.write(message: String.from_int(value: total)); return total } fn main(args: read List<String>) -> Unit { let limit = Arguments.get_or_default(args: read args, index: 0, default: \"3000\"); match String.parse_int(value: limit) { Some(value) => { Output.write(message: String.from_int(value: hot(limit: value))) } None => { Output.write(message: \"bad\") } }; return Unit }",
    ),
    // A `match` over an `Option` that is *not* dissolvable into a scalar: the
    // payload is a `String` the loop keeps, so Option scalar replacement declines
    // and the region is the interpreter's. Recognition must still be exact.
    (
        "osr-option-match-heap-payload.rss",
        "fn hot(limit: Int) -> Int { Output.write(message: \"begin\"); let mut i = 0; let mut total = 0; while i < limit { let mut o: Option<String> = None; if i % 3 == 0 { o = Some(\"abc\") }; match o { Some(text) => { total = total + String.len(value: text) } None => { total = total + 1 } }; i = i + 1 }; Output.write(message: String.from_int(value: total)); return total } fn main(args: read List<String>) -> Unit { Output.write(message: String.from_int(value: hot(limit: 3000))); return Unit }",
    ),
];

/// The two hand-written kernels the contract's gap list grouped with
/// `osr_option_loop.rss`, which the layout normalization does **not** unblock.
/// Their loops are contiguous and always were recognized; each is refused
/// further down the pipeline, for a reason that has nothing to do with loop
/// shape. They are pinned as declines so that a change which does unblock one
/// has to say so in the contract's gap list rather than pass silently.
const OSR_MATCH_LAYOUT_DECLINED_KERNELS: &[(&str, &str, &str)] = &[
    // `p.x` on a locally built struct lowers to `GetField` keyed by *name*, which
    // is an aggregate barrier; the loop-local struct pass dissolves only
    // `MakeStruct` + `GetFieldSlot`, and a slot-keyed read exists solely in the
    // typed-region lowering the direct path uses.
    (
        "osr_struct_loop.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/osr_struct_loop.rss"),
        "a name-keyed GetField is not dissolvable by the loop-local struct pass",
    ),
    // The closure is a *parameter*, so there is no `MakeClosure` to sink and no
    // static callee; naming one would need the profile-guided closure PIC that
    // was removed after failing its retention threshold.
    (
        "osr_closure_loop.rss",
        include_str!("../../../benchmarks/vm-jit/kernels/osr_closure_loop.rss"),
        "a closure parameter has no statically named callee to sink",
    ),
];

/// The evidence gap 3 in the native-jit contract rests on: the callee frame's
/// register-window growth is charged against a **high-water mark**, not per call.
///
/// `RegVm::ensure_regs` bills `grew * (size_of::<VmValue>() + 1)` where `grew`
/// is measured against the shared register stack's current length, and nothing
/// in the VM ever truncates that stack. The two programs below differ only in
/// whether `hot`'s result is computed a second time or copied, so a per-call-site
/// constant would charge the second call's window too. It does not: both report
/// the same allocation bytes. That is why the charge cannot be reserved as a
/// compile-time constant at a native call edge, on either engine.
#[test]
fn the_register_window_charge_is_a_high_water_mark_not_a_per_call_constant() {
    const TWO_CALLS: &str = "fn hot(n: Int) -> Int { let mut i = 0; let mut t = 0; while i < n { t = t + i; i = i + 1 }; return t } fn main() -> Unit { let x = hot(n: 10); let y = hot(n: 10); Output.write(message: String.from_int(value: x)); Output.write(message: String.from_int(value: y)); return Unit }";
    const ONE_CALL: &str = "fn hot(n: Int) -> Int { let mut i = 0; let mut t = 0; while i < n { t = t + i; i = i + 1 }; return t } fn main() -> Unit { let x = hot(n: 10); let y = x; Output.write(message: String.from_int(value: x)); Output.write(message: String.from_int(value: y)); return Unit }";
    let limits = RunLimits::unbounded_for_trusted_host();
    let options = NativeJitOptions {
        cost_model: NativeCostModel::Off,
        collect_telemetry: true,
        ..NativeJitOptions::default()
    };
    let (two_interpreter, two_native) =
        accounting_pair("two-calls.rss", TWO_CALLS, limits.clone(), options);
    let (one_interpreter, one_native) = accounting_pair("one-call.rss", ONE_CALL, limits, options);
    assert_eq!(
        two_interpreter.usage.allocation_bytes_consumed,
        one_interpreter.usage.allocation_bytes_consumed,
        "a second call at the same depth raises no high-water mark, so it charges no \
         register-window bytes; a per-call-site constant would have charged twice"
    );
    for (interpreter, native, name) in [
        (&two_interpreter, &two_native, "two-calls.rss"),
        (&one_interpreter, &one_native, "one-call.rss"),
    ] {
        assert_eq!(
            native.usage.allocation_bytes_consumed, interpreter.usage.allocation_bytes_consumed,
            "{name} must report the interpreter's allocation bytes under native execution"
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "{name} must report the interpreter's step count"
        );
    }
}

/// A map key the loop *builds*.
///
/// `String.concat` produces a live heap `String` — the string length-law fold
/// exists to dissolve such a value, not to keep one — so the OSR lowering had no
/// arm for it and the whole loop stayed on the interpreter with
/// `lower reject: StringConcat`. The arm lowers it through the same host helper
/// the whole-function translator uses, so the loop reaches generated code and
/// the key's hash work is charged by the helper that hashes it.
///
/// The third case keeps the built key live *past* the loop and varies its length
/// per iteration, so a stale value would change the program's output: it pins
/// that the clean OSR exit materializes the handle live-out from the heap table
/// rather than leaving the interpreter's slot behind.
const OSR_BUILT_KEY_CASES: &[(&str, &str)] = &[
    // The key is consumed by the map insert and dead afterwards.
    (
        "osr-built-map-key.rss",
        "fn hot(limit: Int, prefix: String) -> Int { Output.write(message: \"begin\"); let mut table = Map<String, Int>.new(); let mut i = 0; while i < limit { let key = String.concat(left: prefix, right: String.from_int(value: i % 8)); Map.insert(map: mut table, key: key, value: i); i = i + 1 }; let n = Map.len(map: read table); Output.write(message: String.from_int(value: n)); return n } fn main() -> Unit { Output.write(message: String.from_int(value: hot(limit: 3000, prefix: \"k\"))); return Unit }",
    ),
    // The same key in a set, which hashes through the other helper.
    (
        "osr-built-set-key.rss",
        "fn hot(limit: Int, prefix: String) -> Int { Output.write(message: \"begin\"); let mut seen = Set<String>.new(); let mut i = 0; while i < limit { let key = String.concat(left: prefix, right: String.from_int(value: i % 8)); Set.insert(set: mut seen, value: key); i = i + 1 }; let n = Set.len(set: read seen); Output.write(message: String.from_int(value: n)); return n } fn main() -> Unit { Output.write(message: String.from_int(value: hot(limit: 3000, prefix: \"k\"))); return Unit }",
    ),
    // The built key survives the backedge and is read after the loop.
    (
        "osr-built-key-live-after-the-loop.rss",
        "fn hot(limit: Int, prefix: String) -> Int { Output.write(message: \"begin\"); let mut table = Map<String, Int>.new(); let mut i = 0; let mut last = prefix; while i < limit { last = String.concat(left: prefix, right: String.from_int(value: i * 7)); Map.insert(map: mut table, key: last, value: i); i = i + 1 }; let n = Map.len(map: read table) + String.len(value: last); Output.write(message: String.from_int(value: n)); return n } fn main() -> Unit { Output.write(message: String.from_int(value: hot(limit: 3000, prefix: \"k\"))); return Unit }",
    ),
];

#[test]
fn an_osr_loop_that_builds_its_map_key_accounts_exactly_armed_and_unarmed() {
    for (name, source) in OSR_BUILT_KEY_CASES {
        let mut osr_entries = 0_u64;
        let mut limit_cases = vec![RunLimits::unbounded_for_trusted_host()];
        limit_cases.extend(
            STEP_PARITY_BUDGETS
                .iter()
                .map(|budget| RunLimits::unbounded_for_trusted_host().with_step_budget(*budget)),
        );
        limit_cases
            .push(RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(1_000_000));
        for limits in limit_cases {
            for eager_osr in [false, true] {
                let (interpreter, native) = accounting_pair(
                    name,
                    source,
                    limits.clone(),
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
                    "{name} (eager_osr={eager_osr}) must terminate for the same reason as the interpreter"
                );
                assert_eq!(
                    native.usage.steps_consumed, interpreter.usage.steps_consumed,
                    "{name} (eager_osr={eager_osr}) must report the interpreter's step count"
                );
                assert_eq!(
                    native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                    "{name} (eager_osr={eager_osr}) must report the interpreter's intrinsic count"
                );
                assert_eq!(
                    native.stdout, interpreter.stdout,
                    "{name} (eager_osr={eager_osr}) must produce the interpreter's output"
                );
                osr_entries = osr_entries.saturating_add(native_telemetry(&native).osr_entries);
            }
        }
        assert!(
            osr_entries > 0,
            "{name} must enter OSR for its counts to be a statement about generated code"
        );
    }
}

/// An armed allocation budget or live-memory limit still refuses the region: the
/// string helper does not charge its own capacity delta into the transaction-local
/// allocation cell the way `List.push` does, so the region fails closed rather
/// than under-reporting.
#[test]
fn an_osr_loop_that_builds_its_map_key_declines_under_armed_memory_controls() {
    let (name, source) = OSR_BUILT_KEY_CASES[0];
    for limits in [
        RunLimits::unbounded_for_trusted_host().with_allocation_budget(4_000_000),
        RunLimits::unbounded_for_trusted_host().with_live_memory_limit(4_000_000),
    ] {
        let (interpreter, native) = accounting_pair(
            name,
            source,
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
            "{name} under an armed memory control must terminate like the interpreter"
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "{name} under an armed memory control must report the interpreter's steps"
        );
        assert_eq!(
            native.usage.allocation_bytes_consumed, interpreter.usage.allocation_bytes_consumed,
            "{name} under an armed memory control must report the interpreter's allocation bytes"
        );
        assert_eq!(
            native_telemetry(&native).osr_entries,
            0,
            "the string helper charges no capacity delta, so the region must decline"
        );
    }
}

#[test]
fn an_osr_loop_whose_body_matches_reaches_generated_code_and_accounts_exactly() {
    for (name, source) in OSR_MATCH_LAYOUT_PARITY_CASES {
        for eager_osr in [false, true] {
            let mut osr_entries = 0_u64;
            for &budget in STEP_PARITY_BUDGETS {
                let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
                let (interpreter, native) = accounting_pair_with_args(
                    name,
                    source,
                    &["3000"],
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
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must terminate for the same reason as the interpreter"
                );
                assert_eq!(
                    native.usage.steps_consumed, interpreter.usage.steps_consumed,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must report the interpreter's step count"
                );
                assert_eq!(
                    native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must report the interpreter's intrinsic count"
                );
                assert_eq!(
                    native.stdout, interpreter.stdout,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must produce the interpreter's output"
                );
                osr_entries = osr_entries.saturating_add(native_telemetry(&native).osr_entries);
            }
            // The counts above are a statement about the attribution only if the
            // loop actually reaches generated code; declining would satisfy them.
            assert!(
                osr_entries > 0,
                "{name} (eager_osr={eager_osr}) must enter OSR for its step counts to mean anything"
            );
        }
    }
}

#[test]
fn an_osr_loop_whose_body_matches_accounts_intrinsics_under_the_production_defaults() {
    for (name, source) in OSR_MATCH_LAYOUT_PARITY_CASES {
        let mut osr_entries = 0_u64;
        for limits in [
            RunLimits::unbounded_for_trusted_host(),
            RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(1_500),
            RunLimits::unbounded_for_trusted_host()
                .with_intrinsic_call_budget(1_000_000)
                .with_step_budget(10_000_000),
        ] {
            let (interpreter, native) = accounting_pair_with_args(
                name,
                source,
                &["3000"],
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
                "{name} must terminate like the interpreter under the production tiering defaults"
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "{name} must report the interpreter's intrinsic count"
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{name} must report the interpreter's step count"
            );
            osr_entries = osr_entries.saturating_add(native_telemetry(&native).osr_entries);
        }
        assert!(
            osr_entries > 0,
            "{name} must enter OSR under the production tiering defaults"
        );
    }
}

#[test]
fn the_kernels_the_layout_normalization_does_not_unblock_still_account_exactly() {
    for (name, source, reason) in OSR_MATCH_LAYOUT_DECLINED_KERNELS {
        for &budget in STEP_PARITY_BUDGETS {
            let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
            let (interpreter, native) = accounting_pair_with_args(
                name,
                source,
                &["3000"],
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
                "{name} at step budget {budget} must terminate for the same reason as the interpreter"
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{name} at step budget {budget} must report the interpreter's step count"
            );
            assert_eq!(
                native.stdout, interpreter.stdout,
                "{name} at step budget {budget} must produce the interpreter's output"
            );
            assert_eq!(
                native_region_entries(&native),
                0,
                "{name} is still interpreter-owned ({reason}); if that changed, update the \
                 gap list in docs/spec/native-jit-contract.md"
            );
        }
    }
}

#[test]
fn native_step_accounting_matches_the_interpreter_for_an_osr_loop_containing_an_inlined_call() {
    for (name, source) in OSR_INLINE_PARITY_CASES {
        for eager_osr in [false, true] {
            let mut osr_entries = 0_u64;
            for &budget in STEP_PARITY_BUDGETS {
                let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
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
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must terminate for the same reason as the interpreter"
                );
                assert_eq!(
                    native.usage.steps_consumed, interpreter.usage.steps_consumed,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must report the interpreter's step count"
                );
                assert_eq!(
                    native.stdout, interpreter.stdout,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must produce the interpreter's output"
                );
                osr_entries = osr_entries.saturating_add(native_telemetry(&native).osr_entries);
            }
            // Without this the test would pass by declining the region, which is
            // exactly the behavior it exists to retire.
            assert!(
                osr_entries > 0,
                "{name} (eager_osr={eager_osr}) must enter OSR for its step counts to mean anything"
            );
        }
    }
}

#[test]
fn an_osr_loop_containing_an_inlined_call_reports_the_interpreter_intrinsic_call_count() {
    for (name, source) in OSR_INLINE_PARITY_CASES {
        for eager_osr in [false, true] {
            let mut osr_entries = 0_u64;
            for limits in [
                RunLimits::unbounded_for_trusted_host(),
                RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(1_500),
                RunLimits::unbounded_for_trusted_host()
                    .with_intrinsic_call_budget(1_000_000)
                    .with_step_budget(10_000_000),
            ] {
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
                    "{name} (eager_osr={eager_osr}) must terminate like the interpreter"
                );
                assert_eq!(
                    native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                    "{name} (eager_osr={eager_osr}) must report the interpreter's intrinsic count"
                );
                assert_eq!(
                    native.usage.steps_consumed, interpreter.usage.steps_consumed,
                    "{name} (eager_osr={eager_osr}) must report the interpreter's step count"
                );
                osr_entries = osr_entries.saturating_add(native_telemetry(&native).osr_entries);
            }
            assert!(
                osr_entries > 0,
                "{name} (eager_osr={eager_osr}) must enter OSR under an armed intrinsic budget"
            );
        }
    }
}

/// A loop that allocates and calls a `local` closure runs natively and keeps
/// exact step accounting.
///
/// This shape is the one the native-jit contract's closure-sinking gap named.
/// `loop_local_sinkable_closures` marks the loop-local `MakeClosure` and its copy
/// `Move`s dead and `native_inline_leaf_calls_inner` deletes them, so those
/// instructions own no item of their own while the interpreter still ticks them.
/// The pass now moves each deleted instruction's step onto the next emitted item
/// and declines when that item could run without the deleted one having run
/// (`sunk_instruction_accounting_tests` in
/// `crates/rsscript-vm/src/reg_vm/native/passes/inlining.rs` pins both halves),
/// which is what let the sunk `CallClosure` inline arm and the OSR closure-operand
/// candidate be turned on.
///
/// The assertion is therefore the opposite of what it used to be: the loop must
/// reach generated code, so the step counts below are a statement about a
/// natively executed closure loop rather than about a declined one.
#[test]
fn a_closure_bearing_loop_accounts_steps_exactly_in_generated_code() {
    const SOURCE: &str = "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { local f = |x| { return x * 2 + 1 }; total = total + f(i); i = i + 1 }; return total } fn main() -> Int { return hot(limit: 3000) }";

    for eager_osr in [false, true] {
        let mut regions = 0_u64;
        for &budget in STEP_PARITY_BUDGETS {
            let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
            let (interpreter, native) = accounting_pair(
                "closure-in-loop.rss",
                SOURCE,
                limits,
                NativeJitOptions {
                    cost_model: NativeCostModel::Off,
                    collect_telemetry: true,
                    eager_osr,
                    ..NativeJitOptions::default()
                },
            );
            assert_eq!(
                native.outcome(),
                interpreter.outcome(),
                "closure loop at step budget {budget} (eager_osr={eager_osr}) must terminate for the same reason as the interpreter"
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "closure loop at step budget {budget} (eager_osr={eager_osr}) must report the interpreter's step count"
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "closure loop at step budget {budget} (eager_osr={eager_osr}) must report the interpreter's intrinsic count"
            );
            regions = regions.saturating_add(native_region_entries(&native));
        }
        assert!(
            regions > 0,
            "a closure-bearing loop must reach generated code with the sunk `MakeClosure` accounted (eager_osr={eager_osr})"
        );
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

/// Intrinsic-heavy shapes whose natively executed regions must charge
/// `intrinsic_calls` exactly.
///
/// Generated code runs an intrinsic both as a host helper (`String.len`,
/// `Map.get`) and as a direct lowering with no helper at all (`List.len` becomes
/// `ListLenDirect`, a flat-list `List.get` becomes a direct load), so there is no
/// single helper-side charge point. Each native item instead carries an
/// `intrinsic_cost` beside its `source_cost`, charged at the same block/segment
/// points into the call-owned limits cell.
struct IntrinsicParityCase {
    name: &'static str,
    source: &'static str,
    /// Whether the shape reaches generated code with the production tiering
    /// defaults (automatic OSR only). A shape that only tiers up under eager OSR
    /// still has to report the interpreter's counts; it just cannot pin
    /// engagement on the default path.
    reaches_native_by_default: bool,
}

const INTRINSIC_PARITY_CASES: &[IntrinsicParityCase] = &[
    // `List.len` lowers directly (`ListLenDirect`) and the flat `List.get` lowers
    // to a direct load: neither passes through a host helper, so a helper-side
    // charge would miss both.
    IntrinsicParityCase {
        name: "intrinsic-direct-list.rss",
        reaches_native_by_default: true,
        source: "fn hot(values: List<Int>, limit: Int) -> Int { let mut i = 0; let mut total = 0; let n = List.len<Int>(list: values); while i < limit { total = total + List.len<Int>(list: values) + List.get<Int>(list: values, index: i % n); i = i + 1 }; return total } fn main() -> Int { local values = List.new<Int>(); List.push<Int>(list: mut values, value: 5); List.push<Int>(list: mut values, value: 7); List.push<Int>(list: mut values, value: 11); List.push<Int>(list: mut values, value: 13); return hot(values, limit: 1000) }",
    },
    // `String.len` runs as a read-only host helper inside the loop.
    IntrinsicParityCase {
        name: "intrinsic-string-helper.rss",
        reaches_native_by_default: true,
        source: "fn hot(text: String, limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + String.len(value: text); i = i + 1 }; return total } fn main() -> Int { return hot(text: \"rsscript\", limit: 1000) }",
    },
    // An `Int`-keyed map get plus the collection length helpers: the map get also
    // bills the constant key-hash unit on top of its own tick, so the step and
    // intrinsic meters must stay independent.
    IntrinsicParityCase {
        name: "intrinsic-map-get.rss",
        // Automatic OSR does not pick this loop up; eager OSR does.
        reaches_native_by_default: false,
        source: "fn hot(table: Map<Int, Int>, limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + Map.len<Int, Int>(map: table); match Map.get<Int, Int>(map: table, key: i % 8) { Some(value) => { total = total + value } None => { total = total - 1 } }; i = i + 1 }; return total } fn main() -> Int { local table = Map<Int, Int>.new(); let mut k = 0; while k < 8 { Map.insert<Int, Int>(map: mut table, key: k, value: k * 2); k = k + 1 }; return hot(table, limit: 1000) }",
    },
    // The intrinsic sits inside a leaf callee the inliner dissolves, so the
    // spliced item — not the caller's call instruction — owns the dispatch.
    IntrinsicParityCase {
        name: "intrinsic-inlined-leaf.rss",
        reaches_native_by_default: true,
        source: "fn width(text: String) -> Int { return String.len(value: text) } fn hot(text: String, limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + width(text); i = i + 1 }; return total } fn main() -> Int { return hot(text: \"rsscript\", limit: 1000) }",
    },
];

/// Intrinsic budgets chosen to land before, inside and past each shape's native
/// regions, including the exact boundary values, so an off-by-one in reservation
/// or deopt roll-back changes the reported count.
const INTRINSIC_PARITY_BUDGETS: &[u64] = &[
    1, 2, 3, 12, 99, 100, 101, 511, 512, 513, 999, 1_000, 1_001, 1_002, 1_999, 2_000, 2_001,
    1_000_000,
];

#[test]
fn native_intrinsic_accounting_matches_the_interpreter_under_an_armed_budget() {
    for case in INTRINSIC_PARITY_CASES {
        let mut native_regions = 0_u64;
        for &budget in INTRINSIC_PARITY_BUDGETS {
            let limits = RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(budget);
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
                "{} at intrinsic budget {budget} must terminate for the same reason as the interpreter",
                case.name
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "{} at intrinsic budget {budget} must report the interpreter's intrinsic count",
                case.name
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{} at intrinsic budget {budget} must still report the interpreter's step count",
                case.name
            );
            native_regions = native_regions.saturating_add(native_region_entries(&native));
        }
        assert_eq!(
            native_regions > 0,
            case.reaches_native_by_default,
            "{} native engagement under an armed intrinsic budget changed",
            case.name
        );
    }
}

#[test]
fn native_intrinsic_accounting_matches_the_interpreter_at_region_boundaries() {
    // The same shapes with a step budget armed as well, so the region uses the
    // segment-reservation model rather than block charging and both meters must
    // agree at the same segment boundary.
    for case in INTRINSIC_PARITY_CASES {
        for &(steps, intrinsics) in &[
            (100_u64, 100_u64),
            (1_000, 50),
            (10_000, 1_000),
            (10_000_000, 1_001),
            (10_000_000, 10_000_000),
        ] {
            let limits = RunLimits::unbounded_for_trusted_host()
                .with_step_budget(steps)
                .with_intrinsic_call_budget(intrinsics);
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
                "{} at step {steps}/intrinsic {intrinsics} must match the interpreter outcome",
                case.name
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "{} at step {steps}/intrinsic {intrinsics} must report the interpreter's intrinsic count",
                case.name
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{} at step {steps}/intrinsic {intrinsics} must report the interpreter's step count",
                case.name
            );
        }
    }
}

#[test]
fn native_intrinsic_accounting_matches_the_interpreter_under_eager_osr() {
    for case in INTRINSIC_PARITY_CASES {
        for &budget in &[100_u64, 1_000, 1_001, 1_000_000] {
            let limits = RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(budget);
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
                "{} under eager OSR at intrinsic budget {budget} must match the interpreter outcome",
                case.name
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "{} under eager OSR at intrinsic budget {budget} must report the interpreter's intrinsic count",
                case.name
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{} under eager OSR at intrinsic budget {budget} must report the interpreter's steps",
                case.name
            );
        }
    }
}

#[test]
fn an_unbounded_native_run_reports_the_interpreter_intrinsic_call_count() {
    // `intrinsic_calls` is a reported usage fact, not only a ceiling. A natively
    // executed region used to report only the intrinsics the interpreter happened
    // to run outside generated code.
    for case in INTRINSIC_PARITY_CASES {
        for eager_osr in [false, true] {
            let (interpreter, native) = accounting_pair(
                case.name,
                case.source,
                RunLimits::unbounded_for_trusted_host(),
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
                "{} (eager_osr={eager_osr}) must terminate like the interpreter with nothing armed",
                case.name
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "{} (eager_osr={eager_osr}) must report the interpreter's intrinsic count with nothing armed",
                case.name
            );
            if eager_osr || case.reaches_native_by_default {
                assert!(
                    native_region_entries(&native) > 0
                        || native_telemetry(&native).native_bails > 0,
                    "{} (eager_osr={eager_osr}) must actually reach generated code",
                    case.name
                );
            }
        }
    }
}

#[test]
fn an_armed_intrinsic_call_budget_no_longer_refuses_native_dispatch() {
    // The gate used to be unconditional: `native_preemption_controls_supported`
    // and `osr_execution_controls_supported` both refused every whole-function and
    // OSR region while `intrinsic_call_budget` was armed, which left the default
    // runner profile with no native tier at all.
    let case = &INTRINSIC_PARITY_CASES[0];
    let limits = RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(1_000_000);
    let (_, native) = accounting_pair(
        case.name,
        case.source,
        limits,
        NativeJitOptions {
            cost_model: NativeCostModel::Off,
            collect_telemetry: true,
            ..NativeJitOptions::default()
        },
    );
    let telemetry = native_telemetry(&native);
    assert!(
        telemetry.native_calls + telemetry.osr_entries > 0,
        "an armed intrinsic call budget must still admit whole-function or OSR dispatch"
    );
}

#[test]
fn a_custom_max_depth_no_longer_refuses_whole_function_native_entry() {
    // `attempt_native` used to refuse every whole-function region whenever
    // `max_depth` differed from the VM's `DEFAULT_MAX_DEPTH`, because the internal
    // ABI was said to carry only a host-stack cap. It carries the logical limit
    // too — `RegionCallControls::logical_depth` forwards it exactly as OSR entry
    // already did — so entry now declines only when the configured limit is within
    // reach of the region's static frame bound.
    //
    // A recursion that exceeds a small limit must still terminate with the
    // interpreter's reason and count, and an ordinary hot loop must still tier up
    // under a non-default limit.
    let deep_recursion = "fn down(n: Int) -> Int { if n <= 0 { return 0 }; return 1 + down(n: n - 1) } fn main() -> Int { return down(n: 5000) }";
    for &max_depth in &[4_usize, 16, 64, 256] {
        let limits = RunLimits::unbounded_for_trusted_host().with_max_depth(max_depth);
        let (interpreter, native) = accounting_pair(
            "custom-max-depth-recursion.rss",
            deep_recursion,
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
            "a recursion past max_depth {max_depth} must terminate with the interpreter's reason"
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "a recursion past max_depth {max_depth} must report the interpreter's step count"
        );
    }

    // The hot loop is a leaf, so its static frame bound is its own frame plus the
    // dissolved-leaf allowance; a 256-frame profile leaves it far out of reach and
    // native entry must happen.
    let hot = "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 3000) }";
    let (interpreter, native) = accounting_pair(
        "custom-max-depth-hot-loop.rss",
        hot,
        RunLimits::unbounded_for_trusted_host().with_max_depth(256),
        NativeJitOptions {
            cost_model: NativeCostModel::Off,
            collect_telemetry: true,
            ..NativeJitOptions::default()
        },
    );
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(
        native.usage.steps_consumed, interpreter.usage.steps_consumed,
        "a hot loop under a non-default max_depth must report the interpreter's step count"
    );
    let telemetry = native_telemetry(&native);
    assert!(
        telemetry.native_calls + telemetry.osr_entries > 0,
        "a non-default max_depth must no longer refuse whole-function or OSR dispatch"
    );
}

/// The limit profile `rss run --trusted-in-process` applies, mirrored from
/// `RunnerLimitsV1::default()` through `runner::runner_limits`. `--native`
/// selects an accelerator, not a trust level, so it now runs under exactly this
/// profile instead of replacing it with
/// `RunLimits::unbounded_for_trusted_host()`.
fn default_runner_limit_profile() -> RunLimits {
    RunLimits::bounded()
        .with_max_depth(256)
        .with_step_budget(10_000_000)
        .with_allocation_budget(256 * 1024 * 1024)
        .with_live_memory_limit(128 * 1024 * 1024)
        .with_output_budget(1024 * 1024)
        .with_intrinsic_call_budget(1_000_000)
        .with_provider_call_budget(10_000)
        .with_resource_limit(4096)
        .with_deadline(MonotonicDeadline::after(Duration::from_millis(60_000)))
}

#[test]
fn the_default_runner_limit_profile_still_admits_native_dispatch() {
    // Every gate in this profile used to refuse: `intrinsic_call_budget` refused
    // whole-function and OSR dispatch outright, and a `max_depth` of 256 refused
    // whole-function entry because it differs from the VM's `DEFAULT_MAX_DEPTH`.
    // That is why the CLI replaced the profile wholesale; with both gates closed
    // it no longer has to.
    //
    // These three shapes keep their hot work in a `main`-resident loop or in a
    // helper called from one. That is no longer a workaround for anything: the
    // plain `fn main() { ... hot(200000) ... }` shape, whose whole hot loop lives
    // in a called helper and whose `main` does nothing else, reaches native under
    // this same profile and is pinned by
    // `a_called_hot_helper_reaches_native_under_the_default_runner_limits` below.
    for (name, source) in [
        (
            "runner-profile-scalar-loop.rss",
            "fn main() -> Int { let mut i = 0; let mut total = 0; while i < 200000 { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total }",
        ),
        (
            "runner-profile-repeated-entry.rss",
            "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total } fn main() -> Int { let mut out = 0; let mut r = 0; while r < 50 { out = hot(limit: 200); r = r + 1 }; return out }",
        ),
        (
            "runner-profile-intrinsic-loop.rss",
            "fn main() -> Int { let text = \"rsscript\"; let mut i = 0; let mut total = 0; while i < 20000 { total = total + String.len(value: text); i = i + 1 }; return total }",
        ),
    ] {
        let (interpreter, native) = accounting_pair(
            name,
            source,
            default_runner_limit_profile(),
            NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            },
        );
        assert_eq!(
            native.outcome(),
            interpreter.outcome(),
            "{name} must terminate like the interpreter under the default runner profile"
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "{name} must report the interpreter's step count under the default runner profile"
        );
        assert_eq!(
            native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
            "{name} must report the interpreter's intrinsic count under the default runner profile"
        );
        let telemetry = native_telemetry(&native);
        assert!(
            telemetry.native_calls + telemetry.osr_entries > 0,
            "{name} must reach whole-function or OSR dispatch under the default runner profile"
        );
    }
}

#[test]
fn the_default_runner_limit_profile_still_stops_an_over_budget_native_run() {
    let source = "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 100000000) }";
    let (interpreter, native) = accounting_pair(
        "runner-profile-over-budget.rss",
        source,
        default_runner_limit_profile(),
        NativeJitOptions {
            cost_model: NativeCostModel::Off,
            collect_telemetry: true,
            ..NativeJitOptions::default()
        },
    );
    assert_eq!(
        native.termination_reason(),
        TerminationReason::StepBudgetExceeded
    );
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(
        native.usage.steps_consumed,
        interpreter.usage.steps_consumed
    );
}

/// `fn main() { ... hot(200000) ... }`: a `main` whose whole hot loop lives in a
/// called helper, under the profile `rss run --trusted-in-process --native`
/// keeps.
///
/// This shape used to reach no native tier at all, and the reason was not that
/// the helper "failed to get hot" — it was never offered. Whole-function native
/// entry is offered `main` first and declines, because
/// `whole_function_memory_controls_supported`
/// (`crates/rsscript-vm/src/reg_vm/tier.rs`) refuses any body containing a call
/// while an allocation or live-memory control is armed, and this profile arms
/// both. `RegVm::drive` then handed the frame to the tier-0 executor, which runs
/// a whole call tree inside one frame: `RegVm::run_jit` executes a `CallKnown`
/// to a pure-leaf callee through `run_jit_pure_leaf` instead of pushing a frame,
/// so `hot` never became a `drive` frame and `RegVm::attempt_native` never saw
/// its body. `drive` now keeps a frame on the interpreter loop when tier-0 would
/// swallow a callee that is still native-eligible, and the interpreter's own
/// `CallKnown` pushes a real frame per call and gives the native tier first
/// refusal on the callee.
///
/// Nothing about the allocation proof changed: the helper reaches native on its
/// own proof (a scalar body cannot grow storage; a `List.push` body goes through
/// the OSR transaction cell), and `main` still declines.
const CALLED_HELPER_SCALAR: &str = "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 200000) }";

/// The same shape with a growing helper. `List.push` is the one allocating
/// helper the OSR memory proof admits, so this reaches generated code through an
/// OSR entry whose capacity deltas are charged into the transaction-local cell
/// and committed with the heap transaction.
const CALLED_HELPER_LIST_PUSH: &str = "fn hot(limit: Int) -> Int { local xs = List<Int>.new(); let mut i = 0; let mut total = 0; while i < limit { List.push<Int>(list: mut xs, value: i); total = total + i; i = i + 1 }; return total } fn main() -> Int { return hot(limit: 200000) }";

/// A scalar helper that reaches native, followed by a growing helper that runs
/// the run out of memory. Used to pin that an armed ceiling still trips with the
/// interpreter's reason and counts *after* native code has already executed and
/// charged part of the run.
const CALLED_HELPER_WARM_THEN_GROW: &str = "fn hot(limit: Int) -> Int { let mut i = 0; let mut total = 0; while i < limit { total = total + i * 3 - i / 2 + 7; i = i + 1 }; return total } fn grow(limit: Int) -> Int { local xs = List<Int>.new(); let mut i = 0; while i < limit { List.push<Int>(list: mut xs, value: i); i = i + 1 }; return List.len<Int>(list: xs) } fn main() -> Int { let warm = hot(limit: 200000); return warm + grow(limit: 200000) }";

fn assert_called_helper_parity(
    name: &str,
    interpreter: &ExecutionReport,
    native: &ExecutionReport,
) {
    assert_eq!(
        native.outcome(),
        interpreter.outcome(),
        "{name} must produce the interpreter's outcome"
    );
    assert_eq!(
        native.termination_reason(),
        interpreter.termination_reason(),
        "{name} must terminate for the interpreter's reason"
    );
    assert_eq!(
        native.usage.steps_consumed, interpreter.usage.steps_consumed,
        "{name} must report the interpreter's step count"
    );
    assert_eq!(
        native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
        "{name} must report the interpreter's intrinsic-call count"
    );
    assert_eq!(
        native.usage.allocation_bytes_consumed, interpreter.usage.allocation_bytes_consumed,
        "{name} must report the interpreter's allocation bytes"
    );
    assert_eq!(
        native.usage.peak_live_memory_bytes, interpreter.usage.peak_live_memory_bytes,
        "{name} must report the interpreter's peak live memory"
    );
    assert_eq!(
        native.usage.live_memory_bytes_at_return, interpreter.usage.live_memory_bytes_at_return,
        "{name} must report the interpreter's live memory at return"
    );
}

#[test]
fn a_called_hot_helper_reaches_native_under_the_default_runner_limits() {
    for (name, source) in [
        ("called-helper-scalar.rss", CALLED_HELPER_SCALAR),
        ("called-helper-list-push.rss", CALLED_HELPER_LIST_PUSH),
    ] {
        let (interpreter, native) = accounting_pair(
            name,
            source,
            default_runner_limit_profile(),
            NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            },
        );
        assert_called_helper_parity(name, &interpreter, &native);
        let telemetry = native_telemetry(&native);
        assert!(
            telemetry.native_calls + telemetry.osr_entries > 0,
            "{name}: a hot helper called from `main` must reach whole-function or OSR dispatch under the default runner profile (native_calls={}, osr_entries={})",
            telemetry.native_calls,
            telemetry.osr_entries,
        );
    }
}

fn completed_reference(name: &str, source: &str) -> ExecutionReport {
    let reference = accounting_pair(
        name,
        source,
        default_runner_limit_profile(),
        NativeJitOptions {
            cost_model: NativeCostModel::Off,
            collect_telemetry: true,
            ..NativeJitOptions::default()
        },
    )
    .0;
    assert_eq!(
        reference.termination_reason(),
        TerminationReason::Completed,
        "{name}: the reference run must complete so the derived ceilings are real usage"
    );
    reference
}

#[test]
fn a_called_hot_helper_stops_on_the_interpreter_memory_reason() {
    // Size every ceiling from a completed reference run, so the trip point is a
    // property of the program rather than of this machine.
    let growing = completed_reference(
        "called-helper-memory-reference.rss",
        CALLED_HELPER_WARM_THEN_GROW,
    );
    let scalar = completed_reference("called-helper-frame-reference.rss", CALLED_HELPER_SCALAR);

    struct Case {
        name: &'static str,
        source: &'static str,
        limits: RunLimits,
        expected: TerminationReason,
        /// Whether generated code must have run before the ceiling tripped.
        expect_native: bool,
    }

    for case in [
        // The scalar helper reaches whole-function native entry and completes;
        // the growing helper then runs the allocation budget out. The ceiling is
        // therefore enforced *after* generated code has already executed and
        // charged part of this run, not by refusing native dispatch outright.
        Case {
            name: "called-helper-allocation-budget.rss",
            source: CALLED_HELPER_WARM_THEN_GROW,
            limits: default_runner_limit_profile()
                .with_allocation_budget(growing.usage.allocation_bytes_consumed / 4),
            expected: TerminationReason::AllocationBudgetExceeded,
            expect_native: true,
        },
        Case {
            name: "called-helper-live-memory.rss",
            source: CALLED_HELPER_WARM_THEN_GROW,
            limits: default_runner_limit_profile()
                .with_live_memory_limit(growing.usage.peak_live_memory_bytes / 2),
            expected: TerminationReason::LiveMemoryLimitExceeded,
            expect_native: true,
        },
        // The whole `main -> hot` call graph is tier-0 eligible, so this is the
        // exact shape the interpreter loop now owns instead of tier-0. The only
        // storage it grows is the shared register stack `RegVm::ensure_regs`
        // charges when the callee's frame window is opened, so a budget one byte
        // short of the completed run has to trip on that charge, on the same
        // instruction, with the same count — proof that moving the call off
        // tier-0 did not move an allocation charge with it.
        Case {
            name: "called-helper-callee-frame-budget.rss",
            source: CALLED_HELPER_SCALAR,
            limits: default_runner_limit_profile()
                .with_allocation_budget(scalar.usage.allocation_bytes_consumed.saturating_sub(1)),
            expected: TerminationReason::AllocationBudgetExceeded,
            expect_native: false,
        },
    ] {
        let Case {
            name,
            source,
            limits,
            expected,
            expect_native,
        } = case;
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
        assert_eq!(
            interpreter.termination_reason(),
            expected,
            "{name} must trip the expected ceiling on the interpreter"
        );
        assert_called_helper_parity(name, &interpreter, &native);
        let telemetry = native_telemetry(&native);
        if expect_native {
            // `native_calls` specifically: that is the scalar helper's own
            // whole-function entry. The growing helper's OSR region hits the
            // ceiling and rolls back, which is a fail-closed exit and counts no
            // entry at all.
            assert!(
                telemetry.native_calls > 0,
                "{name}: the scalar helper must still reach whole-function native entry before the ceiling trips (native_calls={}, osr_entries={})",
                telemetry.native_calls,
                telemetry.osr_entries,
            );
        }
    }
}

/// OSR and continuation regions whose rewrites replace or delete the intrinsic
/// dispatch the interpreter still runs.
///
/// The string and bytes length-law folds turn `String.len`/`Bytes.len` into
/// arithmetic on operand byte lengths and delete the now-dead allocation, so the
/// transformed stream carries no intrinsic dispatch at all. The intrinsic meter
/// therefore reads the *source* instruction each item is charged for, not the
/// transformed one it lowers: derived from the transformed stream these three
/// shapes report 47, 92 and 3 intrinsic calls against the interpreter's 3002,
/// 6002 and 3003.
///
/// Each shape puts its loop in a function that also writes output, so
/// whole-function native entry declines and the loop can only reach generated
/// code through OSR or a continuation.
const OSR_INTRINSIC_PARITY_CASES: &[(&str, &str)] = &[
    (
        "osr-string-length-fold.rss",
        "fn main() -> Unit { let mut i = 0; let mut total = 0; while i < 3000 { total = total + String.len(value: String.concat(left: \"ab\", right: \"cde\")); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
    (
        "osr-string-from-int-fold.rss",
        "fn main() -> Unit { let mut i = 0; let mut total = 0; while i < 3000 { total = total + String.len(value: String.from_int(value: i)); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
    (
        "osr-map-len-helper.rss",
        "fn main() -> Unit { local table = Map<Int, Int>.new(); Map.insert<Int, Int>(map: mut table, key: 1, value: 2); let mut i = 0; let mut total = 0; while i < 3000 { total = total + Map.len<Int, Int>(map: table); i = i + 1 }; Output.write(message: String.from_int(value: total)); return Unit }",
    ),
];

#[test]
fn a_rewritten_osr_region_reports_the_interpreter_intrinsic_call_count() {
    for (name, source) in OSR_INTRINSIC_PARITY_CASES {
        for eager_osr in [false, true] {
            for limits in [
                RunLimits::unbounded_for_trusted_host(),
                RunLimits::unbounded_for_trusted_host().with_intrinsic_call_budget(1_500),
                RunLimits::unbounded_for_trusted_host()
                    .with_intrinsic_call_budget(1_000_000)
                    .with_step_budget(10_000_000),
            ] {
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
                    "{name} (eager_osr={eager_osr}) must terminate like the interpreter"
                );
                assert_eq!(
                    native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                    "{name} (eager_osr={eager_osr}) must report the interpreter's intrinsic count"
                );
                assert_eq!(
                    native.usage.steps_consumed, interpreter.usage.steps_consumed,
                    "{name} (eager_osr={eager_osr}) must report the interpreter's step count"
                );
                assert_eq!(
                    native.stdout, interpreter.stdout,
                    "{name} (eager_osr={eager_osr}) must produce the interpreter's output"
                );
            }
        }
    }
}

#[test]
fn the_rewritten_osr_cases_reach_generated_code_outside_whole_function_entry() {
    // Pins what makes the case above meaningful: these loops are entered through
    // OSR or a continuation, not through whole-function translation, which
    // accounts through a different pipeline.
    for (name, source) in OSR_INTRINSIC_PARITY_CASES {
        let (_, native) = accounting_pair(
            name,
            source,
            RunLimits::unbounded_for_trusted_host(),
            NativeJitOptions {
                cost_model: NativeCostModel::Off,
                collect_telemetry: true,
                ..NativeJitOptions::default()
            },
        );
        let telemetry = native_telemetry(&native);
        assert_eq!(
            telemetry.native_calls, 0,
            "{name} must not reach whole-function native entry"
        );
        assert!(
            telemetry.osr_entries + telemetry.continuation_entries > 0,
            "{name} must reach generated code through OSR or a continuation"
        );
    }
}

/// A loop that calls a *callback parameter* runs identically on both engines.
///
/// The closure a `CallClosure` dispatches through is a value the caller chose,
/// and for a callback parameter its `MakeClosure` is in another frame entirely,
/// so the native tier cannot prove which body it enters. It must therefore
/// fail closed at that call rather than speculate, and the interpreter stays
/// the accounting oracle: the outcome and `steps_consumed` must agree at every
/// step boundary, including the budgets that stop mid-callback.
#[test]
fn a_callback_parameter_loop_matches_the_interpreter_under_a_step_budget() {
    const SOURCE: &str = r#"
fn fold(values: read List<Int>, f: noescape Fn(Int) -> Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < List.len(list: values) {
        total = total + f(List.get(list: values, index: i))
        i = i + 1
    }
    return total
}

fn main() -> Int {
    let values: fresh List<Int> = [1, 2, 3, 4, 5, 6, 7, 8]
    let bias = 3
    return fold(values: values, f: |x| { return x * 2 + bias })
}
"#;

    let built = Compiler
        .compile("callback-parameter-loop.rss", SOURCE)
        .expect("callback parameter source compiles");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("callback parameter artifact verifies")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("callback parameter artifact links");

    let options = || NativeJitOptions {
        cost_model: NativeCostModel::Off,
        collect_telemetry: true,
        eager_osr: true,
        ..NativeJitOptions::default()
    };

    let completed = linked.execute(ExecutionRequest::default());
    assert_eq!(completed.termination_reason(), TerminationReason::Completed);
    assert_eq!(completed.value(), Some("96"));

    for budget in 0..=completed.usage.steps_consumed.saturating_add(1) {
        let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
        let interpreter = linked.execute(ExecutionRequest::default().limits(limits.clone()));
        let native = linked.execute(
            ExecutionRequest::default()
                .limits(limits)
                .native_jit(options()),
        );
        assert_eq!(
            native.termination_reason(),
            interpreter.termination_reason(),
            "termination at step budget {budget}"
        );
        assert_eq!(
            native.outcome(),
            interpreter.outcome(),
            "outcome at {budget}"
        );
        assert_eq!(
            native.usage.steps_consumed, interpreter.usage.steps_consumed,
            "steps at budget {budget}"
        );
    }

    // Fail-closed, not silently-wrong: an unproved callee behind `CallClosure`
    // must leave the region to the VM. Whichever way the tier decides, the
    // assertions above already pin that the decision changes no observable.
    let native = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host())
            .native_jit(options()),
    );
    let &NativeExecutionEngineTelemetry {
        compiled,
        native_calls,
        osr_entries,
        continuation_entries,
        rejected_resident_bytes,
        ..
    } = native_telemetry(&native);
    assert_eq!(
        (compiled, native_calls, osr_entries, continuation_entries),
        (0, 0, 0, 0),
        "a `CallClosure` on a parameter has no provable callee, so every region \
         containing it must be declined rather than speculated on"
    );
    assert_eq!(rejected_resident_bytes, 0);
    assert_eq!(native.value(), completed.value());
}

/// Shapes whose loop hashes a key whose cost is a property of the runtime value.
///
/// `map_key_from_value` bills `1 + len / 64` for a `String`/`Bytes` key through
/// `RegVm::charge_work`, which charges nothing at all unless a step budget,
/// cancellation token or deadline is armed. Generated code cannot know that cost
/// at compile time and keeps its running count in a register, so
/// `MapInsertHandleKeyInt` and `SetInsertHandle` charge it themselves against the
/// call-owned limits cell while the region flushes and reloads its count around
/// them, and `native_source_cost_is_static` no longer declines the region.
///
/// Each loop sits in a function that also writes output, so it reaches generated
/// code through OSR rather than whole-function entry, and the long-key shape makes
/// the `len / 64` term two units rather than one so a flat constant charge would
/// drift.
const KEY_HASH_PARITY_CASES: &[(&str, &str)] = &[
    (
        "string-keyed-map-insert.rss",
        "fn main() -> Unit { local table = Map<String, Int>.new(); let key = \"fixed-key\"; let mut i = 0; while i < 2000 { Map.insert<String, Int>(map: mut table, key, value: i); i = i + 1 }; Output.write(message: String.from_int(value: Map.len<String, Int>(map: table))); return Unit }",
    ),
    (
        "string-keyed-set-insert.rss",
        "fn main() -> Unit { local seen = Set<String>.new(); let tag = \"fixed-tag\"; let mut i = 0; while i < 2000 { let added = Set.insert<String>(set: mut seen, value: tag); i = i + 1 }; Output.write(message: String.from_int(value: Set.len<String>(set: seen))); return Unit }",
    ),
    (
        "long-string-keyed-map-insert.rss",
        "fn main() -> Unit { local table = Map<String, Int>.new(); let key = \"0123456789012345678901234567890123456789012345678901234567890123456789\"; let mut i = 0; while i < 2000 { Map.insert<String, Int>(map: mut table, key, value: i); i = i + 1 }; Output.write(message: String.from_int(value: Map.len<String, Int>(map: table))); return Unit }",
    ),
];

#[test]
fn native_key_hash_work_matches_the_interpreter_under_an_armed_budget() {
    for (name, source) in KEY_HASH_PARITY_CASES {
        for eager_osr in [false, true] {
            let mut regions = 0_u64;
            for &budget in STEP_PARITY_BUDGETS {
                let limits = RunLimits::unbounded_for_trusted_host().with_step_budget(budget);
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
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must terminate for the same reason as the interpreter"
                );
                assert_eq!(
                    native.usage.steps_consumed, interpreter.usage.steps_consumed,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must report the interpreter's step count"
                );
                assert_eq!(
                    native.stdout, interpreter.stdout,
                    "{name} at step budget {budget} (eager_osr={eager_osr}) must produce the interpreter's output"
                );
                regions = regions.saturating_add(native_region_entries(&native));
            }
            // Without this the test would pass by declining the region, which is
            // exactly what this change retires.
            assert!(
                regions > 0,
                "{name} (eager_osr={eager_osr}) must reach generated code for its step counts to mean anything"
            );
        }
    }
}

#[test]
fn an_unarmed_key_hash_loop_reports_the_interpreter_step_count() {
    // `charge_work` charges nothing with no control armed, so a natively executed
    // key-hash loop must not bill the hash either — applying the charge
    // unconditionally would *over*-report by one unit per insert or more.
    for (name, source) in KEY_HASH_PARITY_CASES {
        for eager_osr in [false, true] {
            let (interpreter, native) = accounting_pair(
                name,
                source,
                RunLimits::unbounded_for_trusted_host(),
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
                "{name} (eager_osr={eager_osr}) must terminate like the interpreter with nothing armed"
            );
            assert_eq!(
                native.usage.steps_consumed, interpreter.usage.steps_consumed,
                "{name} (eager_osr={eager_osr}) must report the interpreter's step count with nothing armed"
            );
            assert_eq!(
                native.usage.intrinsic_calls, interpreter.usage.intrinsic_calls,
                "{name} (eager_osr={eager_osr}) must report the interpreter's intrinsic count with nothing armed"
            );
            assert!(
                native_region_entries(&native) > 0,
                "{name} (eager_osr={eager_osr}) must reach generated code with nothing armed too"
            );
        }
    }
}
