# VM/JIT performance plan

Make the reg-VM and JIT tiers fast, learning from the fastest interpreters/JITs
in the field (LuaJIT, V8 Sparkplug/Ignition, Wasmtime Winch/Wizard,
JavaScriptCore) — **without breaking the §2 parity invariant or §6 sandbox
guarantees** in [`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md).

Status: **major pass complete; all parity green.** Every numbered item now has a
landed increment or a documented, deliberate deferral. Summary:
- **Phase 0 — COMPLETE.** 0.1 kernels + 0.2 baseline committed; **0.3** per-cohort
  Callgrind cost-split (`profile-0.3.md`) — dispatch/frame cohorts are
  dispatch-bound, the worst (variant/struct/option, 298–652×) are alloc+Rc-bound
  (⇒ Phase 4 elevated); **0.4** noise-aware win metric (median+spread JSON +
  `compare-baselines.py` regression gate). The two runtime pathologies (hash-`Set`
  O(n²), quadratic `task_group`) remain **fixed**.
- **Phase 1.1 — DONE (minimal form).** `#[inline(always)]` on `try_exec_pure`;
  independently-measured **14–30%** on dispatch-bound interpreter kernels; 0.3's
  I-cache check (I1 miss 0.34–0.45%) showed no I-cache backfire, so §1.3 not needed.
- **Phase 3.0 — COMPLETE.** Static predictor + new runtime-bail give-up counter
  (JSC/V8 deopt-count style) that demotes compile-then-bail functions.
- **Phase 3.1 — DONE** (coverage audit). **3.2 — first read-family (Float scalar
  reads) DONE**, native at **nat/reg 0.068** (~15×), force-deopt parity on success
  + bail; Bool/Char + heap-returning reads pending. **3.3 (OSR) — deliberately
  DEFERRED** (implementing now contradicts the normative spec = a hack; 3.4 data
  strengthens the case; spec amendment is the prerequisite). **3.4 — DONE**
  (`RSS_JIT_TIER_THRESHOLD` knob + measured sensitivity).
- **Phase 2 — first milestone DONE (path B).** A switchable baseline
  (`opt_level="none"`) single-pass machine-code tier (`RSS_JIT_BASELINE=1`),
  ~23% lower compile latency than the optimizing tier, both ~40× the interpreter,
  parity green in both modes. From-scratch/Winch IR-free assembler rejected
  (aarch64 has no standalone assembler at Cranelift 0.132.1). Side-effecting
  remainder gated on §3.2w — next milestone.
- **Phase 4 — evaluated (0.3 justified it).** 4.1 quantified (~40% of instrs in the
  allocator on the option cohort); 4.2 NaN-boxing **rejected** (cost is alloc, not
  tag-matching); 4.3 move-vs-clone **deferred with design** — the real win needs a
  per-register last-use/liveness pass the reg-VM lacks; forcing it would break parity.

Parity gates used throughout: fast `runtime jit_acceptance` (default + `native-jit`)
and the full N-way `differential` (31/31 on the integrated state). Owner: TBD.
Created 2026-06-19; updated 2026-06-20.

---

## 1. Problem statement

"The VM/JIT is not fast." Two structural reasons, found by reading the code:

1. **The interpreter dispatch is a single large `match`.**
   `try_exec_pure()` (`crates/rsscript/src/reg_vm/mod.rs:7362`) dispatches
   ~70 pure opcodes through one `match`, called from the `drive()` loop
   (`mod.rs:8327`). One unpredictable indirect branch per instruction; VM
   state (`ip`, `base`, `func`) lives in Rust locals but is re-threaded through a
   function-call boundary (`try_exec_pure(...) -> PureStep`) on every step. No
   computed-goto / tail-call threading. This is the classic slow path.

2. **The "tier-0 JIT" does not compile anything.**
   `run_jit()` (`mod.rs:7301`) re-runs the *same* `try_exec_pure` match
   loop; its only win is skipping frame/suspension bookkeeping for functions
   proven non-suspending. It is a specialized interpreter, not a baseline
   compiler. So the real tiering is: interpreter → (slightly-faster interpreter)
   → native Cranelift JIT, with a large cliff between the middle and the top.

The native Cranelift tier (`crates/vm-jit/src/lib.rs`) is real machine code but
covers only **~29 opcodes** (`native_subset_instruction`, `mod.rs:347`) — pure
scalar arithmetic + control flow + read-only heap helpers (`field_int`,
`list_len`, `list_get_int`, backing `GetFieldSlot`/`ListLen`/`ListGet`) —
method-at-a-time, **no OSR** (`try_native`, `mod.rs:7114`). Anything outside that
subset (heap writes, strings,
collections, closures, async) never reaches native and runs on path #1.

**Value representation:** `VmValue` is a 16-byte tagged enum
(`crates/rsscript/src/vm_value.rs:52–74`) with `Rc`/`Rc<RefCell<…>>` for heap
values. No NaN-boxing. 16 bytes is acceptable on 64-bit; the cost is per-op tag
matching and `Rc` traffic, not width.

## 2. Hard constraints (do not regress)

- **§2 Parity invariant.** Every tier — HIR-interp, reg-VM, tier-0, native,
  native-force-deopt, AOT — must remain observably identical. Use the fast
  `runtime` JIT acceptance tests as the local gate, then full backend parity as
  the phase-exit gate. Any perf change ships *with* its parity proof.
  Local JIT gates:
  `docker compose run --rm dev cargo test -p rsscript --test runtime jit_acceptance`
  and, for native-tier work,
  `docker compose run --rm dev cargo test -p rsscript --features native-jit --test runtime jit_acceptance`.
  The full enabled differential/soak profile is intentionally heavier; the
  current full differential sweep (`RSSCRIPT_FULL_BACKEND_PARITY=1`,
  `RSS_DIFF_PROPTEST_CASES=200`, `RSS_GENERATIVE_CASES=64`,
  `RSS_GENERATIVE_MUTATION_CASES=200`) is about **21 minutes** in Docker and is
  a CI/phase-exit check, not the inner development loop.
- **§6 Sandbox.** `VmLimits` (depth/step/mem/cancel/stdout/host-call) must keep
  holding. Note the **preemption gate** (`try_native`, `mod.rs:7124`): native dispatch is
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
- [x] **0.3 Profile the interpreter by *cohort*, not by raw slowest kernel.**
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

      **Result (Callgrind, `benchmarks/vm-jit/profile-0.3.md`):** Cohorts split
      cleanly. **Dispatch/frame cohorts** (`pure_loop_sum`, `linear_recursion`,
      `function_call_hot_loop`) are genuinely dispatch-bound — steady-state ~74–77%
      in the inlined `try_exec_pure`/`drive` loop with ~0% per-iteration alloc (proven
      by a 50k→500k amortization control: malloc/free Ir stays flat while the loop
      scales). **Alloc/variant/struct cohorts** (`variant_match` 298×,
      `nested_struct_field` 365×, `option_result_chain` 652× vs Rust — the *largest*
      slowdowns) are **allocation + `Rc`/`drop`/`deep_copy_value` bound**, not
      dispatch; Phase 1 does nothing for them. **String/JSON helper cohorts** (1.6×)
      are library-bound (serde_json/indexmap/sip-hash), little VM-tier upside.
      **Recommendation:** keep Phase 3 (native-eligible) → Phase 1 (dispatch) for the
      dispatch/frame cohorts, but **elevate Phase 4 (value/`Rc` representation) to
      co-priority with Phase 1**, since the worst-slowdown cohorts are alloc/`Rc`-bound.
      I-cache check (dispatch cohort): I1 miss 0.34–0.45%, LLi 0.01–0.02% → the inlined
      `try_exec_pure` is **not** I-cache-bound, so §1.3 hot/cold should not be justified
      on instruction-cache grounds (D1 1.2–1.4% points at value-rep, i.e. Phase 4).
- [x] **0.4 Define the win metric.** **Done** — the noise-aware comparator is
      landed: `crates/rsscript/src/cli/bench.rs` now exposes per-run samples + a
      true median; `benchmarks/vm-jit/run-baseline.sh` (via `row_stats.py`) records
      a nested `{mean,median,min,max,p25,p75,samples}` per mode alongside the legacy
      mean fields; and `benchmarks/vm-jit/compare-baselines.py` compares two
      baselines on the median (falling back to mean for the old schema), flags a
      regression **iff delta > threshold AND delta exceeds the kernel's measured
      spread band**, groups by cohort, supports `--mode native` for the 3.0
      criterion, and exits non-zero on any unexcused regression (the CI/PR gate).
      Verified: self-compare → 0 regressions/exit 0; a +30% native perturbation →
      flagged/exit 1. Original rationale kept below.
      Make the pass/fail rule concrete before
      changing tier code. **Measurement protocol first — the threshold is
      meaningless without it.** The baseline numbers "shift run to run" (§0.2), so
      a raw per-kernel comparison is noise-dominated: fix the protocol before the
      threshold. Each measurement is **N≥5 runs**, compared on the **median**, with
      the run-to-run **spread (min/max or IQR) recorded alongside**; a delta only
      counts as signal when it clears the kernel's observed spread. Then:
      - each PR refreshes or compares against `benchmarks/vm-jit/baseline/*.json`
        for the touched cohort, using the median-of-N protocol above;
      - no touched kernel may regress by >10% **of its median, and only when that
        10% exceeds the kernel's measured run-to-run spread** (otherwise it is
        within noise and not a regression), without an explicit, documented
        tradeoff;
      - 3.0 succeeds if known ineligible/native-bail kernels no longer get slower
        under `jit-native`;
      - Phase 3 succeeds per opcode family only when the affected cohort improves
        and the local `runtime jit_acceptance` gate passes in default and
        `native-jit` builds;
      - Phase 1 succeeds only if dispatch-bound cohorts improve, not merely one
        synthetic loop.
      Wire this as a script/CI check after the exact thresholds are chosen from
      the 0.3 profile.

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

