# ADR 0230: Extend the jit-tier0 retention decision for un-lowerable fallback workloads

## Status

Accepted

## Problem

`jit-tier0` is a time-bounded experimental retention surface
(`docs/architecture/experimental-retention.toml`) whose `decision_by` is
2026-11-30. Its entry names three controlled fallback workloads —
`string-text`, `mailbox-ring`, and `closure-dynamic` — and its `removal_rule`
keeps the surface only if those workloads show a repeatable 10-15% end-to-end
gain.

Controlled local measurement (25 samples, `RSS_JIT_CONTROLLED=1`, release, in
the `dev` container) shows the surface cannot be given a clean Prove verdict,
because two of its three named workloads cannot be measured at all: the
canonical native-JIT scorecard compiler — the typed-MIR bytecode pipeline in
`crates/rsscript-lowering/src/mir.rs`, a deliberately narrower subset than the
interpreter — refuses to lower them.

- `closure-dynamic` (`benchmarks/vm-jit/kernels/dynamic_closure_call.rss`) fails
  with `unresolved checked HIR call`. The kernel exists to stress dynamic
  indirect dispatch through a first-class `owned Fn` fetched from a heap struct
  field (`op.apply` then `f(index)`). The checker marks that call
  `CallResolution::Unknown`, and `lower_direct_call` (`mir.rs:1656`) only accepts
  enum-variant calls, locally-declared closures tracked in `closure_abis`, and
  statically-resolved static/builtin calls. An indirect call through a stored
  `Fn` value is outside the typed-MIR subset **by design** — the native tier does
  not lower `CallClosure` dynamic dispatch.
- `mailbox-ring` (`benchmarks/vm-jit/kernels/mailbox_ring_only.rss`) fails with
  `unknown checked HIR local` while lowering `mailbox_take` (`lookup_place`,
  `mir.rs:3549`). That function combines a `mut` struct parameter, in-place field
  assignment, and an `Option<Int>` return with an early `return None`; a
  checker-introduced local in that shape is not materialized by the current
  lowering. This is a **lowering-completeness gap**, not a fundamental boundary.

Only `string-text` (`string_text_processing.rss`) lowers and runs; it clears the
10% bar (~1.2-1.3× end-to-end in the local baseline). One measurable workload
out of three is not a defensible Prove, and `jit-tier0` cannot be Cut in place:
it owns the base `native_status` tiering that the separately proven
`jit-cranelift-engine` rides on, so removing it would take the supported native
engine with it. That leaves Extend.

## Decision and non-goals

Extend `jit-tier0`'s `decision_by` from 2026-11-30 to 2027-02-28 (90 days, the
review cap). Within that window the surface must be resolved one of two ways:

1. Teach the typed-MIR lowering to compile these two shapes — indirect closure
   dispatch and/or the `mut`-struct `Option`-return place gap — so the named
   workloads become measurable and can be given a Prove or Cut verdict on their
   own numbers; or
2. Narrow `jit-tier0`'s `workloads` list to the shapes the canonical compiler
   supports (record-scalar/flat-data + statically-resolved calls), so the
   retention decision is measured against workloads the surface can actually run.

This ADR does not itself Prove, Cut, or narrow `jit-tier0`; it does not change
the native engine, the interpreter, or any observable behavior; and it does not
re-introduce the removed `jit-speculation`, `jit-recursion-experimental`, or
`jit-struct-sr-experimental` surfaces. It is a bounded deferral, not a
resolution.

Per the maintainer's local-first decision for this milestone, controlled
evidence is the checked-in canonical baseline
`benchmarks/vm-jit/baseline/canonical-linux-aarch64.json`, pinned by its commit
and recorded `evidence_sha256`, rather than a CI-hosted release artifact.

## Compatibility and migration

No language, Artifact, Provider ABI, SDK runtime API, or persisted-data contract
change. `jit-tier0` remains opt-in experimental and off the stable SDK path. The
native tier already falls back to the verified interpreter for both un-lowered
shapes, so no execution changes for any program. `jit-cranelift-engine` is
resolved separately in the same cycle: it is marked `proven` with the same local
canonical baseline attached (its three workloads clear the 15% threshold).

## Verifier and security impact

None. Untrusted-input validation, resource/cancellation behavior, and the
native-tier trust boundary are unchanged; the un-lowered shapes stay on the
interpreter exactly as before.

## Provider and backend impact

No Provider or VM behavior change. The follow-up work this ADR bounds is a JIT
concern only: either a typed-MIR lowering extension (indirect closure dispatch
and the `mut`-struct `Option` place fix) or a retention `workloads` narrowing.

## Evidence

- The `native_jit_pass_scorecard` scorecard (SDK, `--features native-jit`,
  `--ignored`) emits `mailbox-ring` and `closure-dynamic` as
  `unsupported_by_canonical_compiler` with the reasons quoted above, and
  `string-processing` as `entered` above the 10% bar. The two cases were added to
  the scorecard so the gap is visible and reproducible rather than silent.
- `benchmarks/vm-jit/baseline/canonical-linux-aarch64.json` is the controlled
  local baseline (25 samples, alternating order) backing both this extension and
  the `jit-cranelift-engine` Prove.
- `xtask validate-ci` accepts the extended `decision_by` and the attached
  `jit-cranelift-engine` evidence.
