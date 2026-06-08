# JIT TODO

Goal: a **high-performance** JIT for the register VM with **no gap** between the
VM interpreter, the JIT, and the compiled (Rust-lowering) backend.

Design invariants (must hold at every step): see `docs/jit-roadmap.md`.
Status legend: `[x]` done · `[ ]` todo · `[~]` in progress.

## What kind of JIT is this?

**Method-based (per-function) two-tier baseline JIT — not a tracing JIT.**

- **Per-function, not tracing:** it compiles whole functions, deciding eligibility
  up front. A tracing JIT instead records hot linear execution *traces* (across
  call/loop boundaries) and compiles those with guards; we don't do that.
- **Tier-0 = specializing executor:** executes the supported instruction subset
  via a reduced-dispatch loop (`RegVm::run_jit`) that reuses the interpreter's
  exact semantics — in fact the *same* per-instruction code, via the shared
  `RegVm::try_exec_pure` dispatcher — with per-function fallback. Gap-free by
  construction; a modest speedup (no native code, values still boxed `VmValue`).
- **Tier-1 = native method JIT (`vm-jit` crate, `native-jit` feature):** Cranelift
  codegen with unboxed `i64` registers for the integer/boolean/control core. Hot
  functions tier up to machine code; arithmetic guards deopt (bail) to the
  interpreter, and anything outside the core falls back per-function. ~64× faster
  than the interpreter on numeric kernels. Tracing could be a later
  alternative/addition, but the method JIT is the chosen path.

## Done

- [x] **N-way differential framework** — `Backend` trait (interpreter / jit /
  compiled) + `assert_backends_agree` (`tests/common/differential.rs`).
- [x] **Generative differential** — random `let`-chain + `if`-guard integer
  programs, run interp == jit == compiled (`tests/backend_differential.rs`).
- [x] **Parity suites are 3-way** — `assert_vm_eval_matches_backend` now also runs
  the JIT, so every curated parity fixture verifies interp == jit == compiled.
- [x] **Tier-0 specializing executor** — `RegVm::run_jit`, integrated into
  `drive`, reuses the interpreter's exact helpers/registers (gap-free by
  construction), per-function fallback to the interpreter.
- [x] **Eligibility analysis** — `jit_supported_instruction` + `RegVmExecutable::jit_plan`.
- [x] **Covered instruction subset (tier-0):** loads (unit/int/float/bool/string),
  move, deep-copy, manage, int arithmetic/bitwise/shift, int comparisons,
  equal/not-equal, jumps (uncond / if-bool / if-int-compare), get/set field,
  make struct/variant/list/object/map, option/result (make-some, load-none,
  unwrap-some, unwrap-variant-value), match (option/result/variant/map-get),
  make-closure, runtime-error, return, **collection get/set/index ops** (List
  get/len/push/append/clear/pop/remove-at/set, Map get/clear/insert/insert-old/
  remove), and **cross-function `CallKnown`** (to non-suspending, non-recursive
  callees).
- [x] **Single copy of the pure subset:** `RegVm::try_exec_pure` is the one
  implementation of every pure instruction; both `drive` (interpreter) and
  `run_jit` (tier-0) call it, so gap-freeness is structural, not just
  differential-checked.
- [x] **Cross-function eligibility analysis** — `compute_jit_eligibility`: a
  unit-wide fixpoint marking a function eligible iff it is *non-suspending* (every
  instruction pure-subset or a call to another eligible function — so no
  await/spawn/blocking is reachable) and *non-recursive* (its eligible call graph
  is acyclic, since the executor runs callees on the host stack).
- [x] **Option/Result/match coverage** verified three-way across the parity suite.
- [x] **Out-of-range int literal rejected at the frontend** (RS0033) — keeps the
  three backends consistent.

## To do

### Coverage (make the JIT run more of the language)
- [x] Eliminate the run_jit ↔ drive duplication: extract a shared
  `try_exec_pure(&self, instr, base, &mut ip) -> PureStep` used by both. The pure
  subset now has one implementation; gap-freeness is structural, not just
  differential-checked.
- [x] Collection get/set + index ops (List/Map get/set) in the eligible subset
  (closure-free ops only; map/filter/fold/sort-by still fall back). Verified
  three-way by `backends_agree_on_collection_ops` + `jit_plan` eligibility.
- [x] `Match*` (option/result/variant/map-get) in the eligible subset.
- [x] Cross-function: JIT-compiled code calls other functions. `run_jit` drives a
  `CallKnown` callee to completion via `run_frame`; eligibility
  (`compute_jit_eligibility`) restricts this to non-suspending + non-recursive
  call graphs so it can never suspend or overflow the host stack where the
  stackless interpreter would not. Verified by `backends_agree_on_cross_function_calls`,
  the recursion-fallback `jit_plan` test, and the whole parity corpus under
  force-all JIT.
