# VM/JIT performance plan

Make the reg-VM and JIT tiers fast, learning from the fastest interpreters/JITs
in the field (LuaJIT, V8 Sparkplug/Ignition, Wasmtime Winch/Wizard,
JavaScriptCore) — **without breaking the §2 parity invariant or §6 sandbox
guarantees** in [`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md).

Status: **draft / not started.** Owner: TBD. Created 2026-06-19.

---

## 1. Problem statement

"The VM/JIT is not fast." Two structural reasons, found by reading the code:

1. **The interpreter dispatch is a single large `match`.**
   `try_exec_pure()` (`crates/rsscript/src/reg_vm/mod.rs:7319–8274`) dispatches
   ~70 pure opcodes through one `match`, called from the `drive()` loop
   (`mod.rs:8278–8346`). One unpredictable indirect branch per instruction; VM
   state (`ip`, `base`, `func`) lives in Rust locals but is re-threaded through a
   function-call boundary (`try_exec_pure(...) -> PureStep`) on every step. No
   computed-goto / tail-call threading. This is the classic slow path.

2. **The "tier-0 JIT" does not compile anything.**
   `run_jit()` (`mod.rs:7258–7308`) re-runs the *same* `try_exec_pure` match
   loop; its only win is skipping frame/suspension bookkeeping for functions
   proven non-suspending. It is a specialized interpreter, not a baseline
   compiler. So the real tiering is: interpreter → (slightly-faster interpreter)
   → native Cranelift JIT, with a large cliff between the middle and the top.

The native Cranelift tier (`crates/vm-jit/src/lib.rs`) is real machine code but
covers only **20 opcodes** — pure scalar arithmetic + control flow + read-only
heap helpers (`field_int`, `list_len`, `list_get_int`) — method-at-a-time, **no
OSR** (`mod.rs:7105–7250`). Anything outside that subset (heap writes, strings,
collections, closures, async) never reaches native and runs on path #1.

**Value representation:** `VmValue` is a 16-byte tagged enum
(`crates/rsscript/src/vm_value.rs:52–74`) with `Rc`/`Rc<RefCell<…>>` for heap
values. No NaN-boxing. 16 bytes is acceptable on 64-bit; the cost is per-op tag
matching and `Rc` traffic, not width.

## 2. Hard constraints (do not regress)

- **§2 Parity invariant.** Every tier — HIR-interp, reg-VM, tier-0, native,
  native-force-deopt, AOT — must remain observably identical. The 5-way
  `backend_differential` suite is the gate. Any perf change ships *with* its
  parity proof.
- **§6 Sandbox.** `VmLimits` (depth/step/mem/cancel/stdout/host-call) must keep
  holding. Note the **preemption gate** (`mod.rs:7071–7083`): native dispatch is
  refused while `step_budget`/`cancel` is armed so a hot native loop cannot
  bypass step accounting. Any new fast path must respect the same gate or prove
  it ticks the budget.
- **Determinism.** Float formatting and `Map` iteration order are observable and
  deterministic (FNV-1a hasher, ordered comparisons). Fast paths must preserve
  bit-for-bit output.
- **`panic = abort`, no_panic fuzzing, Miri scope** — see [[reg-vm-hardening]].

## 3. Reference systems (what to copy from each)

| Lever | Teacher | Technique to port |
|---|---|---|
| Interpreter dispatch | **Wizard** (Ben Titzer, OOPSLA'22) + LuaJIT | Tail-call threaded dispatch; pin VM state in registers across handlers |
| Value rep | LuaJIT | NaN-boxing *(evaluate only — likely not worth it at 16B)* |
| Baseline JIT | **V8 Sparkplug** + **Wasmtime Winch** | Single-pass, no-IR machine-code emit; one template per opcode |
| Tier architecture / OSR | **JavaScriptCore** (LLInt→Baseline→DFG→FTL) | OSR entry/exit; profiling in low tiers feeding speculation |

Top-tier speculative optimizers (TurboFan/FTL) are explicitly **out of scope**
until the interpreter and a real baseline are fast.

---

## Phase 0 — Measurement & baseline (prerequisite, blocks everything)

You cannot improve what you cannot measure, and "not fast" is currently a
feeling, not a number. Establish where time actually goes before touching code.

- [x] **0.1 Build a stable, slow-path-complete kernel set.** Done — see
      [`benchmarks/vm-jit/`](../../benchmarks/vm-jit/README.md). One focused
      kernel per slow path (anything the native tier skips): float, string,
      user-variant, set, bytes, stored-`owned Fn` dynamic call, recursion, sort,
      plus the existing int/struct/call/list/map/deque/json/async kernels
      referenced from `benchmarks/micro`. Each is data-dependent so nothing folds
      away. Coverage matrix + the "intentionally not covered" list are in the
      folder README.
- [x] **0.2 Record & commit the baseline matrix.** Done — capture in
      `benchmarks/vm-jit/baseline/baseline-20260620.json`, findings in the folder
      README. **The baseline reframed the problem:**
      - On native-*eligible* kernels the native tier is **15–50× faster than the
        VM and within ~1.4–2× of native Rust** (`nat/reg` 0.02–0.06). Native
        codegen is *not* the problem — **eligibility/coverage is.** Any heap
        write / string / collection / closure / suspend drops the whole function
        back to the interpreter.
      - On the real (ineligible) kernels both tiers do ~nothing and often
        **regress** (native `list_sort` 1.31, `map_int` 1.19; tier-0 `json`
        1.48, `dynamic_closure` 1.66) — translate, bail, eat overhead.
      - `set_insert_contains` is pathological at **1680× native Rust** — and the
        `sorted_set_ops` kernel pins it to the **hash-`Set`** specifically: the
        same insert+contains workload on an *ordered* set is **2.2×**, ~750×
        faster. Heap-variant/combinator paths run 340–660×.
      - `task_group_spawn` (structured concurrency) is **337×** *and* scales
        **≈ quadratically** in round count under every interpreted mode
        (`eval == vm == jit`) — a **runtime** bug (unreclaimed task frames), not a
        JIT one. Split out alongside the Set bug.
      - These two (hash-`Set`, quadratic `task_group`) are **separable, likely
        cheap, high-impact runtime fixes** independent of the tier work below.

      **Re-weighting from the data (overrides the Phase-1-first hypothesis):**
      Phase 3 (widen native eligibility) is now likely the **highest-ROI lever**,
      and a cheap "predict-and-skip bail" guard should land early so the tiers
      stop regressing ineligible code. Phase 1 (dispatch) still matters for the
      large body of code that will never be native-eligible.
- [ ] **0.3 Profile the interpreter** on the 3 slowest kernels (perf/`cargo
      flamegraph` inside the `dev` container) and confirm the hypothesis:
      dispatch + `try_exec_pure` call overhead dominates. Capture flamegraphs.
- [ ] **0.4 Define the win metric.** Pick target speedups per tier
      (e.g. interpreter ≥1.5×, baseline ≥3× over interpreter) and wire a
      regression check so CI fails if a kernel slows >10%.

**Acceptance:** a committed baseline + flamegraphs that confirm (or refute) that
dispatch is the dominant interpreter cost. If the profile says otherwise (e.g.
`Rc` refcount traffic or allocation), **re-order the phases below to match the
data** — do not proceed on assumption.

## Phase 1 — Interpreter dispatch (highest leverage)

Goal: remove the per-instruction indirect-branch + function-call cost. Model:
Wizard's tail-call threaded dispatch.

- [ ] **1.1 Spike: tail-call threaded dispatch.** Prototype the dispatch loop as
      one handler-fn-per-opcode that tail-calls the next handler, VM state
      (`ip`, `base`, regs ptr) passed in registers. Rust `become` (explicit tail
      calls) is the target; if unstable, evaluate a computed-goto-style
      `loop { match }` with `#[inline(always)]` hot arms and a manual jump table.
      Measure against baseline on the integer-loop kernel **before** committing
      to the rewrite.
- [ ] **1.2 Decide dispatch strategy** from 1.1 data: (a) tail-call threading,
      (b) keep the `match` but inline `try_exec_pure` into `drive()` to kill the
      call boundary and let the compiler keep state in registers, or (c) hybrid.
      Cheapest viable win first.
- [ ] **1.3 Split hot vs cold opcodes.** Keep the ~15 hottest (loads, int
      arith, compare, move, jump) on the threaded fast path; route the ~80 cold
      ones (collections, string, json, async) through a slow `match`. Keeps the
      hot dispatch table small and I-cache-friendly.
- [ ] **1.4 Accumulator evaluation (optional).** V8 Ignition uses an accumulator
      register to cut operand traffic. Assess whether a single accumulator slot
      reduces load/store opcodes enough to matter; only pursue if 0.3 shows
      operand decode as a real cost.
- [ ] **1.5 Re-run the matrix; prove parity.** Full `backend_differential`
      green, micro-bench matrix shows the 1.x target, no kernel regressed.

**Risk:** Rust tail-call support (`become`) may be unstable; the inline-the-match
fallback (1.2b) is low-risk and still removes the call boundary. Land that first
regardless.

## Phase 2 — A real baseline (tier-0) compiler

Replace the misnamed "tier-0" interpreter with a genuine single-pass
machine-code emitter, modeled on Winch (it already lives next to Cranelift in
your stack) and Sparkplug (no IR, one template per opcode).

- [ ] **2.1 Decide build-on-what.** Reuse Cranelift's assembler/`MachBuffer`
      (the Winch approach) to emit code without building Cranelift IR, vs. a
      hand-rolled emitter. Prefer Winch-style reuse to inherit register naming,
      relocations, and the existing `vm-jit` plumbing.
- [ ] **2.2 Single-pass codegen for the pure subset.** One linear walk over
      `RegInstr`, emitting a fixed instruction template per opcode against the
      register window in memory (no register allocation, no optimization). Target
      the same eligibility set tier-0 already proves (non-suspending pure).
- [ ] **2.3 Wire it as the new tier-0**, gated behind a feature flag, with the
      old `run_jit()` as the deopt fallback so parity is provable side-by-side.
- [ ] **2.4 Respect the sandbox.** Emit a step-budget tick (or honor the
      preemption gate) so the baseline cannot bypass `VmLimits`. Verify against
      the hostile-input suite.
- [ ] **2.5 Differential + bench.** 5-way agreement green; baseline ≥3× the
      interpreter on the loop/arith kernels.

**Risk:** highest-effort phase. Gate behind a flag; keep `run_jit()` until the
baseline beats it on the matrix *and* passes the full differential suite.

## Phase 3 — Widen & deepen the native (Cranelift) tier

**Re-weighted up by the Phase-0 baseline:** native is already near-Rust where it
runs (`nat/reg` 0.02–0.06), so every opcode family this phase makes eligible
converts a ~100–300× slowdown into ~near-Rust. Today native covers 20
scalar/read-heap opcodes with no OSR. Extend coverage and reduce the cliff.

- [ ] **3.0 Predict-and-skip bail (cheap, do first).** Before translating, cheaply
      predict whether a function will bail (contains an op family known to be
      unsupported) and skip native for it — the baseline shows native currently
      *regresses* `list_sort`/`map_int`/`closure_alloc` by translating then
      bailing. This is a guard, not new coverage, and stops the bleeding.

- [ ] **3.1 Coverage audit.** From the Phase-0 profile, list the highest-traffic
      opcodes that currently force a bail to the interpreter (likely list/map
      reads, string ops, struct writes). Rank by benchmark impact.
- [ ] **3.2 Add host-helper ABIs** for the top bail-causers (e.g. `list_get`
      for non-int elems, map lookup, struct field *write*), each with the
      immediate-bail-on-unsatisfiable-read contract already in §7.1. One opcode
      family per PR, each with a ledger entry (Exec-Spec Appendix C).
- [ ] **3.3 OSR entry (loops).** Today native is function-at-a-time, so a long
      loop discovered mid-function never tiers up until the next call. Evaluate
      OSR-entry so a hot loop can transfer into native mid-execution
      (JavaScriptCore model). Spike first — measure the win on a long single-call
      loop kernel before committing.
- [ ] **3.4 Tune the tier-up threshold** (`tier_up_threshold`, `mod.rs:7105`)
      from the baseline data instead of the current default-0 heuristic.

**Risk:** each new native op multiplies the parity surface. Strict rule: no
native opcode lands without a force-deopt differential test proving native ≡
interpreter on success *and* error paths.

## Phase 4 — Value representation (only if Phase 0 justifies it)

- [ ] **4.1 Quantify `Rc` traffic.** If flamegraphs show refcount inc/dec and
      allocation as a top cost, this phase is worth it; otherwise **skip it**.
- [ ] **4.2 Evaluate small-value unboxing / NaN-boxing.** At 16 bytes the width
      is fine, so the only motivation is cutting tag-match or `Rc` cost. Likely
      **not worth the parity risk** — document the decision either way.
- [ ] **4.3 Cheaper immutable collections.** If `Rc<RefCell<Vec>>` clone/COW
      cost dominates list/map kernels, consider persistent structures already in
      the dep tree (`imbl`) on the hot read path. Measure first.

---

## Sequencing & exit criteria

```
Phase 0 (measure)  ──►  Phase 1 (dispatch)  ──►  Phase 2 (baseline)  ──►  Phase 3 (native widen)
                                                                            Phase 4 only if 0 justifies
```

- **Do Phase 0 first, fully.** Every later phase is re-orderable based on what
  the profile says; the order above is the *hypothesis*, not a commitment.
- **Every phase exits on two gates:** (1) full `backend_differential` green
  (parity), (2) the committed micro-bench matrix shows the phase's target
  speedup with no kernel regressed >10%.
- **One tier change per PR,** behind a flag where it changes a hot path, with the
  prior implementation kept as the deopt oracle until the new one wins the
  matrix.

## Open questions

- Is Rust's `become` (explicit tail calls) stable enough in the toolchain pinned
  by the `dev` container? If not, Phase 1 lands as the inline-the-match variant.
- Does Winch-style codegen expose enough of Cranelift's assembler as a public API
  at the pinned Cranelift version, or do we vendor/fork?
- Acceptable parity-test runtime budget — the 5-way suite already runs ~3.5 min;
  more native opcodes grow it. May need a fast-subset CI gate + nightly full.
