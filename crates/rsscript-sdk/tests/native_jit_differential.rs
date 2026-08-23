use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    operation::{CancellationToken, MonotonicDeadline},
    provider_api::{
        BlockingBehavior, CancellationBehavior, DataEffect, ExternalSymbol, FunctionSignature,
        ParameterSignature, ProviderCallMode, ProviderDescriptor, ProviderError,
        ProviderErrorMapping, ProviderFunction, ProviderFunctionDescriptor, ProviderRegistry,
        RUNTIME_ABI_VERSION, ResourceCleanupContract, WireInterpreterFn, WireValue,
    },
    report::{ExecutionEngineTelemetry, TerminationReason},
    runtime::{
        ExecutionRequest, NativeCostModel, NativeJitOptions, RunLimits, Runtime, TracePolicy,
    },
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
fn cancellation_during_a_closed_native_continuation_is_observed() {
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
            .native_jit(NativeJitOptions::default()),
    );
    canceller.join().expect("cancellation thread completes");
    assert_eq!(report.termination_reason(), TerminationReason::Cancelled);
    let ExecutionEngineTelemetry::Native {
        continuation_compiled_source_instructions,
        ..
    } = report.telemetry.engine
    else {
        panic!("mid-region cancellation must report native telemetry");
    };
    assert!(continuation_compiled_source_instructions > 0);
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
                ..NativeJitOptions::default()
            }),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(native.outcome(), interpreter.outcome());
    assert_eq!(
        native.usage.steps_consumed,
        interpreter.usage.steps_consumed
    );
    assert_eq!(native.provider_call_traces.len(), 1);
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
