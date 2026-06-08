# JIT TODO

Goal: a **high-performance** JIT for the register VM with **no gap** between the
VM interpreter, the JIT, and the compiled (Rust-lowering) backend.

Design invariants (must hold at every step): see `docs/jit-roadmap.md`.
Status legend: `[x]` done · `[ ]` todo · `[~]` in progress.

## What kind of JIT is this?

**Method-based (per-function) baseline JIT — not a tracing JIT, not native (yet).**

- **Per-function, not tracing:** it compiles whole functions, deciding eligibility
  up front. A tracing JIT instead records hot linear execution *traces* (across
  call/loop boundaries) and compiles those with guards; we don't do that.
- **Tier-0 = specializing executor:** today it executes the supported instruction
  subset via a reduced-dispatch loop (`RegVm::run_jit`) that reuses the
  interpreter's exact semantics, with per-function fallback. This is gap-free but
  only a modest speedup (no native code, values still boxed `VmValue`).
- **Next = native method JIT:** Cranelift codegen + unboxed numeric registers in a
  separate crate (see below) is where real performance comes from. Tracing could
  be a later alternative/addition, but the method JIT is the chosen path.

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
  make-closure, runtime-error, return.
- [x] **Option/Result/match coverage** verified three-way across the parity suite.
- [x] **Out-of-range int literal rejected at the frontend** (RS0033) — keeps the
  three backends consistent.

## To do

### Coverage (make the JIT run more of the language)
- [ ] Eliminate the run_jit ↔ drive duplication: extract a shared
  `try_exec_pure(instr, base, &mut ip)` used by both (structural gap-freeness;
  currently the two copies are guarded only by the differential).
- [ ] Collection get/set + index ops (List/Map get/set) in the eligible subset.
- [x] `Match*` (option/result/variant/map-get) in the eligible subset.
- [ ] Cross-function: let JIT-compiled code call other functions (needs frame
  push integration) — currently any `Call` makes a function fall back.
- [ ] Float / string / bytes ops parity coverage in the generator.

### High performance (the actual JIT)
- [ ] New `vm-jit` crate (separate, because `rsscript` is `#![forbid(unsafe_code)]`).
- [ ] Expose `RegFunction`/`RegInstr` as a stable, versioned IR for the crate.
- [ ] Cranelift native code generation for the numeric/control core; call shared
  runtime helpers for non-trivial `VmValue` ops; per-function fallback.
- [ ] `VmValue` ABI for native code (unbox Int/Float/Bool; box/inspect via helpers).
- [ ] Tiering: hot-loop counters → tier-up; OSR in/out of compiled code.
- [ ] Deopt: type guards that bail to the interpreter with exact state
  reconstruction; **test by deopting at every guard** ({jit-off, tier0, tier1,
  force-deopt} must all agree).
- [ ] Wire the native JIT as the third `Backend`; force-JIT CI mode.
- [ ] Coverage-guided differential fuzz (Fuzzilli-style seed→program + mutators).

### Benchmark
- [x] Add a JIT mode to `rss bench` — `--mode jit-internal` (compile once, run
  with the JIT enabled; apples-to-apples with `vm-internal`).
- [x] Tier-0 speedup confirmed on a JIT-eligible numeric kernel: on a `while`-loop
  sum-of-squares, `jit-internal` ≈ 0.51 ms vs `vm-internal` ≈ 0.67 ms (~24%
  faster) — eligible functions skip the big match's non-numeric arms and the
  per-instruction suspension check.
- [ ] After the native tier: report interpreter vs JIT vs compiled-Rust vs
  native-Rust on the standard benchmarks and record in the benchmark README
  (tier-0 gains are modest; native codegen is where the large speedup comes).

## No-gap guarantee (always green)
- interp == jit == compiled on: generative programs, all curated parity fixtures,
  and hand-written struct/loop/branch cases.
- New JIT-covered instructions must be exercised by a 3-way test before they count
  as done.
