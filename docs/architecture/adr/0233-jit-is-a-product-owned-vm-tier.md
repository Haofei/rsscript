# ADR 0233: The Cranelift JIT is a product-owned VM tier, not a retention-gated preview

## Status

Accepted. Supersedes the "no JIT feature work" clauses of ADR 0230, ADR 0231,
and ADR 0232, and the JIT freeze in
[`docs/planning/2026q4-experimental-decision.md`](../../planning/2026q4-experimental-decision.md).

## Problem

The native execution tier was introduced as a time-bounded experiment. Its two
surfaces in `docs/architecture/experimental-retention.toml` — `jit-tier0`
(the `native_status` tiering inside `rsscript-vm`) and `jit-cranelift-engine`
(the `rsscript-jit-cranelift` package) — each carry a `decision_by` date and a
`removal_rule` phrased as a survival test: merge tier 0 back into the
interpreter if fallback workloads do not show a 10-15% gain, and keep the engine
"preview-only" until controlled workloads justify a supported release surface.

That framing no longer matches the product. CPU-bound embedded workloads need
native execution, and the interpreter alone does not meet that need, so the
roadmap now carries native execution as a required VM tier with named active
goals (accounting parity, native coverage, promotion to Core). Two consequences
follow from leaving the retention clock in place:

1. The clock asks the wrong question. `decision_by` forces a periodic
   Prove/Cut/Extend verdict on *whether the engine exists*. That question is
   settled; re-litigating it every 90 days produces ADRs like 0230 whose real
   content is a coverage gap, not a survival argument.
2. The clock blocks the work that would answer it. ADR 0230, ADR 0231, and
   ADR 0232 each restate that the JIT "does not gain roadmap feature work" and
   is limited to correctness, security, dependency, and regression maintenance.
   Under that rule the two un-lowerable tier-0 workloads named in ADR 0230 can
   never become measurable, because closing a lowering gap is feature work.

At the same time, the evidence discipline the retention program created is
worth keeping. The scorecard in `benchmarks/vm-jit` is how the project knows a
native optimization is real rather than asserted, and nothing here should let an
optimization ship on assertion.

## Decision and non-goals

The Cranelift JIT — both the `rsscript-jit-cranelift` engine and the `jit-tier0`
tiering it rides on — is **product-owned**. It is not a retention candidate, and
it carries no decision date.

1. **The engine is not a removal candidate.** Neither surface has a
   `removal_rule`. Its existence is a product decision recorded here and in
   [`../../roadmap.md`](../../roadmap.md), not a recurring verdict.
2. **Individual optimizations remain evidence-gated.** Every native pass,
   region kind, and tier decision is justified through the controlled workload
   scorecard in `benchmarks/vm-jit` and its checked-in baseline. A measured
   regression is removed; an optimization that cannot show its gain is not kept
   on the argument that the engine is product-owned. The retention entries keep
   their `workloads`, `evidence_cases`, `minimum_end_to_end_gain_percent`,
   `evidence_uri`, and `evidence_sha256` fields, and `validate-ci` verifies that
   evidence exactly as it verifies a `proven` surface's.
3. **JIT work is active feature work.** The clauses in ADR 0230, ADR 0231, and
   ADR 0232 that limit Cranelift JIT changes to correctness, security,
   dependency, and regression maintenance, and the JIT freeze in the 2026 Q4
   experimental-decision log, are superseded for the JIT only. Those ADRs are
   otherwise unchanged, and the same clauses continue to bind Rust AOT, REIR,
   and self-hosting. ADR 0230's substance survives as a coverage record: the
   `mailbox-ring` and `closure-dynamic` shapes remain open native-coverage work
   rather than a pending Prove/Cut verdict.

The invariants that make the tier safe are **unchanged** by this decision:

- The JIT is opt-in. It is a non-default Cargo feature and a non-default VM
  option; no supported build enables it implicitly.
