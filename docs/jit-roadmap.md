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
  Exercised by the 3-way differential (generative arithmetic/comparisons/branches
  + hand-written params/loops). No native code yet; eligible heap/field/match
  instructions and calls are the next coverage step (prefer extracting a shared
  `exec_instr` over duplicating arms, to keep gap-freeness structural).

## Phase 2 — native baseline codegen

A native tier must live in a **separate crate** (e.g. `vm-jit`): the `rsscript`
crate is `#![forbid(unsafe_code)]`, and executing generated machine code +
calling function pointers requires `unsafe` (mirror the `native-abi` `host`
feature pattern).

- **Codegen:** Cranelift (`cranelift-jit`/`-frontend`/`-codegen`) — mature, fast
  compile, Rust-native. (LLVM is heavier; a copy-and-patch/template JIT is an
  even lighter first step.)
- **Bytecode ABI:** expose the `RegFunction`/`RegInstr` surface (currently
  private) as a stable, versioned IR the JIT crate consumes.
- **Value glue:** `VmValue` is `Rc`-heavy and tagged, so only `Int`/`Float`/
  `Bool` unbox cleanly into registers. The JIT keeps locals as `VmValue` in the
  register file and emits `extern "C"` calls to shared runtime helpers for
  construction/inspection and all non-numeric ops. Pure numeric/control regions
  run native; the rest are calls — still removes interpreter dispatch overhead.
- **Per-function dispatch:** compile `jit_plan().eligible_functions`; everything
  else falls back (invariant 2).
- **Gate:** add the real `Jit` backend, force-JIT CI mode, run the full corpus +
  generative differential three-way.

## Phase 3 — tiering, deopt, optimization

- **Tiering / OSR:** hot-loop counters trigger tier-up; on-stack replacement to
  enter/leave compiled code mid-loop.
- **Deopt:** type guards that bail to the interpreter with exact state
  reconstruction. The highest-risk part — test by **deopting at every guard** and
  comparing to the interpreter (`{jit-off, tier0, tier1, force-deopt}` must all
  agree).
- **Optimizations:** constant folding, intrinsic inlining, register allocation —
  one at a time, each behind the differential gate.
- **Coverage-guided fuzz:** a `seed(bytes) -> program` decoder + mutators with
  coverage feedback (Fuzzilli-style), run nightly, to reach deep tiering/deopt
  paths. Generators should deliberately emit hot loops, megamorphic calls,
  guard-failing inputs, and high register pressure.

## Don't

- Don't reimplement instruction semantics in the JIT (breaks invariant 1).
- Don't add an optimization without the N-way differential + generative fuzz
  passing.
- Don't widen the JIT-supported instruction set without extending the generator
  to exercise it.