- [x] **1.1 Default plan — inline the match (stable toolchain). Done — minimal
      form, and it already paid off.** Rather than the full manual match-rewrite,
      the minimal reversible version landed first: `#[inline(always)]` on
      `try_exec_pure` so the compiler expands the single-instruction executor into
      **both** callers (`drive`'s hot loop and `run_jit`), keeping VM state
      (`ip`/`base`/regs ptr) in registers across the hot arms — killing the
      per-instruction call boundary §1 called out, with no IR/structure change.
      **Measured win** (release, `native-jit`, 9 iters/2 warmup, size 2 000 000,
      independent before/after with disjoint sample ranges per the §0.4 rule):
      `pure_loop_sum` **−18.8%** reg-VM / **−21.4%** tier-0; `bool_logic_loop`
      **−14.1%** reg-VM / **−29.5%** tier-0. Parity green in default (6/6) and
      `native-jit` (7/7) `jit_acceptance`. **This refutes the caveat below for the
      minimal form**: the I-cache/spill backfire did not materialize, so the call
      boundary itself was the dominant cost on this toolchain and a single attribute
      recovered it — no hot/cold split (1.3) needed *yet*. The original caveat still
      applies to the heavier manual-inline variant if it's ever pursued:
      inlining all ~70 arms into one giant function could trigger I-cache pressure /
      register spilling (what 1.3 fixes); measure perf counters before going there.
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
- [ ] **1.5 Re-run the matrix; prove parity.** Local `runtime jit_acceptance`
      green, full `backend_differential` green at phase exit, micro-bench matrix
      shows the 1.x target, no touched kernel regressed >10%.

**Risk:** the big win (tail-call threading, 1.2) is gated on a nightly feature
the pinned toolchain doesn't have, so it cannot be the plan of record. The
inline-the-match path (1.1) is low-risk, stable, and still removes the call
boundary — **land it first regardless**, and only revisit tail calls if a nightly
spike proves a margin that justifies splitting the toolchain.

## Phase 2 — A real baseline (tier-0) compiler

Replace the misnamed "tier-0" interpreter with a genuine single-pass
machine-code emitter, modeled on Winch (it already lives next to Cranelift in
your stack) and Sparkplug (no IR, one template per opcode).

- [x] **2.1 Decide build-on-what. Done — path (B): single-pass IR lowering at
      `opt_level="none"`, NOT a hand-rolled assembler.** The Phase-2.1 feasibility
      study rejected a from-scratch / Winch-style IR-free assembler: the only clean
      public IR-free assembler at the pinned Cranelift **0.132.1**
      (`cranelift-assembler-x64`) is **x86-64 only**, and the primary dev/CI target
      is **aarch64**, for which no standalone assembler exists at this version.
      Path B therefore reuses the existing `vm-jit` translation (already ~1:1
      `RegInstr→JitInstr→IR`) and distinguishes the baseline tier purely by the ISA
      flag `opt_level="none"` vs. the optimizing tier's `opt_level="speed"`. The win
      is **compile latency** (a baseline compiles faster than the optimizing tier)
      while still emitting real machine code that crushes the interpreter. This is
      path B (IR, no optimization), not a hand-rolled emitter — see
      `benchmarks/vm-jit/phase2-baseline.md`.
- [x] **2.2 Single-pass codegen — side-effect-free subset FIRST. Done (path-B
      milestone).** The baseline emits machine code for exactly the existing
      **side-effect-free** native-eligible set (scalar arith/compare/branch + the
      read-only heap helpers `field_int/float`, `list_len`, `list_get_int/float`).
      No new side-effecting opcode is added to the subset, so it remains the proven
      slice. Implementation: `NativeModule::new_with_opt(helpers, baseline)` in
      `crates/vm-jit/src/lib.rs` (baseline ⇒ `opt_level="none"`, default keeps
      `"speed"`); `NativeState::new_with_opt(.., baseline)` in
      `crates/rsscript/src/reg_vm/mod.rs`, selected by `RSS_JIT_BASELINE=1` at the
      `eval_main_with_args_native` entry (default unset = optimizing). The
      side-effecting remainder of tier-0's eligibility set (`SetField`, `Make*`,
      `ListPush/Set/Pop`, `MapInsert/Remove`, …) is the **next milestone**, gated as
      in 2.3 below.
