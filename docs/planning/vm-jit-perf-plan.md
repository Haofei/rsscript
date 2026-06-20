# VM/JIT performance plan

Make the reg-VM and JIT tiers fast, learning from the fastest interpreters/JITs
in the field (LuaJIT, V8 Sparkplug/Ignition, Wasmtime Winch/Wizard,
JavaScriptCore) — **without breaking the §2 parity invariant or §6 sandbox
guarantees** in [`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md).

Status: **in progress.** Phase 0 done (0.1 kernel suite + 0.2 baseline committed;
0.3 profile and 0.4 metric remain), and the two runtime pathologies the baseline
surfaced — hash-`Set` O(n²) and quadratic `task_group` — are **fixed**. Phases
1–4 not started. Owner: TBD. Created 2026-06-19; updated 2026-06-19.

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
        VM** (`nat/reg` 0.02–0.06). Native *codegen* is not the problem —
        **eligibility/coverage is.** Any heap write / string / collection /
        closure / suspend drops the whole function back to the interpreter.
        Caveat on "near-Rust" (`native_ms` vs `rust_ms`): only **pure-scalar**
        native is near-Rust (`native_scalar_loop` 3.12 ms vs Rust 2.29, **~1.4×**).
        **Read-heap** native is *not* — `native_read_heap` is 7.58 ms vs Rust 0.57,
        **~13×** — because the host-helper call boundary (§7.1) is not free. Phase 3
        must not assume every newly-eligible family lands at scalar-native speed.
      - On the real (ineligible) kernels both tiers do ~nothing and often
        **regress** (tier-up costs more than it saves). Current `baseline-20260620.json`
        offenders: tier-0 (`jit/reg`) `native_read_heap` 2.59, `nested_struct_field`
        1.36; native (`nat/reg`) `task_group_spawn` 1.58, `bytes_scan` 1.42,
        `closure_alloc`/`option_result_chain`/`pipeline_chain` ~1.20 — translate,
        bail, eat overhead. (These shift run to run; re-read the JSON before
        quoting — the *pattern* is stable, the specific numbers are not.)
      - `set_insert_contains` was pathological at **1680× native Rust** — the
        `sorted_set_ops` kernel pinned it to the **hash-`Set`** (a `Vec` with a
        linear scan per op, O(n²)). **Fixed:** `Set` is now backed by the FNV
        `ValueMap` (value → `Unit`), O(1) membership → **4.5×**, on par with
        `map_int`. Heap-variant/combinator paths run 340–660×.
      - `task_group_spawn` (structured concurrency) was **337×** *and* scaled
        **≈ quadratically** under every interpreted mode (`eval == vm == jit`) —
        a **runtime** bug: finished task slots were never reclaimed, so the
        scheduler's per-step scans grew O(n). **Fixed:** reap a task slot on join
        (RS0030 guarantees a handle is awaited once) → linear; kernel restored to
        size 20 000.
      - Both runtime bugs (hash-`Set`, quadratic `task_group`) are now fixed,
        independent of the tier work below.

      **Re-weighting from the data (overrides the Phase-1-first hypothesis):**
      Phase 3 (widen native eligibility) is now likely the **highest-ROI lever**,
      and a cheap "predict-and-skip bail" guard should land early so the tiers
      stop regressing ineligible code. Phase 1 (dispatch) still matters for the
      large body of code that will never be native-eligible.
- [ ] **0.3 Profile the interpreter by *cohort*, not by raw slowest kernel.**
      "Slowest" is misleading: the two slowest kernels in the matrix were runtime
      **bugs** (hash-`Set` O(n²), quadratic `task_group`), now fixed — profiling
      them would have "refuted dispatch" for the wrong reason. Instead flamegraph
      (perf/`cargo flamegraph` in the `dev` container) one representative per
      cohort, because each stresses a different cost and implies a different phase:
      - **dispatch-bound** — `pure_loop_sum`, `bool_logic_loop` (→ Phase 1);
      - **allocation/variant** — `variant_match_loop`, `nested_struct_field`,
        `option_result_chain` (→ Phase 3 / Phase 4 `Rc` traffic);
      - **frame churn** — `linear_recursion`, `function_call_hot_loop` (→ Phase 1);
      - **runtime-helper-bound** — `string_text_processing`, `json_parse_access`
        (already near-Rust; little VM-tier upside).
      Report a per-cohort cost split (dispatch vs `Rc` inc/dec vs alloc vs helper),
      and re-confirm the two fixed pathologies are now O(n) rather than assuming it.
- [ ] **0.4 Define the win metric.** Pick target speedups per tier
      (e.g. interpreter ≥1.5×, baseline ≥3× over interpreter) and wire a
      regression check so CI fails if a kernel slows >10%.

**Acceptance:** a committed baseline + per-cohort flamegraphs (0.3) that say
*which* cost dominates *which* cohort — dispatch, `Rc` refcount traffic,
allocation, frame churn, or runtime helpers. **Re-order the phases below to match
that split** — e.g. dispatch-bound → Phase 1, alloc/`Rc`-bound → Phase 3/4. Do
not proceed on the assumption that dispatch dominates everything.

> **Note on ordering.** The "Phase N" numbers are *labels, not sequence.* The
> Phase-0 data re-weighted the execution order — see the
> [Sequencing](#sequencing--exit-criteria) section for the current data-driven
> order (3.0 → 3 → 1 → 2, Phase 4 only if justified). Read the phases below for
> *what* each entails; read Sequencing for *when*.

## Phase 1 — Interpreter dispatch (broad base: all non-native-eligible code)

Goal: remove the per-instruction indirect-branch + function-call cost. Default
mechanism is inlining the match on stable (1.1); Wizard-style tail-call threading
(1.2) is a nightly research alternative, not the plan of record.

- [ ] **1.1 Default plan — inline the match (stable toolchain).** Inline
      `try_exec_pure` into the `drive()` loop to kill the per-step
      function-call boundary so the compiler keeps VM state (`ip`, `base`, regs
      ptr) in registers across the hot arms; shape it as `loop { match }` with
      `#[inline(always)]` on the hottest arms. This is the **committed** Phase 1
      path: it needs no nightly features and already removes the call boundary
      called out in §1. Measure on the integer-loop kernel before/after.
- [ ] **1.2 Tail-call threaded dispatch — nightly *research* branch, not the
      critical path.** One handler-fn-per-opcode tail-calling the next, VM state
      in registers (the Wizard model). This needs Rust `become` (explicit
      guaranteed tail calls), which is **nightly-only** (`#![feature(explicit_tail_calls)]`;
      stable `rustc` rejects it with **E0658**) — and the `dev` container pins
      **stable**. So treat it as a separate spike on a nightly branch that must
      *beat 1.1's numbers by a margin worth a toolchain split* before it is even
      considered for adoption. If it doesn't, 1.1 stands and this is dropped.
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

**Risk:** the big win (tail-call threading, 1.2) is gated on a nightly feature
the pinned toolchain doesn't have, so it cannot be the plan of record. The
inline-the-match path (1.1) is low-risk, stable, and still removes the call
boundary — **land it first regardless**, and only revisit tail calls if a nightly
spike proves a margin that justifies splitting the toolchain.

## Phase 2 — A real baseline (tier-0) compiler

Replace the misnamed "tier-0" interpreter with a genuine single-pass
machine-code emitter, modeled on Winch (it already lives next to Cranelift in
your stack) and Sparkplug (no IR, one template per opcode).

- [ ] **2.1 Decide build-on-what.** Reuse Cranelift's assembler/`MachBuffer`
      (the Winch approach) to emit code without building Cranelift IR, vs. a
      hand-rolled emitter. Prefer Winch-style reuse to inherit register naming,
      relocations, and the existing `vm-jit` plumbing.
- [ ] **2.2 Single-pass codegen — side-effect-free subset FIRST.** One linear
      walk over `RegInstr`, emitting a fixed template per opcode against the
      in-memory register window (no register allocation, no optimization).
      **Caution on scope:** tier-0's *proven* eligibility (`jit_supported_instruction`,
      `mod.rs:251`) is "non-suspending," which is **not** side-effect-free — it
      already admits `SetField`/`MakeStruct`/`MakeList`/`MakeMap`/`MakeClosure`,
      `ListPush/Set/Pop`, `MapInsert/Remove`, the `Match*` family, and `CallKnown`.
      The first machine-code milestone must target only the **side-effect-free**
      slice of that set (scalar arith/compare/branch + the read-only heap helpers),
      because — see 2.3 — the deopt oracle is only sound before a side effect.
