# Optimizing JIT plan — a C2-class JIT of our own

Build a profile-guided, speculating optimizing JIT — the **architecture** of
HotSpot C2 / V8 TurboFan / Graal — on top of the existing Cranelift backend, so
hot code runs far closer to native **while keeping instant dev startup** (no AOT).
This is the current home for VM/JIT performance status. It must hold the
§2 parity invariant of
[`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md).

Status: **core speculation engine + OSR core shipped (J0–J3, J4.3, J5.1, J5.2-core
on `main`) and differential-verified.** The deopt spine, profiling, mono/poly
inlining, Option scalar replacement, range-proven arithmetic, the OSR spec contract,
**and the OSR-entry/exit core** are landed. **The #1 lever (OSR×J2/J3 composition) is
COMPLETE and PRODUCTION (auto-fires by default, no flag).** Fourteen measured wins
(same-machine, all OSR-differential byte-identical on the full corpus): **OSR** ~39×
(`osr_scalar_loop`); **J4.3b induction** ~16% (`native_scalar_loop`); the composition on
every value shape — **OSR×J3-Option** ~45×, **OSR×J3-variant** ~42×, **OSR×J3-struct**
~75×, **OSR×inline-leaf** ~69× (cross-function), **OSR×J2 capturing-closure** ~15×; and —
firing on the **literal named alloc-bound benchmark kernels** — **`variant_match_loop`**
~40× and **`dynamic_closure_call`** (stored/poly/capturing closure) ~4.6×, both now
allocation-free native via auto-OSR. **Plus five more shipped this session** (clean
rebuild on HEAD `947cd54`, reg_vm vs native, same machine): **closure-allocation sinking**
(`closure_alloc_loop` ~53×), **deopt-before-heap** (`option_result_chain` ~9.5×),
**self-tail-call → loop** (`linear_recursion` ~63×, whole-function native), **native
`IntToFloat`** (`float_loop_sum` ~42×), and **loop-carried struct replacement**
(`struct_field_rw` ~40×). The earlier worst-offender cohorts (298–652× slow) are
the ones now fast. **Auto-trigger** (hot-backedge counter, no interpreter
regression) and a **broadened reducible-loop detector** make it fire on real code by
default. Standalone **J2/J3** (whole-function, outside OSR) stay coverage scaffolds —
the real-kernel wins come *through* OSR, which is shipped. **Remaining:** narrower
coverage (heap-payload variant arms, live-out value reconstruction, megamorphic sites, float-capture edge cases) + the gated tail
(J0.4/J0.5 heap-write deopt, J4.4 range-check [sandbox-risky], J4.5/J4.6, the J4.3
numeric-mode language decision). Owner: TBD. Created 2026-06-20; status updated 2026-06-22.

### Current state (shipped / partial / future)

| Item | State | Notes |
|---|---|---|
| **J0.0–J0.3** precise-deopt spine | **shipped (leaf/scalar subset)** | `NativeOutcome::Deopt` + per-site safepoint ids + live-reg capture + state-map + precise resume + every-safepoint stress test. **Precise resume is now the production DEFAULT** (`eval_main_with_args_native`); re-run-from-top remains the byte-identical fallback when a heap write disables precise resume (`can_precise_deopt_resume`) and stays under differential coverage via the force-deopt backend. The state-map is `resume_ip + live registers` — **not** yet the full inlined logical-frame-chain format J0.1 describes. So: done for the current leaf/scalar subset; full inlined-frame / side-effecting deopt is still **future**. |
| **J0.1 (heap-aware reg reconstruction)** | **shipped** | The deopt state map distinguishes reconstructible scalars (`Int`/`Float`) from heap refs (`Handle`/`FlatInt`/`FlatFloat`): `decode_deopt_live` reconstructs only scalar regs (the interpreter frame already holds heap values — precise resume implies no heap writes), and `restore_native_deopt_live_regs` restores ALL scalar regs incl. **reassigned scalar params** (the `< n_params` skip is gone). Fixes the reassigned-scalar-param resume bug (`precise_deopt_restores_reassigned_scalar_param`); flat-buffer params stay uncorrupted. **Precise resume is now default-on (validated corpus-wide).** **Remaining for full J0.1:** inlined logical-frame-chain state-map format, and heap-payload-variant / live-out *value* reconstruction (rebuilding native-built composite heap values — variant/struct with heap payload — across a bail; today such arms bail). **J0.1(b) slice 1 shipped (2026-06-28): live-after always-`Ok` *scalar*-payload Results now OSR via OSR-exit reconstruction (`OsrEntry.variant_reconstructs`); the heap-payload-arm-taken sub-case still bails.** |
| **J0.4** heap writes + deopt-after-write | **S0–S3 shipped; S4 future** | **S0** (heap-result return ABI, `95e2b71`) and **S1–S3** (allocate-only: 10 `AllocatesResult` host helpers — `StringFromInt`/`StringConcat`/`StringSlice`/`StringPadLeft`/`StringSplit`/`StringLiteral`/`JsonParse`/`JsonField`/`BytesSlice`/`ListNewInt` — publishing fresh unaliased values into `JIT_HEAP_RESULTS`, gated by `escaping_output_handle` escape analysis; `mem_budget` kept exact by **Model-A refusal when armed** (`tier.rs`), so no in-helper accounting/double-charge; §7.2 via clear-on-bail output table + `JitHeapResultsGuard`, force-deopt-tested) are **shipped & green**. In-place **caller-aliased** writes (**S4**) remain **future** — they need J0.1 frame-chain + J0.5. The native subset is no longer read-only: it allocates fresh heap values. |
| **J0.5** in-generated-code `VmLimits` accounting | **step + cancel shipped (OSR tier); mem future** | The armed OSR variant now **enforces `step_budget` and `cancel` in generated code** (Exec-Spec §6.2, the *enforce* branch): a per-instruction step accumulator (one tick per instruction, matching the interpreter's `tick()` stream 1:1 since `resume_ip` is a shared instruction index), a `steps > step_budget` test + `cancel` poll at **every loop header** (= once per iteration of every loop, incl. nested — headers = backward-edge targets), a steps write-back on **every** native exit (clean `Return` + the shared `fallback` deopt edge), and a bail-to-interpreter that resumes at the header so the interpreter (the sole limit authority) raises the error. ABI: a host-owned `[steps, step_budget, cancel_addr]` cell threaded as the new `limits_ptr` param (`call_with_limits`); unarmed variants ignore it (byte-identical pre-J0.5 codegen, zero hot-path cost). Gate: `try_osr`/`resolve_osr_candidate` no longer blanket-refuse on any limit. **`mem_budget`: native must charge EXACTLY what the interpreter charges (parity).** The interpreter's only per-iteration `account_bytes` sites are flat-list capacity growth (`List.push`/`List.append`) and list/map LITERAL construction (`MakeList`/`MakeMap`); strings, JSON, and map/set/deque/sorted-map inserts charge ZERO. Of the charged ops, only `List.push` is native-lowerable (`MakeList`/`MakeMap`/`ListAppend` are not, so a loop using them is not native-eligible). So a native OSR loop charges exactly the interpreter's amount UNLESS it calls a `ListPush*` helper — every other allocation is uncharged by BOTH and runs natively under an armed `mem_budget` (exact parity, no in-code accounting needed). `jit_fn_has_unaccounted_mem_charge` declines only `ListPush*` loops (already vetoed for live-in lists by OSR growth-admissibility, so this is belt-and-suspenders). Tests: `native_osr_completes_under_generous_step_budget`/`native_osr_trips_tight_step_budget`/`native_osr_cancel_flag_preempts`/`native_osr_nonallocating_loop_runs_under_mem_budget`/`native_osr_map_insert_loop_runs_under_mem_budget` (pos, `osr_entries>0`); hostile limits suite green (runaway-list-alloc still trips on the interpreter). **Remaining:** in-code `mem_budget` byte accounting for native `ListPush` growth (tied to S4; rare in OSR) and the whole-function/recursive tiers (still Model-A refusal when armed). |
| **J1** profiling | **shipped** | per-call-site type feedback on dynamic sites, warm-gated, no interpreter regression. (Branch profiling **not** shipped — it regressed `bool_logic_loop` ~7%; needs a hot/cold dispatch split.) |
| **J2.1 / J2.2** mono + poly closure inlining | **shipped; fires on real kernels via OSR (measured)** | guarded inline of closures, **broadened to capturing, stored (struct/list-fetched handle), and polymorphic** closures with scalar-capture materialization. Composed into OSR, this makes the literal `dynamic_closure_call` allocation-free native — **~4.6×**, `osr_entries:1`. (Standalone whole-function J2 outside OSR remains a coverage scaffold; the wins come through OSR.) |
| **J2.3** no deopt loops | **satisfied** | existing give-up counter → permanent fallback. |
| **J3** scalar replacement (Option + variant + struct) | **shipped; composes with OSR (measured ~42–75×)** | non-escaping scalar-payload `Option`, scalar-payload user variants (single- **and multi-field arms**), **and structs (flat, nested/recursive, and loop-carried with in-place mutation)**. **Composed with OSR** (Pending #1, auto-fires by default): `osr_option_loop` ~45×, `osr_variant_loop` ~42×, `osr_struct_loop` ~75×, and the literal `variant_match_loop` ~40× (via OSR×inline-leaf-calls for the cross-function case). **Nested structs and multi-field variant arms now shipped** (recursive struct dissolution; N≥0-scalar-field arms). Remaining: heap-payload variant arms (live heap `Err`/payload not bailed), megamorphic sites. |
| **J4.1** Cranelift egraph (GVN/LICM/fold) | **present** (pre-existing). |
| **J4.2** typed-list loop | **investigated — no standalone slice.** |
| **J4.3** checked-`Int` arith elision | **shipped — measured win** | (a) proof-only i128 interval elision, plus (b) **J4.3b induction-bounded** branch-conditioned refinement: a counter `i` under a guard `i < N` is proven non-overflowing ⇒ `i + 1` emitted unchecked. **~16% on `native_scalar_loop`** (the canonical `i < N; i += 1` counter shape). Honest ceiling: unbounded accumulators (`total += x`) and `i + 2` / `i <= N` correctly stay **checked**. Remaining (future): speculative deopt-on-overflow for accumulators (needs precise deopt). |
| **J4.4** range-check elim | **future / deferred** | sandbox-OOB-critical; the plan's own J4.2 verdict deferred it. |
| **J4.5** branch pruning | **future** | needs branch profiling (regressed). |
| **J4.6** loop unrolling | **future** | modest; must prove a win to ship. |
| **J5.1** OSR spec contract | **shipped** (Exec-Spec §7, dual-of-deopt + parity argument). |
| **J5.2** OSR implementation | **SHIPPED — production, auto-fires by default** | OSR-entry/exit (`compile_osr` / `try_osr`) over a **reducible single-exit loop**, byte-identical to the interpreter on the full differential. **Auto-trigger** (hot-backedge counter, no interpreter regression) fires it **without any flag** (`RSS_JIT_OSR` is now the eager/force path). **OSR × J2/J3 composition** dissolves Options/variants/structs and inlines (leaf + capturing/stored/poly closures) inside the loop. ~39× (`osr_scalar_loop`) up to ~75× (`osr_struct_loop`); literal kernels `variant_match_loop` ~40×, `dynamic_closure_call` ~4.6×. **Remaining:** heap-payload variant arms, live-out struct/value reconstruction, megamorphic sites (production perf-matrix: committed `baseline-20260623-jit.json` on `4219800`, current source of truth). |
| **J6** adaptive tiering | **Model-A + enforce (OSR step/cancel) satisfied** | interpret → warm-profile (J1) → tier-up → give-up demotion. Exec-Spec §6.2 "enforce *or be ineligible*": the **OSR tier now ENFORCES** `step_budget`/`cancel` in generated code (J0.5); the whole-function/recursive tiers remain on the **ineligible** branch (refuse-native-while-armed). `mem_budget` accounting in generated code is still **future** (so OSR is refused while `mem_budget` armed). |

> **Headline caveat (the honest bottom line).** All shipped slices are
> correctness-complete and differential-green, and **the #1 lever (OSR × J2/J3
> composition) now fires on real kernels by default** (auto-trigger, no flag). Fourteen
> measured same-machine wins, all OSR-differential byte-identical on the full corpus:
> **OSR** ~39× (`osr_scalar_loop`); **J4.3b induction** ~16% (`native_scalar_loop`);
> **OSR×J3-Option/variant/struct** ~45×/~42×/~75×; **OSR×inline-leaf** ~69×
> (cross-function); **OSR×J2 capturing-closure** ~15×; the **literal alloc-bound
> benchmark kernels** `variant_match_loop` ~40× and `dynamic_closure_call` ~4.6×; and
> **five more this session** — closure-alloc sinking `closure_alloc_loop` ~53×,
> deopt-before-heap `option_result_chain` ~9.5×, self-tail-call `linear_recursion` ~63×,
> native-IntToFloat `float_loop_sum` ~42×, loop-carried-struct `struct_field_rw` ~40×. The
> earlier worst-offender cohorts (298–652× slow) are the ones now fast. *Standalone*
> J2/J3 outside OSR remain coverage scaffolds — but the wins come through OSR, which is
> shipped and on by default. Remaining is narrow (heap-payload variant arms, live-out
> reconstruction, megamorphic sites) + the gated tail (J0.4 heap-write deopt, J4 numeric-mode decision, range-check).
> The
> checkboxes in J0–J5 below predate this work and are kept for design intent; this
> table is the authoritative current state.

### Pending work, prioritized — the crux is *firing on real kernels*, not more machinery

The shipped slices are correctness-complete; what remains is mostly making that
machinery **engage real program shapes and prove it**, not new subsystems. In priority:

1. **OSR × J2/J3 composition — THE big lever. SHIPPED for Option (~45×) + variant
   (~42×).** OSR gets a hot loop into native; J2 (inlining) and J3 (scalar replacement)
   run *inside the OSR loop region* so loops over closures / constructed
   variants/structs/Options become allocation-free native — the alloc-bound cohorts
   (298–652× slow) made fast, the largest available win.
   - **SHIPPED — all five composition slices, each measured allocation-free native:**
     **OSR×J3-Option** ~45× (`osr_option_loop`), **OSR×J3-variant** ~42×
     (`osr_variant_loop`), **OSR×J3-struct** ~75× (`osr_struct_loop`),
     **OSR×inline-leaf-calls** ~69× (`osr_inline_variant_loop` — cross-function: the
     value lives across `make_shape`/`area`, inlined into the loop then dissolved), and
     **OSR×J2 capturing-closure** ~15× (`osr_closure_loop`). The OSR path runs
     `native_inline_leaf_calls` (incl. monomorphic capturing-closure inline via a
     `closure_capture` helper) → Option → variant → struct region passes, composing all
     transformed→original ip-maps and remapping the exit `resume_ip`; the dissolved
     values are loop-internal/dead-at-boundary (so live-in/out stay the original
     loop-carried regs), and OSR bails if the loop boundary maps into an inlined region
     (`maps_into_inline`) or any dissolved reg escapes (`RegFootprint::All` ⇒ bail). Each
     has positive + escaping/non-inlinable-negative tests; OSR differential byte-identical.
   - **Also SHIPPED since:** **stored + polymorphic + capturing** closures (operand may
     be any native-readable handle via `FieldHandle`/`ListGetHandle`; per-arm
     `closure_capture` materialization) and the **broadened reducible-loop detector** (an
     internal `if`/`match` reset no longer blocks detection; the real fix was making the
     inline pass region-aware so an out-of-region `bench_size` call doesn't veto OSR) —
     together these make the literal `dynamic_closure_call` (~4.6×) and `variant_match_loop`
     (~40×) auto-OSR.
   - **Shipped since (same infra):** **nested structs** (recursive dissolution),
     **multi-field variant arms** (N≥0 scalar fields/arm), **float captures**. **Remaining
     narrower broadening:** **heap-payload variant arms**, **live-out value reconstruction**,
     and **megamorphic** sites (stay un-inlined, conservative). All reuse the shipped
     ip-map / region-gate / resume-remap.
2. **OSR productionization.** Hot-backedge **auto-trigger — SHIPPED, default-on**: a
   per-function-candidate-gated backedge counter fires OSR at a threshold without
   `RSS_JIT_OSR` (now the eager path), with no interpreter regression (non-candidate
   functions pay one hoisted never-taken branch; the J1 lesson respected) and the
   profile-guided closure-inline sites re-probe past the pending window instead of
   giving up. **Production perf matrix — committed (clean), current source of truth
   `baseline-20260623-jit.json` (HEAD `4219800`):** 46 cases, 7 iters / 2 warmup, fresh worktree +
   cold target to defeat the warm-tree clock-skew. Confirms (reg_vm vs native, same machine):
   `recursion-linear` ~70×, `struct-field` ~45×, `float` ~33×, `closure-alloc` ~53×,
   `option_result_chain` ~10×, `match_option_loop` ~25×, **and the fold wins `string`
   (`string_build_scan`) ~13× and `bytes` ~20×**. (Supersedes `baseline-20260623.json` on `947cd54`,
   which predated `6bf1450`/`4219800`, and the deleted stale `current-run-20260622.json`.) Remaining
   still-at-parity cohorts: `string_text` (1.35 — `split` allocates), `json` (1.01), `string-map`
   (0.98), `recursion-tree` (1.04), async — see Pending #7.
3. **Full precise deopt beyond the leaf/scalar subset (J0.1 frame-chain, J0.2, J0.4,
   J0.5).** Side-effecting / native-heap-write deopt + deopt-after-write correctness;
   generated-code `VmLimits` accounting instead of the "refuse native while
   limits/cancel armed" fallback. Becomes necessary once J2/J3-in-OSR or heap writes
   make frames non-trivial.

   **J0.1 remaining — design (the heap-aware-reg-reconstruction core + precise-default
   shipped 2026-06-27; these three slices remain, each a deliberate change to the
   deopt state-map format / OSR-exit, NOT a one-liner — and none has a *correctness*
   driver today, so the differential cannot gate a reconstruction bug by output alone;
   they must be driven by dedicated repro tests):**
   - **(a) Live-out / escaping aggregate OSR — ALREADY COVERED (correctness + OSR)
     via native heap writes; only a perf refinement remains.** A loop-carried struct
     read after the loop or returned already OSRs through the transactional heap-write
     path (`native_osr_loop_carried_struct_live_after_loop_uses_heap_writes`,
     `..._escaping_uses_heap_writes` — both green). The scalar loop-carried-struct SR
     pass declines (`passes.rs:7430`) but OSR proceeds with the un-dissolved body
     (host-helper `SetFieldSlot` writes, journaled). So this is **not a capability
     gap** — the only refinement is scalar-replacing such live-out structs for fewer
     host calls (record a recipe `(orig_reg, layout, [leaf_reg])`, rebuild at OSR-exit);
     pure perf, low priority.
   - **(b) Live-after variant/Result OSR — SLICE 1 SHIPPED (2026-06-28); heap-payload
     sub-case remains.** **Shipped:** a live-after **always-`Ok` Result with a SCALAR
     `Ok` payload** now OSRs via a reconstruction recipe. The RESULT-SR pass
     (`native_scalar_replace_results_in_region`) records `(variant_reg, payload_reg)` for
     each RES register that is read after the region, **iff** (i) no RES register is
     written after the region (pre-loop init is fine), and (ii) the `Ok` def is reached
     UNCONDITIONALLY each iteration (single in-region def, no branch between header and
     def except the loop-exit condition) so `payload_reg` is definitely-assigned. The
     `OsrEntry.variant_reconstructs` recipe is validated at the build site (payload reg
     type must be `Int`/`Float` — the deopt ABI is scalar-only) and applied at OSR-exit
     (`tier.rs`): rebuild `Ok(payload)` = `VmValue::Variant(result_ok_layout(),
     [payload])` into the variant slot, or, after 0 iterations, leave the correct
     pre-loop value. The reconstructed value is observed after the loop, so a wrong
     recipe diverges → **caught by the differential** (not a silent path). **The Option
     analog also shipped** — a live-after always-`Some` Option reconstructs `Some(payload)`
     via the same machinery (`OsrEntry.some_option_reconstructs`). Tests:
     `native_osr_j3_live_after_always_ok_result_reconstructs` +
     `native_osr_j3_live_after_always_some_option_reconstructs` (positive);
     `native_osr_j3_escaping_result_does_not_osr` (branchy inlined `checked` ⇒ `Ok` def not
     unconditional) and `native_osr_j3_escaping_option_does_not_osr` (conditionally `None`
     ⇒ not always-`Some`) still decline (`osr_entries==0`).
     **Remaining (the original hard sub-case):** an `Err`/heap-payload arm that is
     actually TAKEN and live after the loop — a dissolved heap payload has no live heap
     object to write back. Approach: extend the recipe so a heap field references its
     interpreter-visible SOURCE register (loop-invariant live-in), reconstruction reads
     `frame.reg(source)`. Subtle (in-loop-built heap payloads have no live source) — a
     further deliberate slice.
   - **(c) Inlined logical-frame-chain format.** A deopt inside a `native_inline_leaf_calls`
     region resumes at the caller's `CallKnown` and re-runs the (side-effect-free)
     callee — *correct today*. Full J0.1 reconstructs the logical callee frame in place
     (reusing the `DeoptChildSite`/`DeoptFrame` machinery that already serves
     `CallNative`), by recording `(callee_id, callee_ip)` per inlined safepoint and
     threading it through the OSR ip-map composition. Pure refinement (perf + a
     prerequisite for side-effecting inlined deopt); lowest priority, highest
     ip-map-composition risk.
4. **Checked-`Int` arithmetic (J4.3) — SHIPPED with a measured win.** Proof-only interval
   elision **and** J4.3b induction-bounded refinement are landed: a counter under `i < N`
   now emits `i + 1` unchecked (**~16% on `native_scalar_loop`**, the canonical
   `i < N; i += 1` counter shape).
   Honest ceiling reached: an unbounded accumulator (`total += x`) cannot be made
   unchecked without changing overflow semantics (a deopt-on-overflow still must compute
   the overflow check to know when to bail), so it correctly stays checked. **Remaining
   (future):** only speculative deopt-on-overflow or opt-in `WrappingInt` — both
   language/semantics decisions, lower priority than Pending #1.
5. **Later J4 passes (secondary, data-gated):** range-check elimination, branch pruning,
   loop unrolling. **Range-check elim is sandbox-unsafe if the proof is wrong (OOB)** —
   highest care, lowest priority.
6. **Adaptive-tiering polish:** dev/startup threshold tuning; full generated-code limits
   accounting if native is to run *under* armed budgets/cancel rather than be refused.
7. **Next performance slices (ROI order, from the committed `baseline-20260623-jit.json`).**
   The slow alloc-bound/scalar cohorts are now native wins; the still-at-parity cohorts rank:
   - **`bytes` — two SEPARATE facts (don't conflate):**
     - *Old `baseline-20260623.json` cell (on `947cd54`, pre-fold) was NOISE, not overhead.* It
       lists `bytes` at nat/reg 1.75 (native 36 ms), but that does not reproduce: 7 back-to-back
       clean re-measurements gave **~20 ms (parity 0.99)** with native-tier stats all zero — a
       transient load/throttle spike during that one 7-sample window (samples ramp 29→38). The
       profitability gate was fine; there was nothing to fix *at that commit*.
     - *Post-`4219800` (Bytes length-fold) `bytes_scan` now OSRs to native.* A non-escaping `Bytes`
       built only to be measured (`Bytes.len` of `Bytes.slice`/`Bytes.from_string`) folds to
       byte-length arithmetic, deleting the allocation; the report prints `osr: entered`. **Committed
       post-fold matrix `baseline-20260623-jit.json` (HEAD `4219800`): `bytes` reg_vm 20.7 → native
       1.0 ms, nat/reg 0.05 (~20×)**, osr_entries=1.
   1. **Native string/Bytes kernels — SCOPED (2026-06-23): the named kernels are
      ALLOCATION-bound, so this is bigger than first framed.** `json`/`string-text`/`string`/
      `string-map` (~138 ms) are dominated by string *allocations* (`from_int`/`concat`/`slice`/
      `split`/`pad_left`/Map-key hashing — heap writes), not by the read-only accessors
      (`String.len`/`starts_with`). Sound native read-only accessors (lengths, predicates,
      indexed byte reads via host-helper reads, mirroring `list_len`/`list_get`) are a clean
      in-subset foundation but **do not move these kernels** — the allocations still bail. To
      actually win them needs ONE of: **(a) native heap writes** (J0.4 / Pending #3 — allocate
      strings/bytes in native code with deopt-after-write correctness; large, foundational, and
      unlocks general native allocation), or **(b) query-folding** — eliminate a non-escaping
      constructed value used *only* for a read-only query by folding it to arithmetic.
      **(b) — String LENGTH folding SHIPPED (commit `6bf1450`, read-only / §7.2-safe):** a
      non-escaping string used only via `String.len` folds to length arithmetic
      (`len(concat(a,b))=len(a)+len(b)`, `len(from_int(k))=`digit-count incl. sign/0/`i64::MIN`,
      `len(slice)=`clamp — slice folds **only when the source is provably ASCII**, else bails;
      `String.len` is byte length, verified against the interpreter). **Committed post-fold matrix
      `baseline-20260623-jit.json` (HEAD `4219800`): `string` (= `string_build_scan`) reg_vm 34.7 →
      native 2.6 ms, nat/reg 0.07 (~13×)**, osr_entries=1, differential byte-identical incl.
      force-deopt + non-ASCII corpus. **Still at parity / not folded (same committed matrix):**
      `string_text` (native 58.1 ms, nat/reg 1.35 — the `String.split` List allocation blocks it; the
      1.35 native-slower cell may also be transient, like the earlier `bytes` cell — not
      investigated), `json` (1.01, parse), `string-map` (0.98, Map-key hashing). These need
      `List.len(split(...))` count-law / `starts_with` byte-compare folds, or the general allocation
      path (a) for programs that actually *use* the constructed strings; each is its own slice.
   2. **Native-call ABI — the large architectural keystone (IN PROGRESS, sliced).** Unlocks
      `recursion-tree`/`fib` (~192 ms today via the tier-0 scalar executor, not Cranelift),
      bounded recursion generally, and non-inlinable cross-function native calls. Non-recursive
      native call chains already ship (`CallNative` + child-frame deopt); the remaining work is
      **native recursion**, blocked by (i) self-reference (a `CallNative` resolves its callee via
      `self.funcs`, but a self-call's id isn't minted until compile returns → needs a self-call IR
      form + declare-before-define), and (ii) a **host-stack depth guard** (native→native calls the
      callee on the host C stack at lib.rs CallNative codegen, so unbounded recursion overflows it
      → a crash, not a clean bail; safety-critical). Sliced:
      - **Slice 1 — depth-carrying ABI: DONE.** `CompiledAbi` gained a trailing `depth: usize`
        param; the top-level `call()` passes 0, each `CallNative` passes `caller_depth + 1`. Carried
        only (no entry guard yet); behavior unchanged, validated by vm-jit 81/0 + differential 33/0.
      - **Slice 2 — self-call lowering + entry depth guard** (next): a `CallSelf` IR form,
        declare-before-define in `compile_inner`, and an entry check that bails (deopts to the
        interpreter) when `depth >= cap` so the host stack can't overflow.
      - **Slice 3 — enable + verify** (`fact`/`fib` go native, byte-identical to interp, deep
        recursion bails cleanly at the cap), then **slice 4 — mutual recursion** (declare a cycle
        before defining). Interacts with the frame-chain deopt / state maps (Pending #3, J0.1).
   *Net (FRONTIER REACHED, 2026-06-23): the cleanly-tractable incremental wins are DONE* — folds
   (string/Bytes length, ~13×/~20×), scalar replacement (Option/variant/struct/nested/loop-carried),
   closure-sink (~53×), TCO (~70×), float (~42×), deopt-before-heap (~10×), plus the registry +
   missed-opt report + heap-result ABI (S0). *Every remaining item now gates on a LARGE FOUNDATIONAL
   KEYSTONE, proven by this session's feasibility studies + two honest STOPs:*
     - `json`/`string-map` (genuinely *use* allocations / collection builders) → **heap-write S1–S4**
       (S0 done; S1–S3 allocate-only no-prereq but the alloc-bound benchmarks mostly MEASURE strings —
       already folded — so the real benchmark payoff is at **S4** = collection builders = **J0.1 + J0.5**).
     - `string_text` → **nested-loop OSR** (the `split` delimiter-count needs an inner scan loop the
       single-natural-loop OSR rejects).
     - `fib`/multi-function hot code → **native-call ABI** (= **J0.1** + host-stack guard).
     - heap-payload variants / live-out reconstruction → **J0.1** (heap-aware deopt state maps).
   *So the shared keystone for most of the heavy unlocks is **J0.1 (inlined frame-chain deopt / heap-aware
   state maps)**, with **nested-loop OSR** the other. These are deliberate multi-slice foundational
   investments that show no benchmark win until substantially built — not incremental slices. The perf
   push has harvested the tractable frontier; what remains is foundation work, to be chosen deliberately.*

   **CODEGEN-QUALITY DIAGNOSIS (2026-06-24) — native-vs-Rust gap, corrected.** A read-only investigation
   found the matrix's "native 2–8× slower than Rust" was MOSTLY **measurement inflation** (fixed per-eval
   recompile overhead on small loops) + the **irreducible OSR floor (~1.26×:** an OSR'd loop carries the
   enclosing frame as live registers vs a Rust fn whose loop owns its registers). True per-iteration gaps:
   `struct-field` 6.5×→2.6×, `variant-user` 6.7×→2.84×, `osr-struct` 2.1×→1.97× — and those are **0 host-
   helper calls** (fully scalar-replaced), so ~no recoverable codegen beyond ~1.3×. **Checked-int is NOT
   the dominant lever** (native-scalar carries the same overflow checks at 1.07×). The 20–52× kernels scale
   linearly (NOT LLVM loop-deletion) but native handles them by *bailing to the interpreter* — a coverage
   gap, not codegen. **The ONE real codegen win — SHIPPED (`9447c41`):** `int-arith`'s 4.7× was dominated
   by **2 host-helper calls/iter** (`List.get`+`List.len` on a loop-invariant typed list inside an OSR
   loop). Extending direct typed-list reads (`ListGetIntDirect` + flat OSR-window marshalling, TV2 borrow-
   pinned) and hoisting the invariant `List.len` to the OSR path cut it to **0 calls/iter → native 19.6→
   6.6 ms, native/rust 4.66×→~1.6×**, generalizing to any OSR loop over a fixed typed list. Read-only
   (§7.2 holds; OOB still deopts); conservative (mutated/aliased/non-typed list bails to the helper path).
   Net: the general codegen lever was real but NARROWER than the raw matrix suggested — that one win is it.
   *(Out of scope / low ROI: `deep-copy` (alloc-bound, already nat/reg 0.68); the async cohort
   `selfhost-mailbox`/`async`/`async-taskgroup` (suspending → native-ineligible); small collection
   kernels (need native collection kernels / deforestation, modest absolute).)*

### Next-phase architecture — a general hot-region compiler (adopted direction, 2026-06-23)

The slice-by-slice wins have built, in effect, a **typed hot-region compiler with side-exit**:
`try_osr` compiles a hot loop *region*, runs region-scoped passes (inline → scalar-replace →
combinator-expand → length-fold), composes their ip-maps, and **side-exits to the interpreter via
`Bail`/`OsrExit`** (always-correct fallback, §7.2). The next phase is to **generalize and formalize
that machinery** (LuaJIT/trace-JIT-style + RSS static types — *not* a JVM clone), so future wins are
reusable mechanism instead of hand-coded passes. This supersedes chasing J4.4/J4.5/J4.6 (range-check
is sandbox-risky, branch-pruning needs the regressed branch-profiling, unrolling is marginal).

**The pipeline to converge on:** (1) detect hot *regions* (not just whole functions / one natural
loop); (2) carry **facts** — type, ownership/escape, **effect (pure/read/allocate/write/suspend)**,
and deopt state-maps; (3) lower only the *proven-safe* region to Cranelift; (4) side-exit/deopt on a
failed guard or unsupported path; (5) interpreter stays the oracle. *(Refinement: do NOT build a new
SSA IR up front — the escape/effect/foldability facts are already computed per-pass (`RegFootprint`,
the fold fixpoints); first extract them into **reusable analyses over `RegInstr`** fed by the registry
below. Build a dedicated mid-level IR only when pass interactions actually demand it; the existing
`RegInstr`-rewrite + ip-map + `OsrExit` substrate already carries region compilation.)*

**Levers, in adopted priority (risk-adjusted ROI):**
1. **Intrinsic/effect registry — SHIPPED (commit `ee526ef`, behavior-preserving).** `IntrinsicDescriptor`
   (effect = pure/read/allocate/write/suspend; `can_fold`; `native_lowerable`; `view_capable` reserved
   for the view work; `combinator_kind`; `string_fold_role`; `cold_arm_pure_builder`; `notes`) with a
   conservative `Default` (allocate / not-foldable / not-native-lowerable) for the ~637 intrinsics, and
   explicit descriptors for the ~13 the JIT special-cases. The three hand-coded sites
   (`native_subset_instruction`'s `IntToFloat` admission, the Option/Result combinator-expansion
   recognition, the string-length-fold producer/query classification) now **classify via the table**;
   each keeps its exact lowering/fold/expansion mechanism. Proven a clean refactor: both differentials
   33/0 incl. force-deopt twin, jit_acceptance unchanged, all wins intact; a unit test locks the table
   contract. Most of the 637 intentionally use the default — populating more is incremental future work.
   This is the source of truth every future pass (and the report below) reads.
2. **Missed-optimization report — SHIPPED (commit `f58cf91`, behavior-neutral).** Env-gated
   `RSS_JIT_REPORT` developer diagnostic; per hot function/loop it prints the verdict + reason —
   `native: ok` / `osr: entered` / `not native: contains CallIntrinsic BytesSlice (effect=allocate; …)`
   / `not osr: loop body contains allocating intrinsic …` / `not osr: no loop` / `not inlined: callee
   not native-inlinable`, with the intrinsic-level reasons sourced from the registry's `effect`/`notes`.
   **Observational** (style A): a separate read-only re-derivation walks each function and re-runs the
   same predicates the passes use, touching no pass; OSR/native *positives* trust the recorded runtime
   outcome (so a function that actually OSR'd always says `osr: entered`), and the static re-derivation
   only explains genuine non-entries. Gated off by default (a `report` bool read once into `NativeState`
   like `collect_stats`); proven behavior-neutral — differential 33/0 incl. force-deopt with the report
   **both off and on**. Makes the JIT **self-explaining** (the antidote to this session's diagnosis cost:
   the bytes false alarm, the struct-field auto-OSR confusion, the `option_result` veto). 5 report-
   correctness tests assert each verdict matches the real `osr_entries`/native outcome.
3. **Then ONE big unlock — feasibility-studied 2026-06-23 (read-only studies, real effort data):**
   - **View-based strings/bytes — GO-WITH-CAVEATS, but it's an INTERPRETER win, not a JIT win.** Studied:
     strings/bytes are immutable and every consumer goes through TWO accessors (`expect_string_ref`/
     `expect_bytes_ref`, ~277 sites), so a `View{parent,off,len}` arm done **TypedVec-style behind the
     existing `Rc`** (preserve discriminant + size) is observationally identical to a copy with a small
     hash/eq-normalization (the `OptionSomeScalar` precedent). Effort **S1–S3 ≈ medium** (~450–650 LOC +
     parity tests). BUT the payoff is almost entirely **interpreter-side** (slice stops *copying*, helps
     all 5 tiers' interpreter runs); it does **NOT** push any kernel into native code, and does NOT make
     `split` (still allocates a List), `json` parse, or used-`concat` cheaper. Making `starts_with(view)`
     etc. go *native* needs a new content host-helper + `IR_VERSION` bump (a separate, larger "S4"). So:
     real, low-risk, broad interpreter speedup — but **orthogonal to the heap-write JIT unlock, not a
     de-risking of it.**
   - **Native heap-write transaction layer — GO-WITH-PREREQS; splits into a tractable half and a large
     half.** Studied: adopt **effect-after-commit ordering** (NOT checkpoint/rollback — rollback fights
     the existing re-run-from-top fallback). **S0 — heap-result return ABI — SHIPPED (`95e2b71`,
     §7.2-safe infra):** `NativeOutcome::CompletedHandle` + a VM-owned output table materialized ONLY on
     clean completion (cleared on bail), so native can *return* a heap value it was given with no
     allocation and §7.2 holds verbatim (IR_VERSION→10; scalar path byte-identical; differential 33/0 +
     force-deopt). **S1–S3 (allocate-only) — SHIPPED & green (verified 2026-06-28).** 10
     `AllocatesResult` host helpers (`StringFromInt`/`StringConcat`/`StringSlice`/`StringPadLeft`/
     `StringSplit`/`StringLiteral`/`JsonParse`/`JsonField`/`BytesSlice`/`ListNewInt`) `publish_heap_result`
     a fresh *unaliased* value into `JIT_HEAP_RESULTS`, gated by `escaping_output_handle` escape analysis so
     the result is genuinely returned/consumed. Implementation note vs. the original plan: rather than
     charging `mem_budget` at the host-helper boundary + no-bail-tail, the shipped design keeps `mem_budget`
     EXACT by **Model-A refusal — native is declined while `mem_budget`/`step_budget`/`cancel` is armed**
     (`tier.rs` `native_limits_unarmed`), so there is **no in-helper accounting and no double-charge
     possible** (simpler and equally exact). §7.2 holds via clear-on-bail output table + `JitHeapResultsGuard`
     (force-deopt test `native_heap_result_force_deopt_leaves_output_table_empty`). Needs NEITHER J0.1 NOR
     J0.5 (confirmed). Tests: `native_string_from_int_return_allocates_heap_result`,
     `native_string_concat_handle_feeds_string_len`, + vm-jit lib 83/0, runtime 401 functional pass.
     **S4 (in-place mutation of caller-aliased heap — the fully general "write anywhere" unlock) is
     monolithic and requires BOTH J0.1 (frame-chain state maps) AND J0.5 (in-code mem accounting)** — the
     "big scary" layer. Biggest/broadest payoff (~138 ms alloc-bound + general). The moment S1 lands,
     `mem_budget` must join the native-eligibility gate (parity).
   - **Native string-READ host helpers (`string_len`/`string_byte`) — NEW, §7.2-SAFE, low-risk; the
     real string-plateau unblock.** Finding from the `string_text` fold attempt (2026-06-23): the shipped
     length-folds work only because the folded value is *built in-region* (its length is already a tracked
     Int register). Queries on an **external** string (`string_text`'s loop-invariant `line`:
     `List.len(split)`/`pad_left` len/`starts_with`) need to *read* the string's length/bytes natively —
     but the host-helper ABI has list reads (`list_len`/`list_get`) and **no string read**. Read-only
     `string_len`/`string_byte` helpers (mirror `list_len`/`list_get`, §7.2-safe) were **designed +
     validated** (correct values + OOB/non-string bail; ABI unit-tested) but **NOT landed — no unblocked
     consumer.** DEEPER FINDING (2026-06-23): folding `string_text` needs MORE than the helpers — its
     `List.len(split(line,","))` over a runtime-length external string requires a per-byte **delimiter-
     count SCAN loop** (no closed form), and emitting that scan inserts a **second backedge** the OSR
     pipeline rejects (`detect_single_natural_loop` admits only ONE natural loop; no nested/inner-loop
     OSR). So **`string_text` actually gates on NESTED-LOOP OSR**, a separate capability — the
     `pad_left`/`starts_with` folds are straight-line-expressible but useless while the `split` scan can't
     be emitted. Land the read helpers only once a consumer exists (nested-loop OSR, or a kernel reading
     `String.len` of an external string in a single loop).
   - **Native call ABI (best for recursion / function-heavy code).** The native tier has **no call
     instruction** (whole-function, inline-only); a native→native/native→VM call ABI with a host-stack
     depth guard + frame-chain deopt (J0.1) unlocks `recursion_fib` (~192 ms), bounded recursion,
     non-inlinable helpers, multi-function hot code. Lower §7.2 risk than heap-write-S4 for comparable
     payoff (the feasibility finding behind deferring `fib`).
   - **Revised read of the fork (from the studies):** views ≠ the JIT plateau-breaker (it's an
     interpreter win); the tractable JIT alloc win is **heap-write S0–S3** (no prereqs); the big
     monolithic items (heap-write S4, call ABI) both gate on **J0.1 frame-chain deopt** — so J0.1 is the
     shared keystone for the heavy unlocks.
4. **Numeric policy (gated language decision, low priority).** Proof-based checked-`Int` removal is
   shipped (J4.3); further parity/vectorization needs a decision: safe-only vs `WrappingInt` vs
   `UncheckedInt`/`FastInt` vs speculative overflow-deopt. Until then some scalar loops stay slower than
   Rust.

**Measurement discipline (lesson from this session):** benchmark cells go stale (warm-tree clock-skew →
`current-run-20260622.json`) or noisy (a transient spike made `bytes` look 1.75× when it is parity).
Treat as source of truth: a committed clean matrix (current: `baseline-20260623-jit.json` on HEAD
`4219800`; regenerate after fold/perf commits) + independent re-build re-verification + the missed-opt
report for "did it engage" — never a single bench cell. (Corollary: when a perf commit lands a fold/
native win, regenerate + commit the matrix so the doc's source of truth stays current — the
`baseline-20260623.json`→`-jit.json` regen after `6bf1450`/`4219800` is the worked example.)

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

(Updated 2026-06-21 to reflect shipped state — see the current-state table above.)

| HotSpot | We have | Gap |
|---|---|---|
| Interpreter (profiling) | reg-VM interpreter **+ warm-gated call-site type feedback (J1 shipped)** | branch/range profiling not shipped |
| C1 (baseline JIT) | Cranelift native tier (~29 ops, scalar) + typed-list direct reads + **mono/poly closure inlining (J2)**, **Option/variant/struct scalar-replacement (J3)**, **range-proven unchecked arith (J4.3)** — all shipped, **firing on real kernels via auto-OSR** | accumulators stay checked (J4.3); the gated tail (J0.4 writes, J4.4 range-check) remains |
| C2 (optimizing JIT) | **mid-end shipped: profiling + mono/poly inlining + escape/scalar-replacement** | not measured faster; J4.4/J4.5/J4.6 future |
| Deoptimization | **resume-at-guard shipped (J0), default-OFF**; production still re-runs from the top | full inlined-frame / side-effecting deopt future (J0.1 frame-chain, J0.4 writes) |
| Uncommon trap / OSR | **OSR SHIPPED — production, auto-fires by default (J5.2 + auto-trigger + OSR×J2/J3 composition)**; ~39–75× on micro-kernels, ~40× `variant_match_loop`, ~4.6× `dynamic_closure_call` | heap-payload variant arms, live-out reconstruction, megamorphic sites |
| MethodData (profiles) | **per-call-site type feedback (J1 shipped)** | — |

The single most important existing asset: a **deopt *model* already exists** — the
native tier can bail and the interpreter takes over (the §7.2 fallback; the
`force_bail` backend, which refuses native *before execution* and runs the function
on the interpreter, verified bit-identical by the differential). But it is the crude
form in two ways: it's a **pre-execution off switch, not a mid-code guard**, and the
fallback is **re-run the whole function from the top** — sound *only* because the
compiled subset is side-effect-free (a re-run can't double-apply a write — §7.2). In the
pre-J0 baseline the native ABI collapsed every bail to an anonymous `None`, with no
safepoint identity. **(Shipped since: J0.0 replaced this with
`NativeOutcome::Deopt { safepoint_id, live }` — `crates/vm-jit/src/lib.rs`; this
paragraph describes the original problem, not current state.)**
Every step here that compiles side-effecting or speculative code needs the
**precise** form: resume *at the guard*, mid-function, after side effects — which
requires a new deopt ABI (J0.0) and full state maps (J0.1). That is J0, and it is
also exactly what perf-plan **§3.2w (heap writes in native)** needs — so J0 pays off
twice.

**Non-negotiable deopt ABI fact:** live values must be **pushed out by generated
code while the compiled frame is still alive**. Rust cannot safely inspect dead
machine registers or a dead compiled stack frame after `NativeModule::call` returns.
The first workable ABI therefore extends today's `out_ptr`/`bail_ptr` pattern with
a host-owned deopt payload buffer, or calls a deopt helper before unwinding. Any
design based on "record locations, then read them back after return" is impossible
at the safe boundary.

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
- **`force_bail` is NOT a guard — it's a pre-execution off switch** *(pre-J0 baseline
  framing; the ABI half is now shipped — see below).* Be precise:
  `force_bail` in `RegVm::try_native` returns a deopt *before* compiling/running native
  code ("pretend it bailed at the first guard"), so the interpreter runs the function
  from the top. It verifies the *fallback* path, not mid-code deopt. In the pre-J0
  baseline the native ABI collapsed every bail to an anonymous `None`
  (`NativeModule::call -> Option<i64>`) with **no safepoint identity**; **J0.0 has since
  replaced that with `NativeOutcome::Deopt { safepoint_id, live }`.** So "deopt at every
  safepoint" was **genuinely new machinery** (J0.0 ABI + real guards), not a
  generalization of `force_bail` — and it is now built. Once built, a
  **"deopt at every safepoint" stress backend** that must stay bit-identical to the
  interpreter on the whole soak is the killer correctness test for J0+.
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
  budget-tick opportunity." Per Exec-Spec §6.2, native/optimized code must
  **enforce or be ineligible**: while `step_budget`/`mem_budget`/`cancel` is active it
  compiles **only if** it emits the equivalent check **on every loop backedge** and at
  a **bounded straight-line interval** (the same points the interpreter ticks) — a
  native `while true {}` must still trip the budget — and it must **poll `cancel`**.
  Once J0.4/J3 compile *allocating* code, the current `mem_budget` exemption (the
  subset allocates nothing) **dies**: native allocations must be charged to
  `mem_budget`. And **deopt must not double-count**. Concrete invariant: at native
  entry record a `VmLimitsSnapshot { steps, live_bytes, host_calls }` — **accounting
  counters only**. (`cancel` is deliberately NOT in the snapshot: cancellation is
  external, monotonic state, not VM work already paid — generated native code must
  **poll the current cancel state**, and deopt must **never restore or roll back**
  cancellation. Snapshot/restore covers paid-work counters; `cancel` is observed,
  never restored.)
  Native code increments the same logical counters at the same abstract tick points;
  on deopt, restore the register/frame state at the safepoint with counters reflecting
  exactly the work already paid in native, then resume the interpreter *after* that
  safepoint so it does not re-pay those ticks. If a slice cannot prove this invariant,
  it is ineligible while limits are armed. Differential tests must run with
  `step_budget`/`mem_budget`/`cancel` *enabled*, not just the default unlimited runs
  (a J0/J6 deliverable).
- **State-map/code-size budget.** Deopt metadata must be lazily emitted only for
  compiled optimized functions and capped/measured beside machine-code size. A J-step
  that doubles resident code+metadata for a marginal win fails the same measured-win
  discipline as runtime perf.
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
  direct reads. The first typed-list loop investigation found the remaining gap is
  not the list load anymore; it is checked `Int` arithmetic (`sadd_overflow` +
  branch per `total += x` / `i += 1`), which also blocks vectorization.

```
 reg-VM interpreter ──profiles(J1)──► Optimizing tier (our mid-end)
   │  (cold, instant)                   ├─ J2 monomorphic inlining + guards
   │                                    ├─ J3 escape analysis + scalar replacement
   │                                    ├─ J4 checked Int arithmetic / range-check elim / branch prune
   ▲  deopt (J0: resume-at-guard)       └─ Cranelift egraph + regalloc + codegen
   └────────────── guard fails ─────────────────┘
```

---

## J0 — Precise deoptimization foundation (DO FIRST; everything depends on it)

> **Status (2026-06-21): SHIPPED for the leaf/scalar subset, default-OFF.** J0.0–J0.3
> are landed and every-safepoint-stress + full-soak verified. Caveats: precise resume
> is gated behind `RSS_JIT_PRECISE_DEOPT` (production re-runs from the top, which is
> byte-identical for the side-effect-free subset); the state-map is `resume_ip + live
> registers`, **not** yet the full inlined logical-frame-chain format J0.1 describes;
> and J0.4 (writes) / J0.5 (generated-code `VmLimits`) are future. The unchecked
> boxes below are design intent — see the current-state table at the top.

Replace "re-run from function top" with "reconstruct interpreter state at the guard
and resume." This is the prerequisite for *all* speculation and for §3.2w writes.

- [x] **J0.0 A real deopt ABI (FIRST — nothing works without it). SHIPPED.** The
      pre-J0 boundary was `NativeModule::call -> Option<i64>`: success or a *single
      anonymous* bail, with **no safepoint identity** — so "resume *where*?" was
      unanswerable. **Shipped:** `call` now returns
      `NativeOutcome::Completed(value) | Deopt { safepoint_id, live }`
      (`crates/vm-jit/src/lib.rs`), naming the deopt point.
      **Crucial mechanics — the live values must be pushed OUT, not read back.** Once
      `call` returns to Rust the compiled stack frame and machine registers are gone,
      so "record where values live and read them after return" is impossible across the
      safe boundary. **Shipped protocol: (a) the host-owned deopt payload buffer** —
      `call` passes in a buffer pointer (like `bail_ptr`); at a guard the generated code
      **writes `safepoint_id` + the live values** (scalars as bits) into the buffer
      **before returning** `Deopt`, and the host decodes it. (This extends the existing
      `out_ptr`/`bail_ptr` model where generated code already writes into host-owned
      buffers while the frame is live.)
      - **Alternative, NOT taken — (b) a deopt runtime helper** called while the frame is
        live (generated code calls a host helper that performs the deopt before the
        compiled frame unwinds). Kept on file only as a fallback if a future need (e.g.
        very large payloads) makes (a) unattractive; (a) shipped and is the contract.
      Either way, **the generated code materializes the values; the host never reaches
      into the dead frame.** Every guard gets a stable `safepoint_id`.
- [ ] **J0.1 Deopt metadata — FULL frame + window state, not just live values.** For
      each safepoint (`safepoint_id`), record what it takes to reconstruct the
      *complete* interpreter state. Design this format **from day one for inlined
      frame chains**, even though J2 ships later: if J0 only supports one logical
      frame, J2 will force a format break at the exact point correctness risk peaks.
      The map is the **decode schema for the J0.0 deopt payload** (the layout the
      generated code writes), paired with where each decoded value is restored — *not*
      a list of machine-register locations to read after return (impossible per J0.0).
      It must cover:
      - **register-window values** — for each, how to decode it from the payload
        (scalar bits+tag, or handle-table index → rebuilt heap/scalar-replaced value)
        and which RegInstr register it restores to;
      - the **`written` bitmap** for the window — `RegVm::reg` asserts the slot is
        written, and `RegVm::prepare_frame` deliberately clears written bits *and drops
        stale heap values* for ownership/`mem_budget` accounting. Deopt must restore
        written bits **consistent with the resume point** (every slot the resumed
        interpreter reads must be marked written and hold the right value; unwritten
        slots must own nothing);
      - the **`Frame` metadata** (`Frame { base, ret_dst, mut_writeback }` in
        `crates/rsscript/src/reg_vm/mod.rs`) — for **every logical frame the safepoint
        sits in**: once J2 inlines callees, one compiled frame can collapse a chain of
        logical frames (outer caller → inlined callee → …), and deopt must
        **re-expand the whole chain** — re-push a `Frame` for each, innermost resuming
        at `ip` and each outer one at its call site;
      - the resume **`ip`**.
      Store beside the compiled code, keyed like the native cache, emitted lazily and
      size-accounted per the state-map/code-size constraint in §2.
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
- [~] **J0.5 Exact `VmLimits` enforcement in generated code (spec §6.2, normative).**
      **STEP + CANCEL shipped for the OSR tier (2026-06-28).** The armed OSR variant
      emits a per-instruction `step_budget` tick (one per instruction — matches the
      interpreter's `tick()` 1:1, since a `resume_ip` is a shared instruction index), a
      `steps > step_budget` test + `cancel` poll at **every loop header** (= every
      backedge, incl. nested loops), and writes the accumulated `steps` back on every
      native exit (clean `Return` + the shared `fallback` deopt edge). On a trip it
      bails (resume_ip = header) to the interpreter, which raises the error — so there
      is one logical tick stream across native and interpreter with no double-/skipped
      count, and `cancel` is observed (never restored). ABI: a host `[steps,
      step_budget, cancel_addr]` cell via the new `limits_ptr` param (`call_with_limits`);
      unarmed variants ignore it (byte-identical, zero hot-path cost). The whole-function
      and recursive tiers stay on the ineligible (refuse-while-armed) branch.
      **Remaining:** charge native allocations to `mem_budget` (the "subset allocates
      nothing" exemption — tied to J0.4 S4; OSR is still refused while `mem_budget`
      armed) and extend enforcement to the whole-function/recursive tiers. A
      limits-enabled differential (J0.3) is still the desired broader net beyond the
      targeted `native_osr_*_step_budget` / `_cancel_*` acceptance tests + hostile suite.
- **Exit:** deopt-at-every-safepoint backend green on the full soak — **including runs
  with `step_budget`/`mem_budget`/`cancel` enabled** — this is the spine. **Scoped
  rule:** the *leaf/scalar* deopt subset (shipped) is what unblocked the J2/J3/J4.3/J5.2
  scaffolds, which speculate over side-effect-free code and bail via re-run-from-top —
  sound without the full machinery. Do **not** build **side-effecting or full
  inlined-frame** speculation (J0.4 writes, J0.1 frame-chain, OSR×J2/J3 composition)
  until the precise-resume + frame-chain forms are rock-solid. (Original wording was "do
  not build J2+ until rock-solid"; narrowed now that the side-effect-free scaffolds have
  shipped correctly on the subset.)

## J1 — Profiling in the lower tiers (the data speculation needs)

> **Status (2026-06-21): [~] SHIPPED (call-site type feedback).** Per-call-site type
> feedback on dynamic sites is landed, warm-gated, no interpreter regression,
> determinism-preserved. **Remaining broadening:** branch profiling (NOT shipped — it
> regressed `bool_logic_loop` ~7%; needs a hot/cold dispatch split) and optional value
> ranges. The unchecked sub-boxes below are kept for design intent.

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

> **Status (2026-06-22): SHIPPED — fires on real kernels via OSR.** J2.1 (monomorphic)
> + J2.2 (polymorphic 2–3) closure inlining are landed and differential-green, guard-bail
> via re-run-from-top; J2.3 (no deopt loops) satisfied by the give-up counter. **Broadened**
> to **capturing** closures (scalar-capture materialization via a `closure_capture`
> helper), **stored** closures (operand may be any native-readable handle —
> `GetFieldSlot`/`ListGet` result, not just a param), and **polymorphic** dispatch.
> **Composed into OSR** (Pending #1), this makes the literal `dynamic_closure_call`
> allocation-free native — **~4.6×**, auto-firing by default. (Standalone whole-function
> J2 outside OSR remains a coverage scaffold; the wins come through OSR.) **Float captures
> now shipped** (scalarized via `f64::to_bits`). **Remaining:** megamorphic sites and
> non-scalar (heap) captures stay un-inlined (conservative).
>
> **Bounded shape multiversioning (2026-07-26): SHIPPED.** Whole-function and OSR
> dispatch caches are keyed by region/function plus a runtime `ShapeKey`. The key
> contains only stable marshalling metadata: parameter ABI class, closure function
> id, and interned concrete struct/variant layout identity. Scalar payloads and
> collection lengths are excluded. Each tier/site admits at most two versions; a
> third shape takes the generic interpreter/PIC fallback without compiling. Runtime
> mismatch/deopt counters and give-up decisions are per version, while invariant
> structural translation failure remains function-global. Every version still
> passes through the shared code/time admission budgets.

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

> **Status (2026-06-22): [~] SHIPPED — Option + variant + struct scalar replacement,
> composing with OSR (measured ~40–75× wins, clean rebuild on HEAD `947cd54`).** Escape analysis + a RegInstr rewrite
> pre-pass dissolve a **non-escaping, scalar-payload** `Option`, user-variant, or struct
> into tag/field registers (allocation-free native); bails re-run from the top (J3.3
> deopt-reconstruction deferred for the non-bailed-heap case). Conservative — any escaping
> use leaves the function unchanged. **Standalone** (whole-function) it's a coverage
> scaffold (the function must be fully native-subset). **Composed with OSR (Pending #1,
> SHIPPED):** an Option, user-variant, or struct loop inside an I/O-tangled once-called
> function is now allocation-free native — `osr_option_loop` **~45×**, `osr_variant_loop`
> **~42×**, `osr_struct_loop` **~75×**. **Shipped broadenings:** **recursive/nested
> structs** (`nested_struct_field`), **multi-field scalar variant arms** (N≥0 fields/arm),
> **loop-carried struct scalar replacement** (in-place field mutation → registers,
> `struct_field_rw` reg_vm 45.1 ms → native 1.13 ms, **~40×**), and **cross-function shapes** via
> OSR×inline-leaf-calls (`variant_match_loop` **~40×**). Also shipped on this infra:
> **deopt-before-heap** (cold-arm `Bail` + Result scalar-replacement + Option/Result
> combinator expansion → `option_result_chain` 40.4 → 4.27 ms, **~9.5×**) and **closure-allocation
> sinking** (`closure_alloc_loop` 30.6 → 0.57 ms, **~53×**). (All reg_vm-vs-native medians from
> the committed clean matrix `benchmarks/vm-jit/baseline/baseline-20260623-jit.json` on HEAD
> `4219800` via a fresh worktree + cold target, same machine.) **Remaining broadening:** heap-payload variant
> arms (live heap payload not bailed), live-out value reconstruction (a dissolved aggregate
> live *after* the loop), and megamorphic / non-scalar-capture sites — reusing the shipped
> OSR×J3 ip-map/region-gate infra.

Requires flat value representations to dissolve.
- [ ] **J3.1 Escape analysis** at the RegInstr/IR level: a value constructed in the
      function is *non-escaping* if it is not stored into a heap container, not
      returned, not captured by a closure, not written into a `Managed` cell, and not
      compared by identity (trivially true — value-semantic types compare structurally,
      closures are excluded (reference identity). **Blocking
      rule:** until audited otherwise, any value that flows through `Managed` or an
      interior-mutable cell is escaping by definition; guessing wrong here is unsound.
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
- [~] **J4.2 Typed-list loop optimization — INVESTIGATED; no standalone loop-load
      slice left.** TV1+TV2 already made `List<Int>`/`List<Float>` native read loops
      direct and fast (~3.7–3.9× faster; ~128× over interpreter on the pure sum shape).
      The follow-up investigation found:
      - sum/scan/fold-shaped loops over typed lists already reach native when their
        lowering is native-eligible;
      - a real native-eligibility bug was fixed and shipped: `DeepCopy` of an untyped
        or unused parameter no longer rejects the function, because `DeepCopy` is a
        no-op in the pure native subset;
      - LV1 len-hoist was implemented and CLIF-verified (one fewer length load per
        iteration), but same-machine A/B was perf-neutral (`~6.26 ms` vs `~6.28 ms`);
      - deleting bounds checks is not the next slice: checked `Int` overflow branches
        dominate the remaining gap, and a bad range proof would turn into an unsafe
        native read.
      Verdict: keep the DeepCopy eligibility fix; reject/defer LV1 and typed-list
      bounds deletion until the arithmetic problem is addressed.
- [~] **J4.3 Checked `Int` arithmetic / overflow-check strategy — (a)+(b) SHIPPED,
      measured win; (c) speculative future.** The dominant gap to compiled Rust on
      integer loops is per-iteration RSS `Int` overflow semantics: native emits
      `sadd_overflow`/`ssub_overflow`/`smul_overflow` + a bail branch on `total += x` and
      `i += 1`. **Shipped:**
      - **(a) proof-only interval elision** — `iadd`/`isub`/`imul` unchecked where the
        i128 result interval provably fits i64 (else checked).
      - **(b) J4.3b induction-bounded** — branch-conditioned refinement: a counter under
        a guard `i < N` gets `i ≤ N−1` on the in-loop edge, so `i + 1` is proven
        non-overflowing and emitted unchecked. **~16% on `native_scalar_loop`** (the
        canonical `i < N; i += 1` shape); `total += x` (unbounded) and `i + 2` / `i <= N` correctly stay
        checked.
      **Remaining (future, semantics-sensitive):**
      - **(c) speculative deopt-on-overflow** for unbounded accumulators — needs J0
        precise deopt, and note it still must compute the overflow check to know when to
        bail, so the win is bounded; or
      - explicit opt-in numeric semantics (`WrappingInt`, saturating) — a language
        decision.
      Original framing kept below for context:
      - range/induction analysis proves no overflow, then emits unchecked native
        arithmetic for that proven region;
      - profile-guided speculation with a guard/deopt-on-overflow, after J0 precise
        deopt exists;
      - explicit opt-in numeric semantics (`WrappingInt`, saturating ops, or an
        annotation) if the language wants a visible contract rather than proof;
      - keep current checked semantics as the default and accept the cost where proof
        is unavailable.
      Exit gate — **CLEARED for (a)+(b):** full parity + arithmetic-edge tests green
      (overflow traps preserved), no silent wrapping, no interpreter regression, and a
      same-machine win on `native_scalar_loop` (~16%). **Future gates** (for the
      remaining (c)): a measured win on accumulator-heavy / integer typed-list kernels
      (`native_sum_loop`) — which requires the speculative deopt-on-overflow or an opt-in
      wrapping mode, since pure proof cannot help an unbounded accumulator.
- [ ] **J4.4 Range-check elimination** using J1 value ranges + loop induction analysis
      (kills remaining bounds checks on hot list loops), guarded by deopt. This is the
      generalized form of the typed-list bounds work deferred in J4.2: when the loop
      shape or profile proves a range, compile one guard/deopt instead of a check on
      every load.
- [ ] **J4.5 Branch pruning**: a profiled never-taken branch is compiled as a deopt
      point, not real code (HotSpot uncommon trap).
- [ ] **J4.6 Loop unrolling** for tight counted loops (modest).
- [ ] **Vectorization: OUT OF SCOPE** (the long tail; document and stop).
- **Exit:** each pass soak-green and measured; no pass that doesn't pay its way ships.

## J5 — OSR (compile hot loops mid-execution)

> **Status (2026-06-22): J5.1 + J5.2 SHIPPED — production, auto-fires by default.**
> OSR-entry/exit (`compile_osr` in `vm-jit`, `try_osr` in `reg_vm`) over a **reducible
> single-exit loop** loads live-in regs from the interpreter window, runs the loop
> native, and OSR-exits via precise-deopt at the post-loop ip. A **hot-backedge
> auto-trigger** fires it by default (no flag; `RSS_JIT_OSR` is the eager path), with no
> interpreter regression. **OSR × J2/J3 composition** dissolves Options/variants/structs
> and inlines (leaf + capturing/stored/polymorphic closures) inside the loop. Validated
> by the OSR differential backend (byte-identical on the full corpus). Measured
> ~39–75× on micro-kernels and on the **literal** `variant_match_loop` (~40×) and
> `dynamic_closure_call` (~4.6×). **Remaining:** heap-payload variant arms, live-out
> value reconstruction, megamorphic sites (production perf-matrix: committed
> `baseline-20260623-jit.json` on `4219800`, current source of truth).

The perf-plan §3.4 finding: a once-called function with a hot inner loop never tiers
up (call count tops at 1). OSR fixes that class.
- [x] **J5.1 Spec amendment — DONE.** Exec-Spec §7 amended from "OSR not applicable" to
      the normative OSR-entry contract (mid-loop entry/exit state mapping + parity
      argument, dual-of-deopt).
- [x] **J5.2 OSR-entry/exit — SHIPPED, production (auto-fires by default).** Reuses J0's
      machinery as its *dual*: deopt maps compiled→interpreter; OSR maps
      interpreter→compiled entry at a loop header, and OSR-exit deopts back at the
      post-loop ip. Shipped over a **reducible single-exit loop** (`compile_osr`,
      `try_osr`); OSR differential backend byte-identical on the full corpus. Done:
      - [x] hot-backedge **auto-trigger** (candidate-gated counter, no interpreter
            regression; `RSS_JIT_OSR` is now the eager path);
      - [x] **OSR × J2/J3 composition** — inline (leaf + capturing/stored/poly closures)
            and scalar-replace Options/variants/structs *inside* the OSR loop region, so
            the alloc-bound cohorts auto-OSR allocation-free; ~39–75× on micro-kernels and
            the literal `variant_match_loop` ~40×, `dynamic_closure_call` ~4.6×.
      - [x] **production perf-matrix — committed (clean), current source of truth.**
            `benchmarks/vm-jit/baseline/baseline-20260623-jit.json` (46 cases) on HEAD `4219800`
            via fresh worktree + cold target; confirms recursion-linear ~70×, struct-field ~45×,
            float ~33×, closure-alloc ~53×, **string (`string_build_scan`) ~13×, bytes ~20×** (the
            fold wins). Supersedes `baseline-20260623.json` (on `947cd54`, pre-fold). Adjudication of
            the still-at-parity cohorts (`string_text` 1.35, `json` 1.01, `string-map` 0.98,
            `recursion-tree` 1.04, async) is pending.
- **Exit:** the once-called-hot-loop class **tiers up mid-run by default** (auto-trigger
      shipped); soak green; startup unchanged.

## J6 — Adaptive tiering policy (the full HotSpot ladder)

> **Status (2026-06-21): MODEL-A FALLBACK satisfied, not full accounting.** The ladder
> exists (interpret → warm-profile J1 → tier-up → give-up demotion), and limit-aware
> eligibility is handled by **refusing native dispatch while `step_budget`/`cancel` is
> armed** (`reg_vm/mod.rs` `try_native`; Exec-Spec §6.2 "enforce *or be ineligible*"). This
> is the valid Model-A path — NOT the generated-code `VmLimits` tick/poll/`mem_budget`
> accounting (that is J0.5, still future). So J6 is "Model-A satisfied," not "done."

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
                                                   J5 OSR SHIPPED — production, auto-fires by default; OSR×J2/J3 composition fires on real kernels
```
- **J0 is the spine** — months of work, correctness-only, must be bulletproof before
  any speculation. If J0 isn't solid, nothing above it is safe.
- **Typed-list loop optimization is no longer the next lane.** The post-Valhalla
  investigation showed the direct-read path already works, LV1 len-hoist is neutral,
  and the remaining measured gap is checked `Int` arithmetic. Do not keep grinding
  list-loop micro-passes until the arithmetic lever has a design.
- **Checked `Int` arithmetic is the next performance lever, but it is data-gated and
  semantics-sensitive.** Range-proven unchecked arithmetic can be non-speculative;
  this proof-only slice may be investigated before the rest of J4 because the data
  already points at it. Profile/speculative overflow deletion waits for J0 precise
  deopt. Any opt-in wrapping/saturating mode needs a language-level decision before
  implementation.
- **J2 (inlining) is the highest-leverage general win**; **J3 (scalar replacement) is
  the object/composite Valhalla payoff** but needs value-rep V1–V2 first.
- **Exit rule (revised — two tiers).** Every step exits on the correctness gate:
  deopt-at-every-safepoint backend green + full soak green. The *performance* gate is
  conditional on what the slice claims:
  - **Correctness / coverage-scaffold slices** (the ABI, deopt machinery, a coverage
    pass that makes a shape *eligible* but whose win depends on a later slice) may ship
    on **parity alone**, explicitly marked **"coverage scaffold / no speedup yet."**
    *Standalone* whole-function J2/J3 (outside OSR) were in this bucket initially.
  - **Slices that claim a performance win** MUST land a **measured beyond-noise win** on
    a real kernel, same machine, or be downgraded to "scaffold." Cleared this bar: **OSR
    (J5.2)** ~39×; **J4.3b induction** ~16%; and the **OSR × J2/J3 composition** firing on
    real kernels — `osr_{option,variant,struct}_loop` ~42–75×, the cross-function
    `osr_inline_variant_loop` ~69×, the capturing-closure `osr_closure_loop` ~15×, and the
    literal benchmark kernels `variant_match_loop` ~40× and `dynamic_closure_call` ~4.6×.
    So J2/J3 graduated from scaffold to measured wins *through* OSR.

## 6. Risks

- **Deopt correctness is the crown jewel.** Reconstructing precise interpreter state
  at every safepoint, through inlined frames, after partial side effects, with
  scalar-replaced values rebuilt — get it subtly wrong and you get silent divergence
  the soak catches only probabilistically. Mitigation: J0 first, deopt-at-every-
  safepoint stress backend, Miri on the reconstruction module, soak every step.
- **Deopt loops / pathological recompilation** — capped per §2 (give-up after K).
- **Profiling overhead on the dev-common interpreter path** — gate on warm-up;
  measure non-regression.
- **Checked arithmetic is a semantics trap** — RSS `Int` overflow is observable today
  through safe fallback/error behavior. Removing `sadd_overflow` checks without a
  proof, guard/deopt, or explicit language mode would silently change programs.
- **`Managed`/interior mutability is a J3 blocker, not a curiosity.** Escape analysis
  must conservatively treat it as escaping until a formal rule proves otherwise; a
  false non-escape decision can alias-mutate a scalar-replaced value.
- **State-map bloat can erase startup wins.** Metadata is constrained in §2, but each
  implementation slice must report code bytes + metadata bytes; lazy emission is
  required for dev-startup discipline.
- **Parity surface explosion** — every speculation × the soak. The discipline is the
  cost; the soak is the asset.
- **The long tail you won't match** — C2's vectorization + decade of heuristics. Honest
  ceiling ~3–5× off native, not ~1–2×. Don't pretend otherwise.
- **Effort** — multi-quarter. Staged so J0 (also needed for §3.2w) and J2 each ship
  standalone value even if the rest stalls.

## 7. Open questions

- **State-map format details.** The constraint is fixed (lazy, measured, size-capped);
  choose the compact encoding: dense register tables, varint payload schemas, per-
  safepoint deltas, or another representation.
- **How much does Cranelift's egraph already cover** vs. what we must add (measure
  before building J4 passes)?