- [x] **2.3 Wire it behind a flag, with `run_jit()` retained as the deopt oracle.
      Done.** The baseline tier is selectable via `RSS_JIT_BASELINE=1`; default
      builds and the differential (which never sets the var) are undisturbed.
      `run_jit()` / the interpreter are unchanged and remain the deopt oracle.
      Because the compiled subset is the side-effect-free scalar + read-only-heap
      set, the §7.2 bail-re-runs-from-the-top fallback proof carries over
      **verbatim** at `opt_level="none"` — no write can be applied twice because no
      write is compiled. **The side-effecting remainder stays gated on the same
      replacement-equivalence design as 3.2w** (preflight-before-commit /
      checkpoint-rollback / no-bail-after-commit), which is the **next milestone**;
      it is explicitly NOT unlocked by this oracle.
- [x] **2.4 Respect the sandbox. Done (inherited).** The baseline tier reuses the
      identical IR translation, host-helper ABI, and dispatch gates as the optimizing
      tier; only the ISA `opt_level` flag differs. The step-budget / preemption gates
      in `try_native` (which refuse native dispatch while the step budget or cancel
      flag is armed) apply unchanged in baseline mode, so `VmLimits` cannot be
      bypassed. The hostile-input suite (`runtime` target) passes in both modes.
