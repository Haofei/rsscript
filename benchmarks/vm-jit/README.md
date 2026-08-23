# Native JIT workload scorecard

This directory contains the representative workloads used to decide whether a
native-JIT optimization remains in RSScript's trusted in-process engine. The
interpreter is always the semantic oracle; a speedup never excuses an observable
behavior difference.

## Supported checks

The pull-request gate is intentionally small and stable:

```sh
cargo test --locked --release -p rsscript-sdk --features native-jit \
  --test native_jit_smoke native_hot_loop_release_gate_beats_the_interpreter \
  -- --nocapture
```

The weekly hardening workflow runs the broader differential corpus and the
ignored scorecard:

```sh
cargo test --locked -p rsscript-sdk --features native-jit \
  --test native_jit_differential

cargo test --locked --release -p rsscript-sdk --features native-jit \
  --test native_jit_scorecard -- --ignored --nocapture
```

The scorecard emits one `rsscript.native_jit_scorecard.v1` JSON object per case.
It records interpreter/native wall time, compile time, machine-code residency,
arena reservation, native calls, bails, and OSR entries. CI logs are the history;
the repository does not retain ad-hoc developer snapshots.

## Retention rule

A complex optimization remains in the stable SDK path only when at least one
canonical compiler workload shows a repeatable 10–15% end-to-end improvement
without increasing semantic differences, unsafe surface, or unbounded resource
use. Microbenchmark-only wins are insufficient.

Current evidence keeps baseline scalar loops, native call chains, and the shared
Option/Result/Variant scalar-replacement path. Profile-guided closure PIC and
branch-side-exit speculation are isolated behind the VM-only `jit-speculation`
feature. Native recursion is isolated behind `jit-recursion-experimental`, and
loop-invariant helper memoization behind `jit-memoization-experimental`. The
ordinary SDK `native-jit` feature compiles none of those research implementations.

## Adding a workload

1. Add a focused `.rss` file under `kernels/`.
2. Add it to `tests/native_jit_scorecard.rs` in `rsscript-sdk`.
3. Add semantic differential coverage when the workload exercises a new value,
   deoptimization, mutation, resource, or control-flow shape.
4. Run the scorecard in a controlled release build and retain the CI artifact.

Checked-in canonical baselines, when established on controlled CI hardware, must
use `baseline/canonical-<os>-<arch>.json` and conform to
`baseline/schema.json`. They must include commit, CPU, OS, Rust/Cranelift version,
build profile, warmup/sample counts, and fixture digests.
