# RSScript Decomposition Refactor Plan

Status: proposal (2026-06-24). Author: code review across reg_vm, frontend
checker, Rust lowering, and syntax/review/package. Grounded in a file-level read
of every target; line numbers are as of commit `408050a`.

This plan is **behavior-preserving module decomposition**, not a rewrite. Nothing
here changes language semantics, generated Rust, diagnostics, or VM/JIT behavior.

---

## 1. Why this is tractable (the one enabling fact)

Every oversized file is dominated by a **single inherent `impl`** (`impl RegVm`,
`impl RegLowerer`, `impl Analyzer`, `impl RustLowerer`, `impl Parser`) or is a
**flat free-function module** (`review.rs`). Rust allows:

- multiple `impl T { … }` blocks for the same type across different files of the
  same module, and
- child modules to access parent-module-private struct fields.

So decomposition = **move a cohesive cluster of methods/fns into a sibling file**,
add `mod x;` + `use super::*;`, adjust visibility to `pub(super)` where a sibling
now needs it. No logic moves. The compiler proves field/visibility correctness;
the parity suite proves behavior. The existing `reg_vm/` and `checks/` submodules
already use exactly this idiom — we are extending a proven pattern.

## 2. Principles & guardrails (from `ARCHITECTURE.md` + the spec)

1. **Parity Invariant is supreme.** reg-VM ⇄ HIR-interpreter ⇄ compiled-Rust must
   stay observationally identical. Any move is a bug if the differential/parity
   matrix changes.
2. **Checker internals are highest-risk and LAST** (`ARCHITECTURE.md` step 5):
   they carry the language invariants. Do cheap/leaf work first; carve invariant
   carriers one at a time, last.
3. **Co-locate invariants.** Never split a single rule across files mid-rule
   (effect inference, deopt/OSR restore, the TV2 borrow protocol, source-map
   emit↔parse, "unknown is never low-risk").
4. **No compatibility shims / aliases** (`ARCHITECTURE.md` Non-Goals). When a fn
   moves, update call sites; don't leave re-export stubs.
5. **Generated-Rust bytes are a frozen wire format.** Lowering moves must not
   reorder emits (shifts source-map spans + busts compiled-parity stdout caches).
6. **Keep struct *definitions* in the parent** (`mod.rs`) so child `impl` blocks
   inherit field visibility; move *methods*, not the struct.
7. `#![forbid(unsafe_code)]` holds in `rsscript`; never introduce `unsafe` into a
   moved module. Program faults are `EvalError` values, never panics; release
   tooling uses unwind only so boundary code can translate unexpected adapter
   failures before they reach an ABI seam.
8. The four Cargo test targets (`static`/`runtime`/`differential`/`soak`) are
   fixed; internal test modules live under them. Don't add new Cargo targets.

## 3. Verification strategy (per move)

Each move is one commit in its own worktree (see `worktree-shares-warm-target-volume`
in memory: `docker compose -p rsscript run …` from the worktree reuses the warm
cache; serialize with main-tree cargo). Gate ladder, cheapest first:

```
docker compose run --rm dev cargo test -p rsscript --no-run
docker compose run --rm dev cargo test -p rsscript --features native-jit --no-run
docker compose run --rm dev cargo clippy -p rsscript --tests -- -D warnings
docker compose run --rm dev cargo test -p rsscript --lib <moved-module>
docker compose run --rm dev cargo test -p rsscript
```

For **lowering** and **reg-VM intrinsic/exec/native** moves, the focused gate is
insufficient — run the full parity matrix before merging that phase:

```
docker compose run --rm dev bash -lc 'RSSCRIPT_FULL_BACKEND_PARITY=1 cargo test -p rsscript --test differential'
docker compose run --rm dev bash -lc 'RSSCRIPT_FULL_BACKEND_PARITY=1 cargo test -p rsscript --test runtime'
docker compose run --rm dev bash -lc 'RSS_DIFF_PROPTEST_CASES=200 RSS_GENERATIVE_CASES=64 RSS_GENERATIVE_MUTATION_CASES=200 cargo test -p rsscript --test differential'
```

A pure-movement diff is verified by inspection too: the host file should show
**only deletions + the new `mod x;` declaration**; the new file should be the
verbatim moved text plus `use` lines. (This is how the reg_vm/checker test-module
splits in `87980c6`/`408050a` were confirmed.)

---

## 4. Phased plan (ordered by value ÷ risk)