- It executes the same verified `rsscript.bytecode.v1` the interpreter executes.
  It does not add a second verification path or relax a bytecode check.
- It is not selectable by source or by an Artifact. A program cannot request
  native execution; only the embedding host can enable it.
- The interpreter is the semantic oracle. Every native region is differentially
  checked against it, and any divergence is a JIT bug.
- It is unavailable to the isolated runner until native execution reports the
  same deterministic step, allocation, cancellation, and deadline accounting as
  the interpreter.

Non-goals: this ADR does not promote the engine to Core, does not change its
maturity tier, does not make it a default SDK, VM, or CLI dependency, does not
add it to the release closure, and does not re-introduce the removed
`jit-speculation`, `jit-recursion-experimental`, or `jit-struct-sr-experimental`
surfaces. It does not weaken the retention program for any other surface.

## Compatibility and migration

No language, Artifact, Provider ABI, SDK runtime API, or persisted-data contract
change. No program's observable behavior changes.

The governance migration is confined to the retention inventory.
`docs/architecture/experimental-retention.toml` gains a third `status`,
`product`, for surfaces owned by the product rather than by a retention program.
A `product` entry:

- must not declare `decision_by` or `removal_rule`, so no stale clock or
  preview-only removal rule survives the transition;
- must live in the root workspace, so no experiments-workspace surface can
  escape its clock by relabeling itself; and
- has any declared evidence verified on the same terms as a `proven` entry —
  complete `evidence_uri`/`evidence_sha256`, a repo-local performance artifact,
  one evidence case per workload, and a derived gain at or above the entry's
  threshold.

`jit-tier0` and `jit-cranelift-engine` move to `status = "product"`. Both keep
`maturity = "experimental"`: the engine is an active product surface whose
maturity tier is still Experimental, and promotion continues to follow
[`../../feature-matrix.md`](../../feature-matrix.md). The seven
experiments-workspace surfaces resolved by ADR 0231 keep their dates and removal
rules unchanged. Rollback is a TOML edit plus a new `decision_by`.

## Verifier and security impact

None. Bytecode verification, Provider signature validation, runner admission,
and VM limits are unchanged, and the JIT consumes only already-verified
bytecode. The native tier's trust boundary is unchanged: it is available to a
trusted embedding host that opts in, and remains unavailable to the isolated
runner until accounting parity holds. Because the engine is opt-in and
host-selected, removing its retention clock does not widen any untrusted input's
reach.

The `product` status is deliberately narrow so it cannot become a governance
bypass: it drops only the clock, it is restricted to the root workspace, and it
keeps full evidence verification.

## Provider and backend impact

Providers are unaffected and receive no new authority. The reference
interpreter remains the semantic oracle and the definition of correct execution;
the JIT is an accelerator beneath it.

Active JIT work follows the roadmap's native-tier goals: close the accounting
gap so bounded and isolated execution can use native regions, grow native
coverage with a differential parity gate for every new region kind, and keep the
scorecard as the per-optimization evidence. Rust AOT, REIR, and self-hosting are
unchanged by this ADR and stay limited to correctness, security, dependency, and
regression maintenance.

## Evidence

- `classify_retention_status` in `tools/rsscript-xtask/src/main.rs` implements
  the `product` state, with the unit test
  `product_owned_surfaces_drop_the_clock_and_stay_root_only` covering the
  root-workspace restriction, the rejection of a retained `decision_by` or
  `removal_rule`, and the unchanged requirements on `pending`/`proven` entries.
- `xtask validate-ci` accepts the two product-owned entries and still verifies
  `jit-cranelift-engine`'s scorecard evidence digest and its 15% derived gain
  from `benchmarks/vm-jit/baseline/local-linux-aarch64.json`.
- The existing native differential, smoke, and scorecard tests in
  `crates/rsscript-sdk/tests` remain the per-optimization evidence path and are
  unchanged by this decision.