- [x] Float / string ops parity coverage in the generator
  (`backends_agree_on_float_programs`, `backends_agree_on_string_programs`):
  division-free float arithmetic + comparisons (result reduced to an `Int` so float
  *formatting* isn't the variable under test) and `String.concat`/`String.len`
  chains, all three-way. Bytes ops can follow the same pattern next.

### High performance (the actual JIT) — native (Cranelift) tier
Built behind the `native-jit` cargo feature (off by default, so normal builds
don't pull in the codegen dependency). Enable with `--features native-jit`.
- [x] New `vm-jit` crate (separate, because `rsscript` is `#![forbid(unsafe_code)]`).
  The only `unsafe` is the call through a code pointer the crate itself emitted,
  behind the **safe** `NativeModule::call`.
- [x] Stable, versioned IR for the crate. Rather than leak `rsscript`'s private
  `RegInstr`, the JIT crate defines its own `JitInstr`/`JitFunction`
  (`vm_jit::IR_VERSION`); `rsscript` translates eligible functions into it. This
  decouples the producer from the codegen.
- [x] Cranelift native code generation for the integer/boolean/control core;
  per-function fallback for everything else. Non-trivial ops aren't reimplemented
  in native code — the function **bails to the interpreter** (the single source of
  truth) instead, which is gap-free because the compiled subset is side-effect-free.
- [x] `VmValue` ABI for native code: `Int`/`Bool` unbox into `i64` registers,
  the result boxes back as `Int`. Native runs only when every argument is an
  `Int`, so all registers are statically `i64` (`Float`/heap stay on fallback).
- [x] Tiering: a per-function hot-call counter (`tier_up_threshold`) defers
  native compilation until a function is hot. (OSR is not applicable to this
  method-at-a-time JIT — whole functions are (re)compiled and re-entered fresh;
  there is no mid-loop on-stack replacement to do.)
- [x] Deopt: native code bails at each arithmetic guard (overflow, divide/modulo
  by zero, out-of-range shift); the interpreter then re-runs with the original
  args (exact, trivial state reconstruction since the subset is side-effect-free).
  **Tested by deopting at every guard:** a `force-deopt` mode (native always
  bails) is a differential backend, so `{interp, tier0, native, force-deopt,
  compiled}` must all agree.
- [x] Wired the native JIT as a differential `Backend` (and its force-deopt twin);
  force-JIT CI mode = `cargo test --features native-jit` (compiles every eligible
  function on first call and cross-checks against the other backends).
- [x] Coverage-style differential fuzz: a total `seed(bytes) -> program` decoder
  (`program_from_seed`) driven by proptest seeds/shrinking
  (`backends_agree_on_seed_decoded_programs`). Pointing cargo-fuzz/libFuzzer at the
  (total) decoder is the deployment step.

### Benchmark
- [x] Add a JIT mode to `rss bench` — `--mode jit-internal` (compile once, run
  with the JIT enabled; apples-to-apples with `vm-internal`).
- [x] Tier-0 speedup confirmed on a JIT-eligible numeric kernel: on a `while`-loop
  sum-of-squares, `jit-internal` ≈ 0.51 ms vs `vm-internal` ≈ 0.67 ms (~24%
  faster) — eligible functions skip the big match's non-numeric arms and the
  per-instruction suspension check.
- [x] Native mode `rss bench --mode jit-native` (requires `--features
  native-jit`). On a 5M-iteration integer kernel (`acc = acc + i*7 - i + i/3`),
  release build: `vm-internal` ≈ 627 ms, `jit-internal` ≈ 621 ms, **`jit-native`
  ≈ 9.8 ms (~64× faster than the interpreter)** — native machine code for the
  loop, identical output to every other backend. (Tier-0 gains are modest;
  native codegen is where the large speedup comes, as predicted.)

## Known bugs found by the differential (fixed)

- [x] **Integer literals lowered to untyped Rust → default `i32`.** The compiled
  backend emitted integer literals without an `i64` suffix, so an all-literal
  sub-expression defaulted to `i32` and could const-overflow at compile time
  (`attempt to compute 3528_i32 * 3457776_i32`) even though RSScript `Int` is i64
  and the value fit. **Fixed:** `lower_expr` now emits `<n>i64` for integer
  literals (floats keep their `f64` default); `lower_expr_for_expected_type` emits
  the matching suffix for sized-int slots (`Int8/16/32`, `UInt*`, …) via
  `rust_int_literal_suffix`; and `Expr::Index` casts the index `(… ) as usize` so
  slice indexing still type-checks. The ~30 `rust_lowering_*` snapshot assertions
  were updated, and the generative differential's literal range was widened past
  `i32::MAX` (`0..=60_000`) so it now *exercises* the fix instead of avoiding it.

## No-gap guarantee (always green)
- interp == jit == compiled on: generative integer / float / string programs, all
  curated parity fixtures, and hand-written struct/loop/branch/collection/
  cross-call cases.
- New JIT-covered instructions must be exercised by a 3-way test before they count
  as done.