- [ ] **2.3 Wire it as the new tier-0**, behind a feature flag, with the old
      `run_jit()` as the deopt oracle. **This fallback is only valid for the
      side-effect-free subset:** `run_jit()` re-executes from the function top, so
      if compiled code performs a heap write (a mutation tier-0 *is* eligible for)
      and then bails, the fallback applies that write twice — the same §7.2 hazard
      as the native write track (3.2w). Compiling the side-effecting remainder of
      tier-0's eligibility set is therefore **gated on the same replacement
      equivalence design as 3.2w** (preflight-before-commit / checkpoint-rollback /
      no-bail-after-commit), not on this oracle.
- [ ] **2.4 Respect the sandbox.** Emit a step-budget tick (or honor the
      preemption gate) so the baseline cannot bypass `VmLimits`. Verify against
      the hostile-input suite.
- [ ] **2.5 Differential + bench.** 5-way agreement green; baseline ≥3× the
      interpreter on the loop/arith kernels.

**Risk:** highest-effort phase. Gate behind a flag; keep `run_jit()` until the
baseline beats it on the matrix *and* passes the full differential suite.

## Phase 3 — Widen & deepen the native (Cranelift) tier

**Re-weighted up by the Phase-0 baseline:** native is **15–50× faster than the
VM** where it runs (`nat/reg` 0.02–0.06), so every opcode family this phase makes
eligible converts a ~100–300× slowdown into something far closer to Rust. How
much closer is opcode-dependent: pure-scalar native is ~1.4× Rust, but read-heap
native is ~13× Rust (the §7.1 helper-call boundary; see §0.2) — so estimate each
family's ceiling from its helper traffic, not from the scalar number. Today
native covers 20 scalar/read-heap opcodes with no OSR. Extend coverage and reduce
the cliff.

