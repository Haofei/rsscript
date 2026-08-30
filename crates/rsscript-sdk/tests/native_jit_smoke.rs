use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    experimental::native_jit::NativeJitOptions,
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime},
};
use std::time::Instant;

const HOT_LOOP: &str = r#"
fn hot(limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + index * 3 - index / 2 + 7
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    Output.write(message: String.from_int(value: hot(limit: 200000)))
    return Unit
}
"#;

#[test]
fn phase_typed_sdk_can_select_trusted_native_execution() {
    let built = Compiler
        .compile("native-sdk-smoke.rss", HOT_LOOP)
        .expect("hot-loop source should compile");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("artifact should verify")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("artifact should link");

    let reference = linked.execute(ExecutionRequest::default());
    let native = linked.execute(
        ExecutionRequest::default()
            .limits(RunLimits::unbounded_for_trusted_host())
            .native_jit(NativeJitOptions::diagnostic()),
    );

    assert_eq!(native.outcome(), reference.outcome());
    assert_eq!(native.stdout, reference.stdout);
    assert_eq!(native.stderr, reference.stderr);
    let rsscript_sdk::report::ExecutionEngineTelemetry::Native(engine) = &native.telemetry.engine
    else {
        panic!("trusted native execution must report native telemetry");
    };
    assert!(engine.resident_code_bytes > 0);
    assert_eq!(engine.resident_code_bytes, engine.published_code_bytes);
    assert_eq!(engine.rejected_resident_bytes, 0);
    assert!(engine.reserved_arena_bytes >= engine.resident_code_bytes);
}

#[test]
fn native_hot_loop_release_gate_beats_the_interpreter() {
    let built = Compiler
        .compile("native-perf.rss", HOT_LOOP)
        .expect("hot-loop source should compile");
    let admitted = ArtifactVerifier
        .verify(built)
        .expect("artifact should verify")
        .admit_trusted_input();
    let linked = Runtime::new(ProviderRegistry::default())
        .link(&admitted)
        .expect("artifact should link");

    // Debug Cranelift deliberately optimizes for compiler iteration speed. It
    // still exercises the native path above, but wall-clock gating is meaningful
    // only for the shipped release profile.
    if cfg!(debug_assertions) {
        return;
    }

    let mut interpreter = Vec::new();
    let mut native = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let reference = linked.execute(ExecutionRequest::default());
        assert!(matches!(
            reference.outcome(),
            rsscript_sdk::ExecutionOutcome::Completed { .. }
        ));
        interpreter.push(started.elapsed());

        let started = Instant::now();
        let report = linked.execute(
            ExecutionRequest::default()
                .limits(RunLimits::unbounded_for_trusted_host())
                .native_jit(NativeJitOptions::default()),
        );
        assert!(matches!(
            report.outcome(),
            rsscript_sdk::ExecutionOutcome::Completed { .. }
        ));
        native.push(started.elapsed());
    }
    interpreter.sort_unstable();
    native.sort_unstable();
    let interpreter_median = interpreter[1];
    let native_median = native[1];
    eprintln!("native JIT smoke: interpreter={interpreter_median:?} native={native_median:?}");
    assert!(
        native_median * 2 < interpreter_median,
        "the scalar native tier must retain a conservative 2x release win: interpreter={interpreter_median:?}, native={native_median:?}"
    );
}
