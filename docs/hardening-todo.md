# Runtime hardening TODO

rsscript executes untrusted, agent-generated code, so the reg-VM must be a
sandbox: **no agent program may crash or hang the host process.** A scan found
the *execution core* already safe — checked arithmetic (`div`/`mod`-by-zero →
`EvalError`, never a panic), bounds-guarded registers, no reentrant
`borrow_mut` double-borrows — plus a real verification apparatus (differential
testing, libFuzzer targets `differential`/`fail_closed`/`format_idempotent`/
`parse_check`, and `tests/hostile.rs` proptest). So this is a **finishing job at
the edges** (resource exhaustion + the unsafe FFI/JIT seams), not a rebuild.

## The two invariants (the organizing principle)

1. **No agent program can cause a Rust panic / native crash.** Every *program*
   fault becomes a recoverable `EvalError` the runtime returns cleanly.
   Arithmetic already obeys this; deep recursion, infinite loops, OOM, and bad
   FFI handles do not yet.
2. **A Rust panic therefore means a runtime *bug*, not a program fault → respond
   with a clean abort, never UB.** `panic = "abort"` in the release profile kills
   the entire "panic unwinds across the C ABI = undefined behavior" class at the
   JIT/FFI/host-helper seams without scattering `catch_unwind`. Tradeoff: we give
   up Rust task-panic isolation — acceptable here because in RSS errors are
   *values* (a task fails by returning `EvalError`, not by panicking).

Contract once both hold: program faults → recoverable error values; runtime bugs
→ loud, safe aborts; nothing silent.

## Design constraint: don't break long ML runs

rsscript also runs the (trusted) ML framework's long training loops. So limits
are **configurable on `RegVm`** with safe defaults:
- **Recursion depth: default ON**, generous cap (e.g. 16 384 frames) — never
  trips real code, always catches `fn f(){f()}`.
- **Step budget / memory ceiling: default OFF (`Option` = `None`)** — opt-in for
  untrusted/agent-facing entry points; trusted training leaves them unset.
New error cases use `EvalError::Runtime(msg)` with stable, testable message
prefixes (the enum is intentionally minimal: `Diagnostics` / `Runtime`).

## Tier A — stop the crashes (highest probability for agent code)

- [ ] **A1 Recursion depth limit** (Inv 1). `self.frames.push()` is unbounded
  (`reg_vm/mod.rs:7523` "bounded only by memory"; pushes at 7538/7974/8019) → a
  self-recursive program overflows the native stack = uncatchable SIGSEGV. Add a
  configurable `max_depth` (default-on, generous) checked at every frame push →
  `EvalError::Runtime("recursion depth limit exceeded …")`. The #1 crash; trivial.
- [ ] **A2 `panic = "abort"`** (Inv 2). Add to workspace `[profile.release]`. One
  line. `hostile.rs` uses `catch_unwind` but runs in the dev profile (unwind), so
  normal `cargo test` is unaffected; verify fuzz targets still build.

## Tier B — stop the hangs

- [ ] **B3 Step budget** (Inv 1). The interpreter loop (`reg_vm/mod.rs:~7927`,
  `while let Some(instr) = func.code.get(ip)`) has no fuel; `while true {}` hangs
  forever. Add a per-instruction counter + configurable `step_budget: Option<u64>`
  → `EvalError::Runtime("step budget exceeded …")`. Follow-up (not required this
  pass): also poll the existing `CancellationToken` every N steps so cooperative
  cancellation can preempt a tight compute loop — the preemption hook structured
  cancellation is currently missing (it only works at await points).
- [ ] **B4 Memory ceiling** (Inv 1). Configurable `mem_budget: Option<usize>`;
  account live allocation through `ensure_regs` and list/map growth →
  `EvalError::Runtime("memory limit exceeded …")`. Harder (≈169 alloc/growth
  sites); lands after A/B3. Best-effort byte accounting is fine.

## Tier C — longevity (lower urgency)

- [ ] **C5 Miri in CI** over a small test subset (we have none today) — also
  double-checks the `unsafe` in `vm-jit`. Add a `cargo miri test` target/job;
  if Miri flags `vm-jit`, document/fix.
- [ ] **C6 Cycle/leak policy.** The value model makes accidental cycles unlikely;
  decide explicitly — document the weak-discipline + add leak tests, or plan a
  cycle collector for very-long-running apps.

## Meta — prove the invariant continuously

- [ ] **M Invariant-1 fuzz target.** New `fuzz/fuzz_targets/no_panic.rs`: feed
  random valid programs, assert the runtime only ever returns `Ok`/`EvalError`,
  never panics. Converts "we hardened it" into "CI keeps it hardened" — the
  natural extension of the existing `fail_closed`/`hostile.rs` ethos.

## Verification discipline

Each change: clippy `-D warnings` = 0; full reg-VM + parity suites green; fuzz
targets still build; new `hostile.rs` cases prove deep recursion / infinite loop
(with a budget set) / large allocation (with a ceiling set) return a clean
`EvalError` instead of crashing/hanging. Build/test in the Docker dev container.