- [ ] **3.0 Predict-and-skip bail (cheap, do first).** Before translating, cheaply
      predict whether a function will bail (contains an op family known to be
      unsupported) and skip native for it — the current `baseline-20260620.json`
      shows native *regressing* `bytes_scan` (1.42), `closure_alloc`/
      `option_result_chain`/`pipeline_chain` (~1.20) by translating then bailing.
      This is a guard, not new coverage, and stops the bleeding. (Refresh the
      example list from the JSON when implementing; the offenders drift per run.)

- [ ] **3.1 Coverage audit.** From the Phase-0 profile, list the highest-traffic
      opcodes that currently force a bail to the interpreter (likely list/map
      reads, string ops, struct writes). Rank by benchmark impact.
- [ ] **3.2 Add host-helper ABIs — reads first.** Extend the **read-only** §7.1
      contract to the top read-side bail-causers (`list_get` for non-int elems,
      map lookup, more field reads). The §7.2 *fallback proof* carries over
      unchanged (these are still side-effect-free reads), **but the ABI does not**:
      today's helpers (`field_int`, `list_len`, `list_get_int`) are **Int-only and
      return a single `i64`**. A non-`Int` element or a heap-returning read (a
      `String`, a nested `List`/`Struct`, a map *value*) cannot be expressed as one
      `i64`, so this step is a real **ABI design task**, not a contract reuse:
      decide the typed-result representation (tagged return + out-param, or a
      result-into-the-handle-table scheme that hands native a *new* call-scoped
      handle for a heap result), extend rule 2's handle domain to cover
      helper-produced handles, and keep every unsatisfiable/wrong-type read on the
      immediate-bail path. One opcode family per PR, each with a ledger entry
      (Exec-Spec Appendix C) **plus a §7.1 ABI amendment** and a force-deopt
      differential test covering the new return shapes.
