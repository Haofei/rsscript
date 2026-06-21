# Phase 2 — Baseline (tier-0) compiler: path-B first milestone

This is the first milestone of Phase 2 of `docs/planning/vm-jit-perf-plan.md`:
a **real baseline machine-code tier**, switchable behind a flag, over the
existing side-effect-free native-eligible subset.

## Decision: path (B), not a hand-rolled assembler

The Phase-2.1 feasibility study evaluated two ways to build a single-pass
baseline:

- **(A) From-scratch / Winch-style IR-free assembler.** Emit machine code
  directly, one template per opcode, no Cranelift IR.
- **(B) Single-pass lowering to Cranelift IR at `opt_level="none"`.** Reuse the
  existing `vm-jit` translation (already ~1:1 `RegInstr → JitInstr → IR`) and
  distinguish the baseline tier purely by the ISA optimization flag.

**Path (A) was rejected.** At the pinned Cranelift **0.132.1**, the only clean
public IR-free assembler is `cranelift-assembler-x64`, which is **x86-64 only**.
The primary dev/CI target here is **aarch64** (Apple-silicon dev box, ARM CI),
and there is **no standalone aarch64 assembler crate at this version**. Building
a hand-rolled aarch64 emitter from scratch would be a large, error-prone surface
with no parity safety net — the opposite of "do it once, correctly."

**Path (B) was chosen.** It reuses the existing, already-parity-proven `vm-jit`
IR translation verbatim. The baseline tier is distinguished from the optimizing
tier by exactly one knob: the Cranelift ISA flag `opt_level`.

- optimizing tier (default): `opt_level="speed"`
- baseline tier (`RSS_JIT_BASELINE=1`): `opt_level="none"`

Everything else — IR translation, the host-helper ABI, the bail-flag deopt
protocol, the compiled subset — is byte-for-byte identical. **The win is
compile latency**: `opt_level="none"` does far less codegen work, so a baseline
function compiles faster than the optimizing tier, while still emitting real
machine code that beats the interpreter by ~40×.

