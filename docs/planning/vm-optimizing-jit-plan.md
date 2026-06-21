# Optimizing JIT plan — a C2-class JIT of our own

Build a profile-guided, speculating optimizing JIT — the **architecture** of
HotSpot C2 / V8 TurboFan / Graal — on top of the existing Cranelift backend, so
hot code runs far closer to native **while keeping instant dev startup** (no AOT).
This is the tier above [`vm-value-rep-plan.md`](./vm-value-rep-plan.md) and
[`vm-valhalla-plan.md`](./vm-valhalla-plan.md) (typed lists + native direct reads),
and it sits alongside [`vm-jit-perf-plan.md`](./vm-jit-perf-plan.md). It must hold the
§2 parity invariant of
[`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md).

Status: **proposed / aspirational, with one concrete near-term lane now identified:
typed-list loop optimization after Valhalla TV1/TV2.** This is a multi-quarter
initiative, staged so each step is independently shippable and soak-verified.
Owner: TBD. Created 2026-06-20.

---

## 0. The one idea

**Speculate aggressively, then deoptimize when wrong.** A baseline JIT (Cranelift
today) compiles what the code *literally says* — every call real, every check
present, every branch compiled. An optimizing JIT *bets* on what the code actually
does at runtime (this call is always type X; this value is never `None`; this
branch never fires), compiles a lean version assuming the bet, guards it with a
cheap check, and **falls back to the interpreter (deopt) if the guard fails.** That
bet-and-guard loop is the entire difference between C1 and C2 — it removes
megamorphism, unlocks deep inlining, and deletes the checks that pin a baseline JIT
~10× off native.

**Honest scope.** This steals C2's *architecture*, not its decade of heuristics. We
do **not** rebuild LLVM — Cranelift stays the code generator; we build the *mid-end*
(profiling + speculation + inlining + escape analysis) and the *deopt machinery*
above it. Realistic destination: **~3–5× off native on hot loops** (from today's
~10–20×). C2's final ~1–2× — vectorization and the long tail of inlining/heuristic
tuning — is **explicitly out of scope** (never-matched).

## 1. What we already have (and what's missing)

Mapping to HotSpot terms:

| HotSpot | We have | Gap |
|---|---|---|
| Interpreter (profiling) | reg-VM interpreter | **no profiling** (J1) |
| C1 (baseline JIT) | Cranelift native tier (~29 ops, scalar) + Valhalla typed-list direct reads | still pays loop/bounds overhead; no profiling/speculation |
| C2 (optimizing JIT) | — | **the whole mid-end** (J2–J4) |
| Deoptimization | crude: native bails → interpreter **re-runs from function top** | **no resume-at-guard** (J0) |
| Uncommon trap / OSR | — | OSR blocked on spec (J5) |
| MethodData (profiles) | — | (J1) |

The single most important existing asset: a **deopt *model* already exists** — the
native tier can bail and the interpreter takes over (the §7.2 fallback; the
`force_bail` backend, which refuses native *before execution* and runs the function
on the interpreter, verified bit-identical by the differential). But it is the crude
form in two ways: it's a **pre-execution off switch, not a mid-code guard**, and the
fallback is **re-run the whole function from the top** — sound *only* because the
compiled subset is side-effect-free (a re-run can't double-apply a write — §7.2). The
native ABI even collapses every bail to an anonymous `None` (`vm-jit/src/lib.rs:537`)
with no safepoint identity. Every step here that compiles side-effecting or
speculative code needs the **precise** form: resume *at the guard*, mid-function,
after side effects — which requires a new deopt ABI (J0.0) and full state maps (J0.1).
That is J0, and it is also exactly what perf-plan **§3.2w (heap writes in native)**
needs — so J0 pays off twice.

## 2. Hard constraints (do not regress)

- **§2 Parity, and the correction.** Speculation does **not** violate parity — it
  *requires* speculation to be backed by **correct deopt to the interpreter**. When
  the bet holds, compiled code computes the same values as the interpreter; when it
  fails, deopt resumes *in the interpreter* (the reference semantics). All backends
  still agree by construction, including the non-speculating compiled-Rust backend
  (it computes the same values a different way). The **differential + full
  generative soak is therefore an asset**, not just a gate — it is precisely the
  harness that exposes a speculation/deopt bug. Every J-step exits on the soak
  (perf-plan §3 command; ~20 min, compiled backend included).
- **`force_bail` today is NOT a guard — it's a pre-execution off switch.** Be precise:
  `force_bail` (`mod.rs:7474`) returns `None` *before* compiling/running native code
  ("pretend it bailed at the first guard"), so the interpreter runs the function from
  the top. It verifies the *fallback* path, not mid-code deopt, and the native ABI
  collapses every bail to `None` (`vm-jit/src/lib.rs:537`, `call -> Option<i64>`) with
  **no safepoint identity**. So "deopt at every safepoint" is **genuinely new
  machinery** (J0.0 ABI + real guards), not a generalization of `force_bail`. Once
  built, a **"deopt at every safepoint" stress backend** that must stay bit-identical
  to the interpreter on the whole soak is the killer correctness test for J0+.
- **No deopt loops.** A site that deopts must not re-speculate the same failing bet
  forever (the perf-plan §3.0 give-up is the seed of this): after K deopts at a
  site, recompile *without* that speculation, or fall back permanently. Counts are
  per-site, reset on success.
- **Determinism.** Profiles guide *which* code is emitted, never the *values*
  computed. Compiled output may vary run-to-run (different speculation); observable
  program output must not. Float formatting / `Map` order stay bit-for-bit.
- **Dev startup stays instant.** Cold code is interpreted; profiling and compilation
  are lazy and tiered. Profiling overhead on the interpreter (the common dev path)
  must be negligible — only *warming* functions profile (gated by a call counter).
- **§6 Sandbox — exact `VmLimits` semantics (normative, spec §6.2).** Not just "a
  budget-tick opportunity." Per Exec-Spec §6.2 (line 336), native/optimized code must
  **enforce or be ineligible**: while `step_budget`/`mem_budget`/`cancel` is active it
  compiles **only if** it emits the equivalent check **on every loop backedge** and at
  a **bounded straight-line interval** (the same points the interpreter ticks) — a
  native `while true {}` must still trip the budget — and it must **poll `cancel`**.
  Once J0.4/J3 compile *allocating* code, the current `mem_budget` exemption (the
  subset allocates nothing) **dies**: native allocations must be charged to
  `mem_budget`. And **deopt must not double-count** — snapshot the budget state and
  restore it so a tick paid in native isn't re-paid by the interpreter after deopt
  (or vice-versa). Differential tests must run with `step_budget`/`mem_budget`/`cancel`
  *enabled*, not just the default unlimited runs (a J0/J6 deliverable).
- **`panic = abort`, no_panic, Miri** — deopt state-reconstruction is `unsafe`-prone;
  keep it in one audited module under Miri.

## 3. Architecture: what we build vs. reuse

- **Cranelift = the back end. Do not rewrite it.** It does register allocation,
  instruction encoding, relocations, and already ships an **egraph mid-end** that
  gives GVN / LICM / constant folding / simple DCE for free. Reuse all of it.
- **We build the "C2 brain" above Cranelift:** profile collection (J1), profile-guided
  speculation + monomorphic inlining (J2), escape analysis + scalar replacement (J3),
  and the profile-driven passes Cranelift doesn't do (range-check elim, branch
  pruning — J4).
- **We build the deopt machinery (J0):** the side tables + state reconstruction that
  make speculation sound. This is ~80% of the hard work and the project's spine.
- **Value-rep + Valhalla are prerequisites for J3/J4.** Value-rep gives scalar
  `Option` facts; Valhalla gives flat `List<Int>`/`List<Float>` arrays and native
  direct reads. J4's first concrete job is to turn those flat arrays into optimized
  typed-list loops.

```
 reg-VM interpreter ──profiles(J1)──► Optimizing tier (our mid-end)
   │  (cold, instant)                   ├─ J2 monomorphic inlining + guards
   │                                    ├─ J3 escape analysis + scalar replacement
   │                                    ├─ J4 typed-list loops / range-check elim / branch prune
   ▲  deopt (J0: resume-at-guard)       └─ Cranelift egraph + regalloc + codegen
   └────────────── guard fails ─────────────────┘
```

---

## J0 — Precise deoptimization foundation (DO FIRST; everything depends on it)

Replace "re-run from function top" with "reconstruct interpreter state at the guard
and resume." This is the prerequisite for *all* speculation and for §3.2w writes.

- [ ] **J0.0 A real deopt ABI (FIRST — nothing works without it).** Today the boundary
      is `NativeModule::call -> Option<i64>` (`vm-jit/src/lib.rs:537`): success or a
      *single anonymous* bail, with **no safepoint identity** — so "resume *where*?"
      is unanswerable. Replace it with an ABI that names the deopt point:
      `Completed(value) | Deopt { safepoint_id, … }`.
      **Crucial mechanics — the live values must be pushed OUT, not read back.** Once
      `call` returns to Rust the compiled stack frame and machine registers are gone,
      so "record where values live and read them after return" is impossible across the
      safe boundary. So J0.0 must pick **one concrete protocol** (both extend today's
      `out_ptr`/`bail_ptr` model, where generated code already *writes* into host-owned
      buffers while the frame is live):
      - **(a) Host-owned deopt payload buffer.** `call` passes in a buffer pointer (like
        `bail_ptr`); at a guard the generated code **writes `safepoint_id` + the actual
        live values** (scalars as bits+tag; heap values as handle-table indices) into the
        buffer **before returning** `Deopt`. The host decodes the buffer — the values are
        now in host memory, not registers.
      - **(b) Deopt runtime helper, called while the frame is live.** At a guard the
        generated code **calls a host helper** (passing `safepoint_id` + the live values,
        via registers/args or the buffer) that performs the deopt before the compiled
        frame unwinds.
      Prefer (a) for a first slice (smaller blast radius — it's the existing buffer
      pattern). Either way, **the generated code materializes the values; the host never
      reaches into the dead frame.** Every guard gets a stable `safepoint_id`. Until this
      exists, J0.1–J0.3 cannot identify *or recover* a resume state.
- [ ] **J0.1 Deopt metadata — FULL frame + window state, not just live values.** For
      each safepoint (`safepoint_id`), record what it takes to reconstruct the
      *complete* interpreter state. The map is the **decode schema for the J0.0 deopt
      payload** (the layout the generated code writes), paired with where each decoded
      value is restored — *not* a list of machine-register locations to read after
      return (impossible per J0.0). It must cover:
      - **register-window values** — for each, how to decode it from the payload
        (scalar bits+tag, or handle-table index → rebuilt heap/scalar-replaced value)
        and which RegInstr register it restores to;
      - the **`written` bitmap** for the window — reads **assert** `written[index]`
        (`mod.rs:8278`) and `prepare_frame` deliberately clears written bits *and drops
        stale heap values* for ownership/`mem_budget` accounting (`mod.rs:8255`, §4
        rule 4 / §4.1). Deopt must restore written bits **consistent with the resume
        point** (every slot the resumed interpreter reads must be marked written and
        hold the right value; unwritten slots must own nothing);
      - the **`Frame` metadata** (`mod.rs:6578`): `base`, `ret_dst`, and
        `mut_writeback` (the `(caller_reg, frame_reg)` pairs that propagate `mut`
        params on return) — for **every logical frame the safepoint sits in**: once J2
        inlines callees, one compiled frame can collapse a chain of logical frames
        (outer caller → inlined callee → …), and deopt must **re-expand the whole
        chain** — re-push a `Frame` for each, innermost resuming at `ip` and each outer
        one at its call site;
      - the resume **`ip`**.
      Store beside the compiled code, keyed like the native cache.
- [ ] **J0.2 Deopt mechanism.** On a guard, read the map for `safepoint_id`:
      materialize each value into the register window (rebuild scalar-replaced values),
      **set the `written` bits** to match, **re-expand the collapsed frame chain** —
      push a reconstructed `Frame` (`base`/`ret_dst`/`mut_writeback`) for each logical
      frame the inlined safepoint sits in (outer callers + inlined callees), restore
      `ip`, and jump into
      `drive()`. The compiled frame is abandoned. Reconstruction is the `unsafe`-prone
      core — one audited module under Miri.
- [ ] **J0.3 "Deopt at every safepoint" stress backend.** A mode that forces a deopt at
      an *arbitrary* safepoint mid-execution (not the pre-execution `force_bail` off
      switch) and resumes precisely. Add it as a differential backend; it MUST be
      bit-identical to the interpreter across the whole soak — the master correctness
      test. Run it with `step_budget`/`mem_budget`/`cancel` *enabled* too (§2), so
      deopt budget snapshot/restore is exercised, not just unlimited runs.
- [ ] **J0.4 Wire §3.2w on top.** With resume-at-guard, a compiled function may now
      perform a heap *write* and still deopt correctly afterward (no double-apply,
      because deopt resumes *after* the write, not from the top). Pick the
      replacement-equivalence discipline (preflight / checkpoint / no-bail-after-commit
      from perf-plan §3.2w) and prove it against the deopt machinery.
- [ ] **J0.5 Exact `VmLimits` enforcement in generated code (spec §6.2, normative).**
      Emit the budget/cancel checks the interpreter would tick: a `step_budget` tick on
      **every loop backedge** + a bounded straight-line interval, and a `cancel` poll —
      or mark the function ineligible while those limits are armed (today's preemption
      gate). Once J0.4/J3 allocate, **charge native allocations to `mem_budget`** (the
      current "subset allocates nothing" exemption ends). Deopt must **snapshot/restore**
      the budget so a tick paid in native is not re-paid by the interpreter (or
      vice-versa) — no double-count, no skipped count. Verify with the limits-enabled
      differential from J0.3.
- **Exit:** deopt-at-every-safepoint backend green on the full soak — **including runs
  with `step_budget`/`mem_budget`/`cancel` enabled** — this is the spine; do not build
  J2+ until it is rock-solid. No perf claim yet — correctness only.

## J1 — Profiling in the lower tiers (the data speculation needs)

- [ ] **J1.1 Profile record per function** (HotSpot MethodData analog): per call-site
      **type feedback** (observed callee / receiver type-id + a monomorphic/polymorphic/
      megamorphic state), per-branch **taken/not-taken** counts, and `Option`/`Variant`
      case bias (Some/None, Ok/Err, which variant). Optionally value ranges for
      range-check elimination later.
- [ ] **J1.2 Cheap collection.** Only functions past a warm-up counter profile (cold
      code pays nothing — protects dev startup). Profiling writes are a few
      increments/stores on the already-warm path; measure that the interpreter does
      not regress (§0.4 harness, dispatch cohort).
- [ ] **J1.3 Determinism guard.** Assert profiles never feed into a *value* — only
      into compile decisions. A test that runs with profiling on vs. off must produce
      identical program output.
- **Exit:** profiles collected, interpreter non-regression proven, soak green.

## J2 — First speculation: monomorphic call inlining (the biggest single win)

Inlining is C2's highest-leverage move: it's what *exposes* every other optimization.
- [ ] **J2.1 Profile-guided monomorphic inlining.** At a call site the profile says is
      monomorphic (one observed callee), **inline that callee**, guarded by a cheap
      type/identity check that **deopts (J0)** if a different callee ever appears. This
      generalizes today's `native_inline_leaf_calls` (which inlines *statically-known*
      leaves) to *dynamically-monomorphic* sites via a guard instead of a static proof.
- [ ] **J2.2 Polymorphic (2–3 target) inline cache** with a small switch + guard;
      fall to a real call (or deopt) on miss. Megamorphic sites stay un-inlined.
- [ ] **J2.3 Deopt-loop guard** (per §2): a call site that keeps deopting is recompiled
      without the inline.
- [ ] **J2.4** deopt-at-every-safepoint differential (J0.3) per shape + soak; measure on
      `function_call_hot_loop` / `dynamic_closure_call`.
- **Exit:** soak green; hot polymorphic-call kernels improve; no deopt loops.

## J3 — Escape analysis + scalar replacement (the Valhalla payoff)

Requires value-rep unboxing (V1–V2 of the value-rep plan) — flat values to dissolve.
- [ ] **J3.1 Escape analysis** at the RegInstr/IR level: a value constructed in the
      function is *non-escaping* if it is not stored into a heap container, not
      returned, not captured by a closure, not written into a `Managed` cell, and not
      compared by identity (trivially true — value-semantic types compare structurally,
      see value-rep plan §1). Closures are excluded (reference identity).
- [ ] **J3.2 Scalar replacement.** A non-escaping unboxed `Option`/small-variant/small-
      struct is **never allocated** — its fields live in registers across the loop; the
      `Make`/`Match`/`GetFieldSlot` become register moves. This is what turns a hot alloc
      loop into an allocation-free compiled loop (the 40%-allocator cost → 0 on that loop).
- [ ] **J3.3 Deopt reconstructs scalar-replaced values** (J0.2): if the function deopts
      while a value is dissolved, rebuild the heap value from its register fields before
      resuming the interpreter. This is the tricky interaction — test it hard.
- [ ] **J3.4** soak + measure the alloc-bound cohorts (`option_result_chain`,
      `variant_match_loop`, `nested_struct_field`) under `jit-native`.
- **Exit:** soak green; alloc-bound hot loops approach native; deopt-with-scalar-
      replacement proven bit-identical.

## J4 — The rest of the optimization suite (incremental, lean on Cranelift)

- [ ] **J4.1** Reuse Cranelift's egraph mid-end for GVN / LICM / constant folding
      (already there — just ensure our IR feeds it well).
- [ ] **J4.2 Typed-list loop optimization (NEXT after Valhalla TV1/TV2).** TV1+TV2
      proved the representation and direct-read path: `List<Int>`/`List<Float>` native
      read loops are ~3.7–3.9× faster and near scalar-native, but each iteration still
      pays loop-shape overhead and conservative checks. This lane owns the next step:
      recognize counted loops over flat typed lists, hoist typed-list handle/len once
      per loop, keep the raw typed pointer live across the loop under the same pinned
      borrow protocol TV2 uses, and emit direct indexed loads in the loop body.
      Initial shapes:
      - `while i < List.len(xs) { acc = acc + List.get(xs, i); i = i + 1 }`
      - scan/update variants with one induction variable and one accumulator;
      - simple `fold`/`sum` helpers once their lowering shape is stable.
      This first sub-slice should be non-speculative when the loop guard already proves
      `0 <= i < len`; otherwise it must either keep the per-iteration bounds check or
      depend on J0 deopt before deleting it. Exit gate: keep TV2's direct-read win,
      improve `native_read_heap(_float)` beyond TV2, no interpreter regression, soak
      green.
- [ ] **J4.3 Range-check elimination** using J1 value ranges + loop induction analysis
      (kills remaining bounds checks on hot list loops), guarded by deopt. This is the
      speculative/generalized version of J4.2: when the loop shape or profile proves a
      range, compile one guard/deopt instead of a check on every load.
- [ ] **J4.4 Branch pruning**: a profiled never-taken branch is compiled as a deopt
      point, not real code (HotSpot uncommon trap).
- [ ] **J4.5 Loop unrolling** for tight counted loops (modest).
- [ ] **Vectorization: OUT OF SCOPE** (the long tail; document and stop).
- **Exit:** each pass soak-green and measured; no pass that doesn't pay its way ships.

## J5 — OSR (compile hot loops mid-execution)

The perf-plan §3.4 finding: a once-called function with a hot inner loop never tiers
up (call count tops at 1). OSR fixes that class.
- [ ] **J5.1 Spec amendment FIRST** (perf-plan §3.3 blocker): Exec-Spec §7 says OSR is
      "not applicable." Amend it (mid-loop entry/exit state mapping + parity argument)
      before any code.
- [ ] **J5.2 OSR-entry** reuses J0's machinery as its *dual*: deopt maps compiled→
      interpreter state; OSR maps interpreter→compiled entry at a loop header. Build
      OSR-entry as "deopt in reverse." Spike on a long single-call loop kernel.
- **Exit:** the once-called-hot-loop class tiers up mid-run; soak green; startup unchanged.

## J6 — Adaptive tiering policy (the full HotSpot ladder)

- [ ] **J6.1** interpret (instant) → profile when warm (J1) → optimize-compile when hot
      → recompile-or-fallback on repeated deopt (J2.3). Reuse `RSS_JIT_TIER_THRESHOLD`.
- [ ] **J6.2** Tune thresholds for *dev*: short scripts compile nothing (instant);
      heavy loops tier up. The cost guard (perf-plan §3.0 / the tiny-function-loses
      finding) lives here: don't optimize a body too small to beat the boundary cost.
- [ ] **J6.3 Limit-aware eligibility (with J0.5).** The tier-up decision must honor the
      §6.2 "enforce or be ineligible" rule: while `step_budget`/`mem_budget`/`cancel` is
      armed, only tier up functions whose codegen emits the required ticks/poll/mem
      accounting; otherwise keep them interpreted. The limits-enabled differential
      (J0.3/J0.5) is the gate.
- **Exit:** dev startup unchanged; hot workloads reach the J2–J4 wins; soak green
  (limits-enabled runs included).

---

## 4. Verification strategy (per J-step, non-negotiable)

1. **The deopt-at-every-safepoint backend** (J0.3) — the master correctness test;
   bit-identical to the interpreter on the whole soak, run after *every* J-step.
2. **Fast inner loop** — `runtime jit_acceptance` (default + native-jit) + `--test
   differential`.
3. **Full generative soak** (perf-plan §3 command, ~20 min) — the slice-exit gate; the
   only thing that random-tests enough value/type shapes to catch a deopt/speculation bug.
4. **Performance gate** — §0.4 harness: median+spread on the touched cohort, beyond
   noise, plus an interpreter-non-regression check (profiling overhead).
5. **Deopt-loop test** — a program that violates a speculation every iteration must not
   livelock recompiling; assert it stabilizes (deopt count caps).

## 5. Sequencing & exit criteria

```
J0 precise deopt (spine) ─► J1 profiling ─► J2 monomorphic inlining ─┐
        │                                                            ├─► J4 suite ─► J6 tiering
        └──────────────────► (value-rep V1–V2) ─► J3 escape+scalar ──┘
                                                   J5 OSR (needs spec amendment) — parallel to J3+
```
- **J0 is the spine** — months of work, correctness-only, must be bulletproof before
  any speculation. If J0 isn't solid, nothing above it is safe.
- **Typed-list loop optimization is the concrete post-Valhalla lane.** Its first
  non-speculative slice can ship on loop shapes whose existing guard proves
  `0 <= i < len`; the speculative/general range-check deletion waits for J0 deopt.
- **J2 (inlining) is the highest-leverage general win**; **J3 (scalar replacement) is
  the object/composite Valhalla payoff** but needs value-rep V1–V2 first.
- Every step exits on: deopt-at-every-safepoint backend green + full soak green +
  measured beyond-noise win (J2+).

## 6. Risks

- **Deopt correctness is the crown jewel.** Reconstructing precise interpreter state
  at every safepoint, through inlined frames, after partial side effects, with
  scalar-replaced values rebuilt — get it subtly wrong and you get silent divergence
  the soak catches only probabilistically. Mitigation: J0 first, deopt-at-every-
  safepoint stress backend, Miri on the reconstruction module, soak every step.
- **Deopt loops / pathological recompilation** — capped per §2 (give-up after K).
- **Profiling overhead on the dev-common interpreter path** — gate on warm-up;
  measure non-regression.
- **Parity surface explosion** — every speculation × the soak. The discipline is the
  cost; the soak is the asset.
- **The long tail you won't match** — C2's vectorization + decade of heuristics. Honest
  ceiling ~3–5× off native, not ~1–2×. Don't pretend otherwise.
- **Effort** — multi-quarter. Staged so J0 (also needed for §3.2w) and J2 each ship
  standalone value even if the rest stalls.

## 7. Open questions

- **State-map format & size.** Deopt metadata can rival code size; how compact, and is
  it emitted lazily (only for compiled functions)?
- **How much does Cranelift's egraph already cover** vs. what we must add (measure
  before building J4 passes)?
- **Inlining policy** — depth/size budget, profile thresholds; the heuristic that most
  affects results and is hardest to tune.
- **Interaction with the 5-backend parity** — the AOT/compiled-Rust backend doesn't
  speculate; confirm it always computes identical values (it does today; re-confirm per
  speculation family).
- **Does OSR (J5) want its own compiled entry or share the J2 compilation** with an
  alternate entry block? (Affects the state-map design — decide with J0.)
- **`Managed`/interior-mutability values** under escape analysis — are they ever
  non-escaping, or always escaping by definition? (Audit before J3.)