- [x] **2.5 Differential + bench. Done.** `runtime jit_acceptance` green in both
      default (optimizing) and `RSS_JIT_BASELINE=1` (baseline) modes — proving
      `opt_level="none"` is observationally identical — and the `differential` N-way
      suite green. Bench (`native_scalar_loop`, reg VM): both native tiers are
      ~40× the interpreter; baseline compile latency is ~23% lower than optimizing
      (the path-B win), steady-state ~17% slower (the expected path-B trade). Full
      numbers + parity output in `benchmarks/vm-jit/phase2-baseline.md`.

**Risk:** highest-effort phase. Gate behind a flag; keep `run_jit()` until the
baseline beats it on the matrix *and* passes the full differential suite.

## Phase 3 — Widen & deepen the native (Cranelift) tier

**Re-weighted up by the Phase-0 baseline:** native is **15–50× faster than the
VM** where it runs (`nat/reg` 0.02–0.06), so every opcode family this phase makes
eligible converts a ~100–300× slowdown into something far closer to Rust. How
much closer is opcode-dependent: pure-scalar native is ~1.4× Rust, but read-heap
native is ~13× Rust (the §7.1 helper-call boundary; see §0.2) — so estimate each
family's ceiling from its helper traffic, not from the scalar number. Today
native covers ~29 scalar/read-heap opcodes with no OSR. Extend coverage and reduce
the cliff.