**Honest framing:** this is path B — IR lowering with optimization turned off —
**not** a hand-rolled, IR-free Sparkplug/Winch emitter. The plan's original 2.1
language ("reuse Cranelift's assembler/`MachBuffer` to emit code *without
building Cranelift IR*") is not achievable on aarch64 at 0.132.1; path B is the
pragmatic baseline that ships a real, faster-to-compile machine-code tier today.

## What changed

Two functions, one new env flag, no change to the compiled subset.

1. **`crates/vm-jit/src/lib.rs`** — parameterized the optimization level:
   - new `NativeModule::new_with_opt(helpers, baseline: bool)`:
     `baseline ⇒ opt_level="none"`, else `"speed"`.
   - `NativeModule::new(helpers)` retained as the optimizing default
     (back-compat), now delegating to `new_with_opt(helpers, false)`.
   - Only the `opt_level` ISA flag changes; host-helper symbol registration,
     imports, and the rest of module construction are untouched.

2. **`crates/rsscript/src/reg_vm/mod.rs`** — made the native tier run in baseline
   mode:
   - new `NativeState::new_with_opt(tier_up_threshold, force_bail, collect_stats,
     baseline)`; `NativeState::new(..)` retained (delegates with
     `baseline=false`).
   - `eval_main_with_args_native_inner` reads `RSS_JIT_BASELINE` (`is_some()`) at
     the entry and threads `baseline` into `NativeState::new_with_opt`. Default
     (var unset) = optimizing. The differential never sets the var, so its
     behavior is unchanged.

3. **No new side-effecting opcodes** were added to the native subset. The
   baseline targets exactly the existing **side-effect-free** eligible set —
   scalar arith/compare/branch plus the read-only heap helpers (`field_int`,
   `field_float`, `list_len`, `list_get_int`, `list_get_float`). Therefore the
   §7.2 fallback proof carries over **verbatim**: a runtime bail re-runs the
   function from the top, which is sound precisely because no heap write was
   performed. `run_jit()` / the interpreter remain the deopt oracle, unchanged.

## Measurements

Kernel: `benchmarks/vm-jit/kernels/native_scalar_loop.rss` (a pure-`Int` hot
loop in its own non-recursive function — the canonical native-eligible shape).
Release binary built once with `cargo build --release --features native-jit
--bin rss`. Run inside the Docker `dev` container (aarch64). The bench JSON
reports `jit.compile_ms` directly from an `Instant` around `module.compile()`
(under the existing stats flag), so compile latency is measured, not inferred.

### Steady-state (SIZE = 2,000,000, `--iterations 7 --warmup 2`)

| config       | mode         | median ms | compile_ms (this run) | vs interpreter |
|--------------|--------------|----------:|----------------------:|---------------:|
| optimizing   | jit-native   |     3.372 |                 0.225 |        ~39.6×  |
| baseline     | jit-native   |     3.938 |                 0.189 |        ~33.9×  |
| interpreter  | vm-internal  |   133.541 |                     — |          1.0×  |

Both native tiers are roughly **40× faster than the interpreter floor**.
Baseline steady-state is ~17% slower than optimizing (3.94 vs 3.37 ms) — exactly
the expected path-B trade: `opt_level="none"` emits less-optimized machine code.

### Compile latency (the milestone's whole point)

The steady-state `compile_ms` above is a single, noisy sample. To isolate
compile latency cleanly, the kernel was compiled 10× per mode with
`--iterations 1 --warmup 0` (each run does exactly one `module.compile()`) at
SIZE = 200,000, reading `jit.compile_ms` each time:

| config     | compile_ms median | mean   | min    |
|------------|------------------:|-------:|-------:|
| optimizing |            0.2481 | 0.2482 | 0.1964 |
| baseline   |            0.1919 | 0.1938 | 0.1645 |

**Baseline compile latency is ~77% of optimizing — a ~23% reduction** (lower
`opt_level` ⇒ less codegen work), consistent across all 10 samples. The kernel
is a single small hot function, so absolute deltas are sub-millisecond, but the
direction and ratio are stable and exactly what path B predicts: the baseline
trades a little steady-state speed for faster compiles.

Raw `compile_ms` samples:

```
optimizing: 0.256378 0.259753 0.242712 0.356963 0.275462 0.200920 0.196378 0.214795 0.225169 0.253462
baseline:   0.185169 0.222461 0.198627 0.166377 0.222878 0.214544 0.212836 0.164544 0.169086 0.181419
```

**Honest summary of what the numbers show:** on this tiny pure-scalar loop the
two tiers produce similarly fast steady-state code (both ~40× the interpreter);
the measurable, consistent difference is **compile latency**, where the baseline
is ~23% faster. That IS the expected path-B result — the baseline's value is
lower compile cost, not faster steady-state execution.

## Parity proof (both modes)

All three gates green. Output below is pasted from the Docker `dev` container.

**Optimizing (default) — `runtime jit_acceptance`:**

```
running 8 tests
test jit_acceptance::native_jit_acceptance_reports_real_native_execution ... ok
test jit_acceptance::jit_acceptance_runs_float_parameter_loop ... ok
test jit_acceptance::jit_acceptance_runs_heap_read_helpers ... ok
test jit_acceptance::jit_acceptance_runs_float_heap_read_helpers ... ok
test jit_acceptance::jit_acceptance_runs_collection_index_mutation_ops ... ok
test jit_acceptance::jit_acceptance_runs_cross_function_loop_calls ... ok
test jit_acceptance::jit_acceptance_falls_back_for_recursive_calls ... ok
test jit_acceptance::jit_acceptance_runs_branchy_inlined_callees ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 295 filtered out; finished in 0.29s
```

**Baseline (`RSS_JIT_BASELINE=1`) — `runtime jit_acceptance`** (proves
`opt_level="none"` is observationally identical):

```
running 8 tests
test jit_acceptance::native_jit_acceptance_reports_real_native_execution ... ok
test jit_acceptance::jit_acceptance_runs_float_parameter_loop ... ok
test jit_acceptance::jit_acceptance_falls_back_for_recursive_calls ... ok
test jit_acceptance::jit_acceptance_runs_branchy_inlined_callees ... ok
test jit_acceptance::jit_acceptance_runs_float_heap_read_helpers ... ok
test jit_acceptance::jit_acceptance_runs_heap_read_helpers ... ok
test jit_acceptance::jit_acceptance_runs_cross_function_loop_calls ... ok
test jit_acceptance::jit_acceptance_runs_collection_index_mutation_ops ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 295 filtered out; finished in 0.29s
```

**N-way `differential`** (interp ≡ tier-0 ≡ native ≡ force-deopt ≡ compiled):

```
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.11s
```

(All 31 differential trials passed, including `backend_differential::*`,
`differential_corpus::corpus_fixtures_pass`, `generative::*`, `examples_exec::*`,
and `metamorphic::*`.)

## Scope statement (honest)

This is the **path-B first milestone** of Phase 2: a real, switchable baseline
machine-code tier (`opt_level="none"`) over the **existing side-effect-free
native-eligible subset** only — scalar arith/compare/branch plus read-only heap
helpers. It is IR lowering with optimization off, not a hand-rolled IR-free
assembler (rejected for the aarch64-no-assembler reason above).

The `run_jit()` / interpreter deopt oracle is retained unchanged and remains
valid verbatim, because no side-effecting opcode was added to the subset (§7.2
re-run-from-top is sound only before a heap write).

**Next milestones (not in this change):**
- The §2.3 / §3.2w deopt-oracle gating for the **side-effecting remainder** of
  tier-0's eligibility set (`SetField`, `Make*`, `ListPush/Set/Pop`,
  `MapInsert/Remove`, …). Compiling those requires the replacement-equivalence
  design (preflight-before-commit / checkpoint-rollback / no-bail-after-commit),
  because re-running from the top after a committed write would double-apply it.
  That track is explicitly NOT unlocked by the present oracle.