- [ ] **3.2w Heap *writes* are a separate, harder track — do NOT fold into 3.2.**
      §7.1 is **reads only** and §7.2's fallback proof depends on the compiled
      subset being side-effect-free: a bail re-runs the function from the top, so
      *any* write performed before the bail would be applied twice. A struct/list
      /map write therefore **cannot** use the read helper contract. It requires a
      §7.2 *replacement equivalence argument* before any code lands — pick one and
      spec it: (a) **preflight-before-commit** (do every bail-able check, then
      perform all writes in a no-bail tail), (b) **checkpoint/rollback** of the
      touched heap on bail, or (c) **no-bail-after-first-commit** (the function is
      ineligible the moment a write is followed by a bail-able op). Plus:
      `mem_budget` accounting for the writes (the current native gate excludes
      `mem_budget` *because* the subset allocates nothing — that exemption dies
      here), and differential tests covering the **failure/bail-after-write path**,
      not just the success path. Until that design exists, writes stay on the
      interpreter.
- [ ] **3.3 OSR entry (loops) — requires a spec amendment first.** Today native
      is function-at-a-time, so a long loop discovered mid-function never tiers up
      until the next call. OSR-entry (JavaScriptCore model) would let a hot loop
      transfer into native mid-execution. **Blocker:** Exec-Spec §7 currently
      states "OSR is **not applicable**: this is a method-at-a-time JIT … there is
      no mid-loop replacement to perform." Pursuing OSR contradicts the normative
      spec, so step 1 is a spec change (mid-loop entry/exit state mapping +
      its parity argument), *then* a spike measuring the win on a long single-call
      loop kernel. Do not implement against the current spec.
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

**Data-driven order (supersedes the numeric phase order).** The original
hypothesis was 1 → 2 → 3 (dispatch first). The Phase-0 baseline overturned that:
native is 15–50× the VM where it runs but almost never runs, so *widening
eligibility* is the highest-ROI lever, and a cheap guard stops the tiers
regressing ineligible code today. Recommended execution order:

```
Phase 0 (measure) ─┬─► 3.0 predict-and-skip bail   (cheap guard, stop the bleeding)
                   ├─► Phase 3   (widen native — highest ROI per the data)
                   ├─► Phase 1   (dispatch — broad win for the non-eligible majority)
                   └─► Phase 2   (real baseline compiler — highest effort, do once 1+3 plateau)
                        Phase 4 only if Phase 0 profiling justifies it
```

- **Finish Phase 0 first** (0.3 profile + 0.4 metric remain). The order above is
  the current data-driven recommendation; 0.3's per-cohort split can still
  re-order it — e.g. if `Rc`/alloc traffic dominates the variant cohort, Phase 4
  rises and pure-dispatch work (Phase 1) falls.
- **Every phase exits on two gates:** (1) full `backend_differential` green
  (parity), (2) the committed micro-bench matrix shows the phase's target
  speedup with no kernel regressed >10%.
- **One tier change per PR,** behind a flag where it changes a hot path, with the
  prior implementation kept as the deopt oracle until the new one wins the
  matrix.

## Open questions

- Rust's `become` (explicit tail calls) is **nightly-only** (E0658) and the `dev`
  container pins stable, so Phase 1 lands as the inline-the-match variant (1.1).
  Open part: is a nightly-only dispatch tier worth a toolchain split *at all*, or
  do we wait for `explicit_tail_calls` to stabilize before spending on 1.2?
- Does Winch-style codegen expose enough of Cranelift's assembler as a public API
  at the pinned Cranelift version, or do we vendor/fork?
- Acceptable parity-test runtime budget — the 5-way suite already runs ~3.5 min;
  more native opcodes grow it. May need a fast-subset CI gate + nightly full.