- [x] **3.0 Predict-and-skip bail (cheap, do first). Done.** This split into two
      mechanisms, because the regressions had two root causes:
      1. **Static** skip (was already in place): `mark_predictably_native_ineligible`
         marks functions containing a structurally-unsupported op family
         `NATIVE_STATUS_NOT_ELIGIBLE` before the run, and `try_native`'s
         cheap-negative early-return skips them with zero per-call work.
      2. **Runtime-bail give-up (new).** The residual regressions
         (`bytes_scan` 1.42, `closure_alloc`/`option_result_chain`/`pipeline_chain`
         ~1.20) came from functions that *pass* the structural check (via inlinable
         helper calls), get **compiled**, then **bail on every call** (arg-type
         mismatch or a runtime guard) — with no memory of the bail, so they
         re-marshal+re-invoke+bail forever. Fix (`reg_vm/mod.rs`): a per-function
         **consecutive-bail counter** (`NativeState::bail_counts`,
         `NATIVE_BAIL_GIVEUP_THRESHOLD = 3`); at threshold the function is demoted to
         `NOT_ELIGIBLE` and evicted from the compile cache, so mechanism #1's
         cheap path then short-circuits it. The counter **resets on any successful
         native completion**, so a hot function that bails only on a rare data edge
         keeps its fast path (JSC/V8 deopt-count style). Two native-jit tests prove
         attempts plateau at the threshold instead of scaling with call count, and
         that the counter is consecutive (resets on success). Parity re-verified:
         `jit_acceptance` green in both default (6/6) and `native-jit` (7/7) builds.
         The threshold is a candidate for the data-driven tuning in §3.4.
         **Follow-up:** quantify the actual regression removal by re-running
         `run-baseline.sh` and diffing with `compare-baselines.py --mode native` (the
         §0.4 harness) once a fresh baseline is captured.

- [ ] **3.1 Coverage audit.** From the Phase-0 profile, list the highest-traffic
      opcodes that currently force a bail to the interpreter (likely list/map
      reads, string ops, struct writes). Rank by benchmark impact.