- **Inlining policy** — depth/size budget, profile thresholds; the heuristic that most
  affects results and is hardest to tune.
- **Checked arithmetic policy** — do we want only proof-based removal, speculative
  deopt-on-overflow, or explicit opt-in numeric modes such as wrapping/saturating
  arithmetic?
- **Interaction with the 5-backend parity** — the AOT/compiled-Rust backend doesn't
  speculate; confirm it always computes identical values (it does today; re-confirm per
  speculation family).
- **OSR compiled-entry — ANSWERED: OSR has its own entry.** `compile_osr` builds a
  dedicated OSR-entry (load live-in from the window → jump to the loop-header block),
  separate from the normal param entry.
- **OSR×J2/J3 composition — ANSWERED: the OSR loop region runs the passes itself.**
  `try_osr` runs region-scoped `native_inline_leaf_calls` (leaf + closure-sink), then the
  Result/Option/variant/struct scalar-replacement transforms, and **composes their ip-maps**,
  remapping the OSR-exit `resume_ip` back to the original loop body (so the OSR translator is
  no longer identity-indexed). J2/J3 do **not** run first; the region passes produce the
  transformed→original maps that the resume mapping threads through. Remaining concern is
  scoped to budgets/metadata/perf (pass ordering cost, ip-map composition depth), not
  correctness — the differential (incl. force-deopt twin) is byte-identical.
- **`Managed`/interior-mutability values** under escape analysis — the default is
  "escaping"; audit whether any narrower non-escaping rule is sound before J3.