### Phase 0 — Cross-cutting cheap wins (LOW risk, high value). Do first.

These shrink files and remove real debt without touching invariants.

| # | Move | Files | Risk | Notes |
|---|------|-------|------|-------|
| 0.1 | **Dedup checker leaf helpers** → new `checks/shared.rs` | `analyzer.rs`, `checks/calls.rs`, `checks/local.rs`, `checks/body/{try_checks,fresh,forbidden}.rs` | LOW | `hir_expr_type_name` (×4), `builtin_value_type_name` (×4), `callee_display` (×3), `type_ref_name` (×3), `hir_expr_span` (×3), `expr_data_effect` (×2), `substitute_type_params` (×2). **Diff each pair byte-for-byte before merging** — a silent divergence would change diagnostics. |
| 0.2 | **Extract `rust_lower/rustc_remap.rs`** out of `source_map.rs` | `rust_lower/source_map.rs` (lines 10–227) | LOW | Pure fns: `remap_rustc_diagnostic_json*`, rustc JSON structs, `rustc_severity`, `best_source_map_entry`, `generated_span_from_rustc`, `generated_*_matches/contains`. Completes a sanctioned `lower/` target. `tests/checker_lowering.rs` imports `remap_rustc_diagnostic_json` directly — keep the path. |
| 0.3 | **Create `rust_lower/intrinsics.rs`** | move from `helpers.rs` | LOW–MED | `runtime_intrinsic_target`, `runtime_intrinsic_wants_managed_handle_arg`, the `is_*_callee` classifiers; wrap `runtime_abi::lookup_runtime_intrinsic`. Completes the last sanctioned `lower/` target. Do NOT reformat the 720-entry `runtime_abi` table. |
| 0.4 | **`bbom.rs` 3-way split** | `bbom.rs` (1135) → `bbom.rs` (model+extract+format), `bbom_policy.rs` (CapabilityDelta/MergePolicy/PolicyResult, lines 666–968), `bbom_reir.rs` (bundle bridge) | LOW–MED | Three physically separable concerns. |
| 0.5 | **Leaf util lifts** | `vm_value.rs` `FnvHasher` → `fnv.rs`; (optional) `formatter.rs` test block → tests | LOW | Trivial; do only if convenient. |

### Phase 1 — Low-risk mechanical splits of the free-fn / shallow-impl files.

| # | Move | Target layout | Risk |
|---|------|---------------|------|
| 1.1 | **`parser.rs` → `syntax/parser/`** | `mod.rs` (Parser struct + cursor + entry), `items.rs`, `stmt.rs`, `pattern.rs`, `expr.rs`, `types.rs`, `scan.rs` | LOW–MED |
| | | Most of parser.rs (line 930+) is already free fns over `&[Token]` — no shared mutable state. **Prereq:** promote the recovery/scan helpers (`skip_unknown_top_level`, `find_matching`, `statement_end`, `find_top_level_*`, `trim_outer`) to `pub(super)` in a single `scan.rs` — they must NOT be duplicated or reordered (error-recovery determinism). MED only because parser is foundational. | | |
| 1.2 | **`review.rs` → `review/`** | `types.rs`, `diff.rs`, `map.rs`, `facts_ast.rs`, `facts_hir.rs`, `mod.rs` (re-export public surface) | MED |
| | | Cleanest cut is between the **review-map** half (classification + facts) and the **semantic-diff** half (sig model + compare + contracts). `review/mod.rs` MUST re-export `ReviewMapClassification`, `ReviewMapFileRisk`, `review_map_sources`, `format_review_map_*` exactly (consumed by `checker_review.rs`, bbom, package). `facts_hir.rs` (managed-capture / handle-field analysis) is the entangled piece — extract it last within this phase. | | |
| 1.3 | **`package/review.rs` → +`review_await.rs`** | extract await-site analysis (lines 1015–end) | MED |
| 1.4 | **reg_vm leaf modules** | `reg_vm/value_ops.rs` (list-register ops + numeric/sorted/path/json/crypto free helpers), `reg_vm/resources.rs` (tcp/ws/pool/file I/O impl + resource-value constructors + `VmSender`/`VmReceiver`/`VmProcess*`/`VmStreamState`) | LOW–MED |
| | | These are leaf-like: Group G pure helpers + the cohesive resource maps. Big line-count win on `reg_vm/mod.rs` with low blast radius. | | |

