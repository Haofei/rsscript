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
Wall-clock samples use production native options with telemetry disabled. After
timing completes, the harness performs one separate diagnostic native execution
to collect compile, code-size, transition, bail, helper, and optimization evidence.
Diagnostic instrumentation is therefore never included in `native_ns`.
It also emits one `rsscript.aot_jit_matrix.v1` record, validated against
`aot-jit-matrix.schema.json`. The matrix records semantic parity, execution and
compile time, transition counts, and explicit `null` values for metrics the
current engine cannot report. The Core harness marks AOT as `not_measured`:
the Rust/AOT backend lives in the experiments workspace and must contribute a
measurement from its own controlled runner before any AOT/JIT performance
comparison is claimed. `not_measured` is evidence of a missing measurement,
not a zero-cost or unsupported engine.

The experiments workspace owns the slow, ignored cross-engine harness that
turns the AOT cell into a real measurement without creating a Core-to-AOT
dependency:

```bash
cargo test --locked --release --manifest-path experiments/Cargo.toml \
  -p rsscript-aot-backend --test aot_jit_matrix -- --ignored --nocapture
```

It builds the generated Rust package in an isolated target directory and
alternates interpreter, JIT, and AOT samples. AOT execution includes process
startup and records that measurement mode in its reason field. Scheduled CI
publishes this evidence; promotion decisions still require a pinned,
controlled-hardware run rather than GitHub-hosted timing alone.

`mixed-mode-continuation` is the canonical barrier workload: it repeatedly
executes scalar regions around an interpreter-owned aggregate boundary and guards
against admitting region transitions that do not beat the interpreter end to end.
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
struct scalar-replacement implementation is isolated behind
`jit-struct-sr-experimental` after the canonical case failed to establish stable
native entry and end-to-end benefit. The ordinary SDK `native-jit` feature
compiles none of those research implementations.

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
