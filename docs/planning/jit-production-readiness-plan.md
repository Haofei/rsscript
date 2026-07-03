# JIT production-readiness plan — promote the native tier to production

Goal: promote the reg-VM native (Cranelift) JIT from a **development-tier
accelerator** to the **production execution tier for the interactive path** —
`rss run` ships with the JIT on by default, sandbox limits bind native code, and
realistic programs (the self-host frontend, agent tool loops) measurably speed
up. AOT (RSScript → Rust → `rustc`) remains the deployment and conformance
target; this plan does **not** change §0.0 authority order, only the engine's
shipping posture. It must hold the §2 parity invariant and §7.2 fallback proof
of [`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md)
at every step.

Status: **plan drafted from the 2026-07-02 production-readiness design review**
(three-agent audit of architecture, correctness infrastructure, and performance
state at HEAD `427cf064`). No phase started. Owner: TBD. Created 2026-07-02.

Source review verdict, one line: **the core correctness architecture is strong
(parity design, §7.2 fallback proof, confined unsafe surface), but not yet
proven at production breadth — the gaps are shipping posture, sandbox
composition, coverage on real programs, operational lifecycle, and the
verification breadth that would earn the word "production-grade" (F3).**

---

## 0. Scope decision (record of intent)

Three possible meanings of "production JIT" were considered:

1. Production-*quality* dev tier — largely achieved (readiness ladder L0–L3).
2. **Production execution tier for the interactive path** — CHOSEN. The VM+JIT
   is what actually executes untrusted/AI-generated/interactive programs; it
   must be shippable, sandbox-composed, and fast on real code.
3. Replace AOT as deployment target — REJECTED (contradicts language spec §21
   non-goals and the review-evidence product story).

Adopting (2) requires an explicit spec amendment (Exec Spec §0.1 wording +
Appendix B.1 calibration) in Phase 4 — a deliberate commit, not drift.

---

## 1. Findings register (evidence-backed, ranked)

Findings are numbered F1–F11 and referenced by the phases. File:line evidence
was verified at HEAD `427cf064` (2026-07-02).

### Tier 1 — production blockers

| # | Finding | Evidence |
|---|---------|----------|
| **F1** | **Compiled code is never freed; compile cost unbounded in aggregate.** `NativeModule.funcs` only grows; `JITModule::free_memory` never called; no eviction; no cap on compiled-code bytes or total compile time; `finalize_definitions()` re-runs per compile. Long-lived VMs leak executable pages monotonically; a program with many eligible functions burns unbounded host CPU in Cranelift, outside `VmLimits` authority. | `crates/vm-jit/src/lib.rs:2238-2248` (single-compile push/finalize; group site `:2352-2365`; no Drop/free anywhere) |
| **F2** | **Sandbox and whole-function native are mutually exclusive (Model A).** `try_native` refuses dispatch while `step_budget`/`cancel`/`mem_budget`/`host_call_budget` is armed — but a budgeted run *is* the production case for untrusted code. OSR already implements Model B for step/cancel (per-instruction step accumulator + backedge budget/cancel checks emitted in Cranelift, J0.5) **and has real `mem_budget` coverage**: the `JIT_MEM_CELL` charges `List.push` growth natively, with pinning tests (`native_osr_list_push_int_charges_mem_budget`, `native_osr_map_insert_loop_runs_under_mem_budget`). The remaining `mem_budget` gap is narrower than "no meter": (a) whole-function native still refuses armed `mem_budget` entirely, and (b) allocating helpers not covered by the mem cell need an effect/charge audit so a new allocating op can't ship un-metered. | `crates/rsscript/src/reg_vm/exec.rs:100-109`, `crates/rsscript/src/reg_vm/tier.rs:933-946` (Model A gate); `crates/vm-jit/src/lib.rs:4480-4521`, `:4092-4106` (J0.5 Model B); `crates/rsscript/src/reg_vm/tier.rs:2557`, `crates/rsscript/src/reg_vm/mod.rs:3064-3111` (mem cell); `crates/rsscript/tests/jit_acceptance.rs:6684`, `:5858` (tests) |
| **F3** | **Verification breadth is dev-tier while the optimizer expands weekly.** Per-PR `native-jit` CI runs the generative differential at default **4 cases/property** over a narrow grammar (integer sum-of-products); the 16-case seed-decoded sweep, both fuzzers, and ASan are weekly-only (`jit-hardening.yml`, cron). The generative differential compares **stdout only** — un-printed heap state after a forced bail is covered only by hand-written rollback tests. `clippy -D warnings` never lints the ~15K feature-gated lines of `crates/rsscript/src/reg_vm/native/` (583 gated blocks; the `vm-jit` crate itself IS linted as an unconditional workspace member). `panic=abort` is release-profile-only, so all CI tests cross the `extern "C"` seam under unwinding. No Miri on any JIT path (inherent for machine code; the safe-Rust rollback/helper side is Miri-able in principle). | `crates/rsscript/tests/common/differential.rs:30-32` (4 cases), `:264-291` (stdout compare); `.github/workflows/ci.yml` (native-jit job, no env); `.github/workflows/jit-hardening.yml` (weekly); `packages/test-runner/manifests/all.rsstest.toml:12` (clippy without features); root `Cargo.toml` `[profile.release] panic="abort"` only |
| **F4** | **Thread-safety by convention.** Bail flag / safepoint id / deopt payload are thread-locals; `NativeModule` is `!Send+!Sync` by accident of contents, not by declared contract. Safe under today's single-threaded scheduler; silently unsound if a VM ever executes on multiple threads. | `vm-jit/src/lib.rs:2756-2786` (thread-locals) |

### Tier 2 — the "worth shipping" gap (perf on real programs)

| # | Finding | Evidence |
|---|---------|----------|
| **F5** | **Coverage on realistic code ≈ 0.** Self-host ledger records `translated: 0` on real tool code (SH-001/006/011); local (non-parameter) collections, string/JSON building, and intrinsic-heavy loops fall back whole-function. Where the JIT fires it is near-Rust (nat/reg 0.01–0.02, ~0.9–2.3× of Rust); where it doesn't, it is a no-op. Coverage, not codegen quality, is the bottleneck. | `docs/ledgers/rss-selfhost-ledger.md:83-108` (SH-004); `benchmarks/vm-jit/baseline/*.json` |
| **F6** | **Live, unexplained baseline regression.** `option_result_chain` (~9.6× at `baseline-20260623-jit.json`) and `osr-multifield-variant` (~33×) decayed to ~1.1× (parity) in `baseline-20260626-six-framework.json`, while controls (`native-scalar` 0.017, `osr-struct` 0.013, `recursion-linear` 0.011) held. Baseline JSON lacks `osr_entries`, so cause (lost win vs mis-captured baseline) is undiagnosable from the file. Headline numbers must not be quoted until re-verified. | `benchmarks/vm-jit/baseline/baseline-20260623-jit.json` vs `baseline-20260626-six-framework.json` |
| **F7** | **Compile policy wrong for production.** Synchronous compile on the execution thread at `tier_up_threshold` default **0** (compile on first call) at `opt_level=speed`; the `opt_level=none` baseline mode exists (`NativeModule::new_with_opt`) but is not in the tiering ladder; no background compilation. | `reg_vm/mod.rs:1350-1353` (threshold 0); `tier.rs:1017-1026` (sync compile); `vm-jit/src/lib.rs:2026-2038` (opt levels) |

### Tier 3 — productization and hygiene

| # | Finding | Evidence |
|---|---------|----------|
| **F8** | **The JIT does not ship.** `native-jit` off by default; the default `rss` binary runs reg-VM + tier-0 only. Wiring already exists — `exec.rs` engages `try_native` inline behind `#[cfg(feature = "native-jit")]`, so shipping is a feature-default flip plus hardening, not new plumbing. | `crates/rsscript/Cargo.toml:50`; `reg_vm/exec.rs:1133-1369`; `cli/run_cmd.rs:222` |
| **F9** | **Spec/doc drift is material.** Exec Spec §6.2 still documents Model A as the only implementation (J0.5 shipped); §7/§10 still call OSR "specified, staged" (shipped, auto-fires at backedge 1000); `vm-jit` docs call precise deopt opt-in while the harness defaults it on (`mod.rs:1359-1365`); `IR_VERSION = 21` is dead doc (producer never reads it; crates are type-coupled); `benchmarks/vm-jit/README.md:51` says cost model defaults `off` (code default is `enforce`, `profitability.rs:44-49`); `CompiledAbi` prose documents params 1–8 of 10; `fuzz/README.md` references make targets that don't exist. Collectively the B.2 release-gate audit trail no longer matches reality. | cited inline |
| **F10** | **75-helper ABI is a hand-maintained drift surface.** Signatures macro-generated in `vm-jit` (`host_helpers!`), implementations hand-written in `reg_vm/mod.rs`, bound by string symbol; a semantic drift in one impl is a runtime-only catch. | `vm-jit/src/lib.rs:451-1099`; `reg_vm/mod.rs:3788+` |
| **F11** | **Scalar-only cross-call ABI.** Native call ABI is fully landed further than older docs claim — self-recursion AND mutual-recursion SCCs compile with frame-size-derived depth guards ([16,250] cap), `CallNative` chains deopts — but heap (`Handle`) params/returns in recursion groups fall back, and native call edges score neutral 0 in the cost model ("revisit after report-mode data"). | `tier.rs:2916-2925`, `:3048`, `:3107-3109`; `vm-jit/src/lib.rs:1248-1278`, `:4047-4085`; `profitability.rs:263-271` |

### Strengths to preserve (do not relitigate)

- §7.2 fallback proof: side-effect-free reads + journaled transactional heap
  writes + deopt-before-heap Bail splicing → "native can only be faster, never
  different". Every extension must keep passing through it.
- 6-way differential with permanent force-deopt / forced-safepoint / OSR twins.
- Unsafe surface: 1 fn-pointer type + 3 sites in `vm-jit` (`lib.rs:2243,2359,2681`),
  each with SAFETY prose; `forbid(unsafe_code)` in `rsscript`; W^X/MAP_JIT/icache
  delegated to `cranelift-jit`.
- Telemetry: `RSS_JIT_STATS` counters, `RSS_JIT_REPORT` "why not native/OSR"
  verdicts, per-function cost-model decline attribution.
- Anti-regression governors: profitability gate (**enforce by default**),
  no-amortize give-up (64), bail give-up (3), perf gate failing on unexpected
  native bails.

---

## 2. Phase plan

Ordering rule: trust → shippability → sandbox → coverage → default-on. Each
work item lists its acceptance criteria (AC). Every phase exits through the
existing B.2 differential release gate; no phase may leave `backend_differential`
red.

### Phase 0 — restore trust (days) [F6, F9]

| Item | Task | AC |
|------|------|----|
| P0.1 | Re-run the vm-jit baseline suite with `RSS_JIT_STATS=1`; diagnose `option_result_chain` + `osr-multifield-variant` decay (lost win vs suppressing capture config). Fix the regression or re-capture the baseline; extend the baseline JSON schema to embed `osr_entries`, cost-model mode, and env knobs so a capture is self-describing. | Both kernels back at (or consciously re-baselined from) their 06-23 ratios; every committed baseline JSON records the config that produced it; perf-gate fails if a capture omits it. |
| P0.2 | Doc-reconciliation pass: Exec Spec §6.2 (Model A note → hybrid A/B reality), §7/§10 OSR status (shipped, auto-on, threshold 1000), precise-deopt default (`vm-jit` doc vs `mod.rs:1359-1365`), `IR_VERSION` claim, `benchmarks/vm-jit/README.md` cost-model default, `CompiledAbi` params 9–10, `fuzz/README.md` make targets. | Spec/README claims match code at cited lines; add change-discipline rule: a capability change edits the spec in the same PR (extends §11). |

### Phase 1 — make it shippable (1–2 weeks) [F1, F3]

| Item | Task | AC |
|------|------|----|
| P1.1 | **Code-cache lifecycle.** Free compiled code with the owning VM (drop/`free_memory` path); add an aggregate compiled-code-bytes budget and a total-compile-time budget to `NativeState` (exceed → stop tiering, keep interpreting — never an error); batch `finalize_definitions` where compile sites allow. | A long-lived VM that tiers N functions and drops releases executable pages (asserted via a leak test); budgets observable in `RSS_JIT_STATS`; hostile many-eligible-functions program bounded. |
| P1.2 | **Clippy gate.** Add `cargo clippy -p rsscript --features native-jit -- -D warnings` (or `--all-features` workspace-wide) to the test-runner manifests; burn down existing warnings in `reg_vm/native/`. | CI fails on a new warning in `passes.rs`/`translate.rs`/`profitability.rs`/gated `tier.rs`/`mod.rs` blocks. |
| P1.3 | **Generative breadth per-PR.** Bump the `native-jit` CI job to `RSS_DIFF_PROPTEST_CASES=64`; extend the generator grammar toward the shipped optimizer surface (variants/Option/Result chains, closures incl. polymorphic sites, list/map ops on params AND locals, cross-function + self/mutual recursion). | Per-PR native differential exercises every producer pass family; job stays under an agreed wall-clock budget. |
| P1.4 | **Heap-state-diffing force-deopt property.** Add a generative property that, after a forced bail, compares full reachable heap state (not stdout) against pure-interpreter execution — closing the un-printed-mutation gap in the §7.1 rule 9 journal. | Property in `backend_differential.rs`, red under an intentionally broken rollback (mutation test), green at HEAD. |
| P1.5 | **Abort-seam job + fuzz cadence.** Add a CI job running the native seam tests under `panic=abort` (custom profile); promote the `differential --features native-jit` fuzzer and ASan smoke from weekly to nightly (bounded, e.g. 5–10 min each); keep weekly deep runs. | A panic introduced across the `extern "C"` seam aborts (not unwinds) in at least one gating job; nightly fuzz artifacts empty. |
| P1.6 | Encode the threading contract [F4]: debug assertion that a `NativeModule` is only called from its owning thread; doc the invariant on `CompiledAbi`/`NativeState`. | Violation panics in debug; contract documented where a future multi-thread implementer will read it. |

### Phase 2 — compose with the sandbox (1–2 weeks) [F2]

| Item | Task | AC |
|------|------|----|
| P2.1 | **Whole-function Model B.** Generalize J0.5 `LimitChecks` from OSR entry to whole-function native entry: step accumulator + backedge/entry budget-and-cancel checks; write-back on deopt keeps §6.2 rule 2 (no double-count). | A budgeted/cancellable hot function runs native and trips the budget with the identical fault class + step count as pure interpretation; differential twin for budgeted runs green. |
| P2.2 | **In-native `mem_budget` meter.** Replace the "declining allocating loops + narrow mem-cell" argument with charging at every native allocation site (list push growth, string concat/alloc helpers) against the host-owned mem cell; keep decline as fallback for un-metered ops. | A native loop that allocates trips `mem_budget` within the same bound as the interpreter; adding an allocating helper without a charge fails a lint-style test (helper effect-tag audit). |
| P2.3 | **`host_call_budget` decision.** Either charge host-helper dispatches natively (they are the same `Type.method` boundary) or keep ineligibility and document it as the deliberate §6.2 rule-1 branch. | Decision recorded in spec §6.2; test pins whichever branch. |
| P2.4 | **Hostile suite × JIT.** Extend the hostile-input suite to run budgeted AND JIT-forced (`step`/`mem`/`cancel`/`stdout`/`host_call` each armed, native + OSR + force-deopt). | Hostile suite green with native tiers active; a native path that escapes any budget is caught here. |

Phase 2 exit = **L2 (sandbox-sound) holds with the JIT actually running**, not
by making it ineligible.

### Phase 3 — real-program coverage (4–8 weeks, the long pole) [F5, F11]

Sequenced by leverage; each item lands behind the differential + a self-host
corpus benchmark. Success metric for the whole phase: **the self-host
lexer/parser corpus shows a real multiple (target ≥2× VM), and the ledger's
`translated: 0` entries (SH-001/006/011) go non-zero.**

| Item | Task | AC |
|------|------|----|
| P3.1 | **Local-collection support (ledger SH-004).** Native `List.push`/`Map.insert`/reads on locally-created collections (not just handle parameters) — the ledger's own named unlock for real tool loops. Route through the existing heap transaction; extend the generator (§2 consequence 4). | SH-006/SH-011-shaped loops tier up; force-deopt + heap-diff properties green; measured win on the corresponding kernels. |
| P3.2 | **Inline hot heap-read helper fast paths (followups Axis-B/#3).** Direct `TypedVec` len/buffer/field loads emitted in Cranelift with slow-path helper fallback — closes the ~13×-off-Rust heap-read gap without new deopt machinery. | `native-read-heap`-family kernels move toward Rust; no new bail sources (perf gate `bails == 0` holds). |
| P3.3 | **Heap (`Handle`) args/returns across the native call ABI.** Lift the `is_scalar` gate for `CallNative`/recursion groups; give native call edges a real cost-model weight from report-mode data. | Cross-function native code with list/struct params compiles; chained deopt across heap-carrying frames differential-verified. |
| P3.4 | **J0.1 keystone: inlined logical-frame-chain + heap-payload/live-out reconstruction.** The consciously deferred foundation (unlocks heap-payload variant OSR arms, live-out aggregates, S4 precise resume). Multi-slice; silent-bug-prone; requires dedicated forced-deopt repros per slice — differential alone cannot gate it. | Each slice ships with its forced-deopt repro; heap-payload arms that today splice to `Bail` stay native. |

### Phase 4 — flip the default (days, after 1–3) [F7, F8, plus scope §0]

| Item | Task | AC |
|------|------|----|
| P4.1 | **Compile policy.** Raise default `tier_up_threshold` above 0 with an `opt_level=none` first-compile → `speed` recompile-on-hot ladder (or measured acceptance of the current policy); document the chosen policy and its startup-latency envelope. | Cold-start regression budget met on the CLI benchmark set; policy in spec Appendix. |
| P4.2 | **Feature default flip.** `native-jit` into default features for the shipped `rss` binary; platform gate (Cranelift host ISAs; clean fallback to tier-0 elsewhere); kill-switch env (`RSS_JIT=off`); measure binary-size/build-time cost. | `rss run` executes native by default on supported hosts; `RSS_JIT=off` restores today's behavior; CI covers both. |
| P4.3 | **Spec amendment (scope §0).** Amend Exec Spec §0.1 + Appendix B.1: the VM+JIT is the production execution tier for the interactive path; define the production ladder P0–P4 below alongside L0–L4. | Spec merged in the same PR as the default flip. |
| P4.4 | **Helper-ABI hardening [F10].** Generate the 75 `extern "C"` impl stubs from the `host_helpers!` rows (or add a startup signature-hash self-check). | A signature drift between vm-jit descriptor and reg_vm impl fails at build (preferred) or module-init (fallback), not at first miscall. |

---

## 3. Production readiness ladder (P-ladder)

Extends, does not replace, the Exec Spec B.1 L-ladder (which stays the
soundness floor):

| Level | Guarantee | Phase |
|-------|-----------|-------|
| **P0 — Trusted numbers** | Committed baselines are self-describing and reproduced; no unexplained decay. | Phase 0 |
| **P1 — Operationally bounded** | Code memory and compile cost are budgeted and released; lint/abort/fuzz gates cover the native surface per-PR/nightly. | Phase 1 |
| **P2 — Sandbox-composed** | Every active `VmLimits` binds *running* native code (not via ineligibility); hostile suite passes with JIT on. | Phase 2 |
| **P3 — Real-program fast** | Self-host corpus ≥2× VM; ledger `translated: 0` entries eliminated for tool-shaped code. | Phase 3 |
| **P4 — Default-on** | Shipped `rss` runs native by default with kill-switch, platform fallback, and amended spec. | Phase 4 |

---

## 4. Non-goals (explicit, with reasons)

- **New SSA / heap-aware mid-level IR** — the plan-of-record NO-GO stands
  (`vm-optimizing-jit-plan.md`): the `RegInstr`-rewrite substrate is still
  delivering; build a dedicated IR only when pass interactions demand it.
- **Native async/suspend** (followups #9) — stays "maybe never"; `Suspending`
  remains an eligibility boundary.
- **Multi-threaded JIT execution** — no demand; F4's contract work is the
  prerequisite if it ever comes.
- **`HelperStatus` failure-kind enum** — binary bail stays until native code
  reconstructs errors itself (Exec Spec §7.1).
- **Cross-compilation / non-host ISAs** — host-only Cranelift is fine for this
  tier's role.

---

## 5. Measurement contract

- **Headline metric: the self-host corpus**, not the kernel suite. Kernels
  gate regressions; the self-host lexer/parser/checker runs (and ledger SH-*
  entries) decide whether coverage work is succeeding.
- Every baseline capture: release build, pinned machine note, `RSS_JIT_STATS=1`,
  embedded config (post-P0.1 schema).
- Perf-gate rules stay: regression threshold vs committed baseline, `bails != 0`
  fails unless explicitly allowed, telemetry minimums per kernel.
- Compile-latency and code-size budgets (post-P1.1) get their own gate columns:
  `compile_ms` and `compiled_code_bytes` regressions fail like wall-time ones.

## 6. Risks

| Risk | Mitigation |
|------|------------|
| P2.1/P2.2 limit checks tax hot loops | Unarmed variants already compile byte-identically (J0.5 precedent); gate with the perf suite; budgets off ⇒ zero-cost path preserved. |
| P3 coverage work widens the miscompile surface faster than verification | P1.3/P1.4 land *first* (breadth + heap-diff property); §2 consequence 4 discipline: no subset widening without generator extension. |
| J0.1 (P3.4) silent-bug class | Dedicated forced-deopt repro per slice; differential explicitly acknowledged as insufficient; keep slices small. |
| Default flip regresses cold-start CLI UX | P4.1 policy work precedes P4.2; kill-switch env; platform fallback. |
| Doc drift recurs after P0.2 | §11 change-discipline extension: capability PRs must edit the spec in-PR; review checklist item. |