### Phase 2 — Medium-risk impl decompositions.

| # | Move | Target | Risk |
|---|------|--------|------|
| 2.1 | **`lowerer.rs` (6239) sub-split** | keep `lowerer.rs` as the spine (struct + stmt/expr core), carve `lower_decls.rs` (A), `lower_async.rs` (C, ~900 LOC, cleanest), `lower_call.rs` (F), `lower_types.rs` (I), `lower_match.rs` (J+K), `lower_managed.rs` (H) | MED → MED-HIGH for `lower_call.rs` |
| | | Keep one `RustLowerer` struct; multiple `impl` blocks across files. **Do NOT extract free fns that change emit order.** Cluster D (source-map emit) is interleaved with E — leave it in the spine or move emit+parse together to `source_map.rs` (HIGH, see 3.x). | | |
| 2.2 | **reg_vm `impl RegVm` clusters** | `reg_vm/scheduler.rs` (C6 task/async/channel), `reg_vm/calls.rs` (C9 closure/call + C10 higher-order/tensor), `reg_vm/intrinsics/mod.rs` (C11 `call_intrinsic` dispatch) + `reg_vm/intrinsics/{json,string,list,map,bytes,date,math,char,path,option,result,set,deque,regex,hex,url,scalar}.rs` (C12, ~17 `exec_*_intrinsics`) | MED |
| | | Uniform `(unit, intrinsic, args, base, next_base)` signatures route to Group-G free fns — mechanical. Risk: a match arm landing in the wrong family file changes dispatch. Run the full parity + generative matrix. | | |
| 2.3 | **reg_vm native passes** | `reg_vm/native/passes.rs` (all `#[cfg(native-jit)]` Group-B analysis/fold/scalar-replace/inline, ~7000 LOC), `reg_vm/native/translate.rs` (`translate_to_native_jit`, `translate_osr_loop`, `detect_single_natural_loop`, `OsrLoop`/`OsrEntry`) | MED |
| | | Pure free fns; biggest volume reduction. Risk is `#[cfg(native-jit)]` propagation + `use` churn + the dual `OsrTrigger` defs. Keep the two `translate_*` fns together (sole producers of `vm_jit::JitFunction`). | | |
| 2.4 | **reg_vm IR model (optional)** | `reg_vm/model.rs` ← `RegInstr`, `RegIntrinsic`, `RegFunction`, `RegUnit`, profiling structs | LOW mechanically / HIGH blast radius |
| | | Every file imports these — do it in isolation, first or last in the reg_vm work. | | |
| 2.5 | **`hir.rs` split** | `hir/mod.rs` (data shapes, lines 16–471), `hir/lower.rs` (lowering, ~1083–2612), `hir/infer.rs` (type inference, 2613–end) | MED–HIGH |
| | | Data shapes are the shared vocabulary — keep stable in `mod.rs`. `infer_hir_expr_type` feeds every checker. | | |

### Phase 3 — High-risk invariant carriers. One at a time, full matrix between each.

| # | Move | Target | Risk |
|---|------|--------|------|
| 3.1 | **reg_vm exec core** | `reg_vm/exec.rs` ← `drive` (C7), `try_exec_pure` (C3), register/frame mechanics (C5), construction/limits (C1) | MED–HIGH |
| | | `drive` is the hot dispatch and reads nearly all `RegVm` fields; the written-bit protocol (§4.1) must stay intact. Keep `drive` whole. | | |
| 3.2 | **reg_vm native tier** | `reg_vm/native/tier.rs` ← C2 (`try_native`/`try_osr`/`run_jit`/`is_jit_eligible`), `NativeState`/`NativeAttempt`/`NativeStats`, the `JitHeapArgsGuard`/`JitHeapResultsGuard` FFI guards | HIGH |
| | | Contains the §7.2 "no native side effects before bail", §7.3 OSR parity, and the TV2 borrow-protocol "audited in one place" (mod.rs ~15900). **Move as one unit**; do not separate the guards from `try_native`/`try_osr`/`run_jit`. | | |
| 3.3 | **checker signatures/effects** | `checks/signatures.rs` ← analyzer C7 (signature/effects/retains/pure) + C8 (generics/protocols) + protocol-sig free fns | HIGH |
| | | Carries read/mut/take + retains + pure + native invariants. | | |
| 3.4 | **checker unknowns** | `checks/unknowns.rs` ← analyzer C9 (`check_unknown_*`, capability/binding/field/type) | HIGH |
| | | The "do not treat unknown as safe" rule — keep intact as one unit; easy to silently widen acceptance. Guard with `checker_frontend/{types,capabilities}.rs`. | | |
| 3.5 | **checker resource types** | `analyzer/resource_types.rs` ← analyzer C12 (resource/weak/Fd/pool type-level) | HIGH |
| | | Deliberately split from `checks/body/resource_pool.rs` (declaration vs body). Document the seam; re-run RS0702–0711. | | |
| 3.6 | **lowering source-map emission** | move cluster D (`record_*_source_map`) into `source_map.rs` | HIGH |
| | | Directly manipulates generated-Rust offsets mid-emission; highest parity sensitivity. Only attempt with the dedicated `tests/checker_lowering/source_map.rs` + full backend parity. May be left in the lowerer spine if not worth the risk. | | |