- [~] **3.2 Add host-helper ABIs — reads first. First family (Float scalar reads)
      DONE; remaining families pending.** The first read-family — non-Int **Float**
      field/element reads (`GetFieldSlot`/`ListGet` → `f64`) — is implemented and
      parity-proven. New `FieldFloatFn`/`ListGetFloatFn = extern "C" fn(i64,i64)->f64`
      + `JitInstr::{FieldFloat,ListGetFloat}` in `vm-jit` (IR_VERSION 2→3), rss-side
      `jit_struct_field_float`/`jit_list_get_float` returning `Option<f64>`, and the
      native type-inference now lets a read's result type **flow from its uses**
      (Float dst ⇒ Float helper) instead of forcing `Int`; ambiguous/unconstrained
      still defaults to Int, heap/Bool dst still bails. Purely additive — the Int
      path is byte-for-byte unchanged and no write was introduced, so the §7.2
      deopt-from-top fallback stays valid. Force-deopt differential covers success
      **and** bail paths (`backends_agree_on_native_float_heap_reads`,
      `backends_all_fail_on_out_of_bounds_float_list_get`). **Measured:** the new
      `native_read_heap_float` kernel runs native at **nat/reg = 0.068** (~15×, on
      par with the Int `native_read_heap`). **Still pending** (each its own PR):
      Bool/Char scalar reads (ride the i64 channel, trivial), then the harder
      heap-returning reads (String / nested struct / map value) that need the
      result-into-handle-table scheme below.
      The original design note (still governs the remaining families): extend the
      **read-only** §7.1 contract to the top read-side bail-causers (`list_get` for
      non-int elems, map lookup, more field reads). The §7.2 *fallback proof* carries over
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
      **Status: deliberately DEFERRED, not implemented — implementing it now would
      contradict the normative spec, which would itself be a "hack."** The 3.4
      measurement supplies fresh, concrete motivation: a function whose entire hot
      loop runs in a body `main` calls *once* (e.g. `native_read_heap` at size 2e6)
      can **never** tier up under per-call counting — its call count tops out at 1 —
      so any `tier_up_threshold ≥ 1` leaves it interpreted (a measured **~13×**
      cliff). OSR-entry is the only mechanism that fixes this class. The correct
      next action remains the **Exec-Spec §7 amendment** (mid-loop entry/exit state
      mapping + parity argument) *before* any code; that spec change is the
      prerequisite deliverable and is out of scope for a code-only pass. OSR is
      elaborated as **J5** of [`vm-optimizing-jit-plan.md`](./vm-optimizing-jit-plan.md),
      where it reuses the precise-deopt machinery (J0) as its dual (deopt maps
      compiled→interpreter; OSR maps interpreter→compiled).
- [x] **3.4 Tune the tier-up threshold** (`tier_up_threshold`, `mod.rs:6640`;
      hot-check at `7150`)
      from the baseline data instead of the current default-0 heuristic.
      **Result:** added a runtime `RSS_JIT_TIER_THRESHOLD` env knob in
      `eval_main_with_args_native` (default stays 0, so the differential keeps full
      coverage). Measured ∈ {0,1,5,20,100} on `native_read_heap` and
      `function_call_hot_loop` (median ms, reg VM, 7 iters / 2 warmup): the
      many-short-calls kernel is fully threshold-insensitive (≈203–206 ms across all
      thresholds), and the single-hot-function kernel **regresses ~13×** (6.9 ms → ~93 ms)
      at *any* threshold > 0 — because its hot loop lives in one function that is
      entered exactly once, so threshold ≥ 1 pins it on the interpreter forever.
      Recommendation: **keep the production default at 0**; only raise
      `RSS_JIT_TIER_THRESHOLD` when profiling a workload with many *briefly-called*
      eligible functions (which the current all-hot kernel suite does not contain).

**Risk:** each new native op multiplies the parity surface. Strict rule: no
native opcode lands without a force-deopt differential test proving native ≡
interpreter on success *and* error paths.

## Phase 4 — Value representation (only if Phase 0 justifies it)

