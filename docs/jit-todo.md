# JIT TODO

Goal: a **high-performance** JIT for the register VM with **no gap** between the
VM interpreter, the JIT, and the compiled (Rust-lowering) backend.

Design invariants (must hold at every step): see `docs/jit-roadmap.md`.
Status legend: `[x]` done · `[ ]` todo · `[~]` in progress.

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
  make struct/variant/list/object/map, return.
- [x] **Out-of-range int literal rejected at the frontend** (RS0033) — keeps the
  three backends consistent.

## To do

### Coverage (make the JIT run more of the language)
- [ ] Eliminate the run_jit ↔ drive duplication: extract a shared
  `try_exec_pure(instr, base, &mut ip)` used by both (structural gap-freeness;
  currently the two copies are guarded only by the differential).
- [ ] Collection get/set + index ops (List/Map get/set) in the eligible subset.
- [ ] `Match*` (option/result/variant) in the eligible subset.
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

### Benchmark (do AFTER the high-performance JIT lands)
- [ ] Add a JIT mode to `rss bench` (alongside eval / vm-internal / release-internal).
- [ ] Extend an existing CPU-bound benchmark (e.g. a numeric/loop workload) to
  report interpreter vs JIT vs compiled-Rust vs native-Rust.
- [ ] Record results in the benchmark README; confirm JIT > interpreter and the
  3-way differential still holds on the benchmark workload.

## No-gap guarantee (always green)
- interp == jit == compiled on: generative programs, all curated parity fixtures,
  and hand-written struct/loop/branch cases.
- New JIT-covered instructions must be exercised by a 3-way test before they count
  as done.