The remaining analyzer leaf clusters (diagnostics C13, derives C4, exhaustiveness
C6, syntax-support C3, runtime-guarantee C10, `AssignChecker`) are LOW–MED and can
be interleaved into Phase 1/2 to shrink `analyzer.rs` (6657 → ~700 target) before
the HIGH-risk carriers.

---

## 5. Risk register / landmines (consolidated)

- **reg-VM:** `#![forbid(unsafe_code)]` holds — don't add `unsafe`; the real
  indirect call is in `vm-jit`. Keep deopt/OSR/borrow-protocol co-located (3.2).
  Preserve dual `OsrTrigger` defs and every `#[cfg(native-jit)]`. Don't reorder
  the written-bit protocol. Keep `translate_*` as the sole `vm_jit::JitFunction`
  producers; never leak `RegInstr` to vm-jit.
- **Checker:** "unknown is never low-risk" must not regress (3.4). Effect
  inference (`apply_expr_effects` ↔ `BodyState` ↔ `ParamEffect`) is one logical
  unit across body/effects.rs + local.rs + hir.rs — don't fragment. Diagnostic
  code↔label pairs are pinned by `tests/static.rs::REQUIRED_SPEC_DIAGNOSTICS`.
  `run()` pass ordering is observable — don't reorder the call list.
- **Lowering:** generated-Rust bytes are frozen (source-map spans + compiled
  stdout caches). Source-map emit↔parse must move together. Rustc remap assumes
  `src/lib.rs`. The 720 `runtime_abi` `rust_target` strings are compiler-unchecked
  against `crates/runtime` fn names — don't reformat the table.
- **Syntax/review:** parser error-recovery (`skip_unknown_top_level` + friends)
  must stay one shared `scan` module — reordering changes which tokens get
  skipped. `review/map.rs` must keep classification + both propagation passes
  together (Unknown precedence). Formatter idempotency is pinned by
  `tests/metamorphic.rs`; the duplicate-looking `Session` structs in formatter.rs
  are test fixtures — do not "dedupe" them.

## 6. Suggested sequencing & rough effort

1. **Phase 0** (0.1–0.4): ~1 sitting; highest value/risk ratio; validates the gate.
2. **Phase 1** (1.1–1.4): parser/, review/, reg_vm leaf modules. Each its own
   worktree + commit; run `docker compose run --rm dev cargo test -p rsscript`
   between moves.
3. **Phase 2** (2.1–2.5): lowerer + reg_vm intrinsics/native + hir. Full backend
   parity matrix per sub-step; these are the bulk of the line-count reduction.
4. **Phase 3** (3.1–3.6): invariant carriers, strictly one-at-a-time, full
   matrix + generative fuzz between each.

Expected outcome: `reg_vm/mod.rs` 26K → ~1–2K (struct defs + entry points + `mod`
decls); `lowerer.rs` 6.2K → ~2K spine; `analyzer.rs` 6.6K → ~700; `parser.rs` and
`review.rs` become small `mod.rs` facades over focused submodules. Zero behavior
change, enforced by the parity suite at every step.

## 7. What NOT to do

- Don't rename `rust_lower/` → `lower/` (cosmetic, touches every path; treat the
  `ARCHITECTURE.md` names as logical).
- Don't carve the checker invariant carriers before the leaf work (Phase 3 last).
- Don't mass-rewrite the ~77 native-jit-only clippy warnings (57 `needless_range_loop`
  in hot codegen) — not in the gate, high churn, see `native-jit-clippy-gate-gap`.
- Don't add compatibility re-export shims for moved functions.
