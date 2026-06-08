# VM JIT roadmap

Goal: a JIT for the register VM with **no gap** between the JIT, the interpreter,
and the compiled (Rust-lowering) backend. Built verification-first.

## Invariants (hold at every tier)

1. **Single source of semantic truth.** The JIT reuses the interpreter's value
   representation (`VmValue`) and runtime intrinsics. It generates code for hot
   control-flow / arithmetic and *calls back* into the shared runtime for
   everything else — it never reimplements semantics.
2. **Fallback = correctness floor.** Anything the JIT does not fully support
   runs on the interpreter (`run_frame`). Unsupported features cannot create a
   gap; only what the JIT *does* compile can have bugs — and that is exactly what
   the differential targets.
3. **N-way differential as the gate.** `interp ≡ jit ≡ compiled` on the curated
   corpus and on generated programs (`tests/common/differential.rs`,
   `tests/backend_differential.rs`). Every tier must keep it green.
4. **Converging coverage.** What the JIT covers vs falls back is reported
   (`RegVmExecutable::jit_plan`); unsupported instructions must be a documented,
   shrinking set, never a silent divergence.
5. **Determinism.** Generated programs exclude/seed all nondeterminism (float
   NaN/ordering, map iteration order, time/random/env, scheduling, div-by-zero)
   so divergence is always a real bug.

## Status

- **Phase 0 — N-way differential framework.** Done. `Backend` trait
  (interp/jit/compiled), `assert_backends_agree`, generative differential.
- **Phase 1 — tier-0 JIT (specializing executor).** Done.
  `RegVm::run_jit` executes JIT-eligible functions (the numeric/control core)
  via a specializing loop that reuses the interpreter's exact value/register
  semantics (`eval_numeric_binary`/`eval_numeric_compare`/`reg`/`set_reg`/…), so
  it is gap-free by construction. Integrated into `drive`'s frame loop with
  per-function fallback to the interpreter for anything outside the subset.
  Exercised by the N-way differential (generative arithmetic/comparisons/branches
  + hand-written params/loops). The pure subset now has a single shared
  implementation (`try_exec_pure`), and coverage spans heap/field/match,
  collection get/set, and cross-function calls.
- **Phase 2 — native (Cranelift) codegen.** Done — see below (`vm-jit` crate,
  `native-jit` feature). ~64× faster than the interpreter on numeric kernels.
- **Phase 3 — tiering / deopt / fuzz.** Done (baseline) — see below.

## Phase 2 — native baseline codegen — **DONE** (`vm-jit` crate, `native-jit` feature)

A native tier lives in the **separate `vm-jit` crate**: the `rsscript` crate is
`#![forbid(unsafe_code)]`, and executing generated machine code + calling function
pointers requires `unsafe`, so it lives there behind a safe API
(`NativeModule::call`).

- **Codegen:** Cranelift (`cranelift-jit`/`-frontend`/`-codegen`/`-native`),
  `opt_level=speed`. ✓
- **Bytecode ABI:** the JIT crate defines its own stable, versioned IR
  (`JitInstr`/`JitFunction`, `vm_jit::IR_VERSION`); `rsscript` translates eligible
  `RegFunction`s into it rather than exposing its private `RegInstr` — cleaner
  decoupling than leaking internals. ✓
- **Value glue:** `Int`/`Bool` unbox into `i64` registers (Bool as `0`/`1`); the
  result boxes back as `Int`. Native runs only when every argument is an `Int`, so
  all registers are statically `i64`. Non-core ops (heap/`Float`/calls) are not
  reimplemented — the function **bails to the interpreter** (gap-free because the
  compiled subset is side-effect-free, so re-running is observationally identical).
  Choosing bail-to-interpreter over `extern "C"` helper calls keeps the no-gap
  guarantee trivial. ✓
- **Per-function dispatch:** native-eligible functions compile; everything else
  falls back (invariant 2). ✓
- **Gate:** `NativeJit` (and `NativeJitForceDeopt`) differential backends;
  force-JIT CI mode = `cargo test --features native-jit`; verified across the full
  curated corpus + the generative differential. ✓

## Phase 3 — tiering, deopt, optimization — **DONE** (baseline)

- **Tiering:** a per-function hot-call counter (`tier_up_threshold`) defers native
  compilation until a function is hot. ✓ OSR is **not applicable** to this
  method-at-a-time JIT (whole functions recompile and re-enter fresh; there is no
  mid-loop on-stack replacement to perform).
- **Deopt:** native code bails at every arithmetic guard (overflow, divide/modulo
  by zero, out-of-range shift) and the interpreter re-runs with the original args
  (exact, trivial state reconstruction). **Tested by deopting at every guard:** a
  `force-deopt` backend (native always bails) is in the differential, so
  `{interp, tier0, native, force-deopt, compiled}` all agree. ✓
- **Optimizations:** Cranelift's `opt_level=speed` provides the baseline
  optimizer. Source-level constant folding / intrinsic inlining can be layered on
  later, each behind the differential gate.
- **Coverage-guided fuzz:** a total `seed(bytes) -> program` decoder
  (`program_from_seed`) drives the N-way differential via proptest seeds and
  shrinking. ✓ Wiring it to a coverage-guided engine (cargo-fuzz / libFuzzer) is
  the deployment step; the decoder is total, as such engines require.

## Don't

- Don't reimplement instruction semantics in the JIT (breaks invariant 1).
- Don't add an optimization without the N-way differential + generative fuzz
  passing.
- Don't widen the JIT-supported instruction set without extending the generator
  to exercise it.
