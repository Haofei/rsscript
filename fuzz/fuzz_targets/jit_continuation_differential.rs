#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript_sdk::{
    artifact::ArtifactVerifier,
    compile::Compiler,
    experimental::native_jit::{NativeCostModel, NativeJitOptions},
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime},
};

fn source(shape: u8, limit: u16) -> String {
    match shape % 3 {
        0 => format!(
            "struct B {{ value: Int }} fn boundary(value: Int) -> Int {{ let b = B(value: value); return b.value }} fn main() -> Int {{ let mut i = 0; let mut total = 0; while i < {limit} {{ let a = i * 3 + 1; total = total + boundary(value: a); i = i + 1 }}; return total }}"
        ),
        1 => format!(
            "struct B {{ value: Int }} fn boundary(value: Int) -> Int {{ let b = B(value: value); return b.value }} fn main() -> Int {{ let seed = boundary(value: 7); let mut i = 0; let mut total = seed; while i < {limit} {{ if i % 3 == 0 {{ total = total + i * 2 }} else {{ total = total - 1 }}; i = i + 1 }}; return total }}"
        ),
        _ => format!(
            "struct B {{ value: Int }} fn main() -> Int {{ let b = B(value: 9); let mut i = 0; let mut total = b.value; while i < {limit} {{ total = total + i * 5 - i / 2; i = i + 1 }}; return total }}"
        ),
    }
}

fuzz_target!(|data: &[u8]| {
    let shape = data.first().copied().unwrap_or(0);
    let raw_limit = u16::from_le_bytes([
        data.get(1).copied().unwrap_or(0),
        data.get(2).copied().unwrap_or(0),
    ]);
    let limit = 16 + raw_limit % 257;
    let source = source(shape, limit);
    let Ok(built) = Compiler.compile("continuation-fuzz.rss", &source) else {
        return;
    };
    let Ok(verified) = ArtifactVerifier.verify(built) else {
        return;
    };
    let admitted = verified.admit_trusted_input();
    let Ok(linked) = Runtime::new(ProviderRegistry::default()).link(&admitted) else {
        return;
    };

    let complete = linked.execute(
        ExecutionRequest::new(std::iter::empty::<String>())
            .limits(RunLimits::unbounded_for_trusted_host()),
    );
    let completed_steps = complete.usage.steps_consumed;
    let requested_budget = u64::from_le_bytes([
        data.get(3).copied().unwrap_or(0),
        data.get(4).copied().unwrap_or(0),
        data.get(5).copied().unwrap_or(0),
        data.get(6).copied().unwrap_or(0),
        0,
        0,
        0,
        0,
    ]);
    let step_budget = requested_budget % completed_steps.saturating_add(2).max(1);
    let limits = RunLimits::bounded().with_step_budget(step_budget);
    let request = || ExecutionRequest::new(std::iter::empty::<String>()).limits(limits.clone());
    let interpreted = linked.execute(request());
    let native = linked.execute(request().native_jit(NativeJitOptions {
        cost_model: NativeCostModel::Report,
        ..NativeJitOptions::default()
    }));

    assert_eq!(native.outcome(), interpreted.outcome());
    assert_eq!(
        native.usage.steps_consumed,
        interpreted.usage.steps_consumed
    );
    assert_eq!(native.stdout, interpreted.stdout);
    assert_eq!(native.stderr, interpreted.stderr);
    assert_eq!(
        native.provider_call_traces,
        interpreted.provider_call_traces
    );
    assert_eq!(
        native.usage.resources_live_at_return,
        interpreted.usage.resources_live_at_return
    );
});