> Phase 0 **did** justify it (the alloc/Rc cohorts are the worst). The full
> structural treatment — unbox value-semantic types (Valhalla-style) to kill the
> 40% allocation cost and make values register-shaped so Cranelift can elide
> allocation on hot loops — is detailed in
> [`vm-value-rep-plan.md`](./vm-value-rep-plan.md). 4.1–4.4 below are the scouting
> that fed it; 4.4 (closure cache) is the landed proof-of-concept.

- [x] **4.1 Quantify `Rc` traffic.** DONE — callgrind on `option_result_chain`
      (30000) shows **~40% of retired Ir inside the libc allocator** plus ~7% in
      `drop_in_place<VmValue>` / `Rc::drop_slow` / `<VmValue as Clone>::clone`.
      Allocation + refcount churn dominates, not dispatch. Phase 4 is justified.
      Evidence: `benchmarks/vm-jit/profile-4.1.md` §4.1.
- [x] **4.2 Evaluate small-value unboxing / NaN-boxing.** DONE — **decided SKIP**.
      At 16B width is fine and the 0.3 + 4.1 data show the cost is
      allocation/`Rc` traffic, not tag-matching, so NaN-boxing wouldn't address
      the real cost and carries large parity/UB risk (float bit-patterns,
      deterministic float formatting, `Map` order). See profile-4.1.md §4.2.
- [~] **4.3 Cheaper churn (move vs clone) — EVALUATED, DEFERRED.** The lowest-risk
      win (move boxed/`Rc` payloads on *consumed* `Some`/`Ok`/`Err` unwraps
      instead of clone-then-drop) requires per-register liveness the reg-VM does
      not track today; forcing a `take_reg` without it risks §2 parity. Exact
      follow-up design (add a compile-time `consume`/last-use flag to the
      value-moving opcodes, then move on consume) recorded in profile-4.1.md §4.3.
      No code changed this step. (`imbl` immutable-collection swap is the other
      open sub-lever; measure list/map kernels first.)
- [x] **4.4 Non-capturing-closure cache — DONE (first sound allocation win).**
      `MakeClosure` heap-allocated a fresh `Rc<VmClosure>` every execution; for a
      closure that captures nothing (`|x| x*2+1`) that allocation is identical each
      iteration. Now cached per `function` id and the `Rc` cloned instead — killing
      ~300k allocations/run on `closure_alloc_loop`. **Parity hazard handled
      soundly:** closures compare/hash by pointer (`vm_value.rs:471` `Rc::ptr_eq`,
      `:296`), so sharing is only enabled when a whole-program gate
      (`RegUnit.closure_identity_observable`) proves no `==`/`!=` can reach a
      closure-bearing value (transitively through structs/variants/lists/options);
      `is_hashable=false` already bars closures from Map/Set keys. Over-approximates
      toward *disabling* (never unsound). **Verified by the full generative soak**
      (`RSSCRIPT_FULL_BACKEND_PARITY=1` + 200/64/200 cases, compiled backend):
      31/31 in 1229s, plus jit_acceptance 8/8 and differential 31/31.
      **Measured: `closure_alloc_loop` 30.6→27.7 ms (~9.5%)**, delta ≫ spread.
      This is the template for the remaining allocation work: elide/share heap
      cells only behind an identity/escape gate that the soak certifies.

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
- **Every phase exits on three gates:** (1) local `runtime jit_acceptance` green
  in Docker, including `--features native-jit` when native code changes; (2) full
  `backend_differential` green for parity; (3) the committed micro-bench matrix
  shows the phase's target speedup with no touched kernel regressed >10%.
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
- Acceptable parity-test runtime budget — the local JIT acceptance gate is
  sub-second once built, the full default `runtime` target is about 20 seconds,
  and the full enabled differential profile is about 21 minutes in Docker. More
  native opcodes grow the full gate, so keep `runtime jit_acceptance` as the fast
  PR gate and reserve full parity for CI/phase exit.
