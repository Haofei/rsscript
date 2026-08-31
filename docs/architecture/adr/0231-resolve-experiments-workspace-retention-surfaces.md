# ADR 0231: Resolve the experiments-workspace retention surfaces (Prove/retain)

## Status

Accepted

## Problem

`docs/architecture/experimental-retention.toml` lists seven experiments-workspace
surfaces with `decision_by = 2026-12-31` and empty evidence: `aot-backend`,
`aot-model`, `aot-runtime`, `artifact-store-integration`, `reir-model`,
`selfhost-parity`, and `reir-review-integration`. Each must reach a
Prove / Cut / Extend decision under the retention program, and none may be
removed or retained on assertion — measurement decides.

None of the seven is depended on by any crate in the supported workspace
(`crates/`, `tools/`). Their `removal_rule`s, however, do not cut on "no
supported consumer" alone; each protects a specific retention condition —
differential evidence, an independently consumed versioned model, conformance
value, an integration boundary, or a regression-detecting research corpus — and
cuts only when that condition is *also* absent.

## Decision and non-goals

Each surface's named retention workload was run in the `dev` container
(`cargo test --workspace` in `experiments/`, commit
`9db57648b4e7314c0deb999a50a01a9ffaa0e021`); all pass. Each therefore
demonstrates the value its `removal_rule` protects, so the cut condition (no
consumer **and** no demonstrable value) holds for none of them. All seven are
resolved **Prove/retain**, `status` flipped `pending` → `proven`, with the
consolidated evidence artifact `experiments/retention-evidence.json` (pinned by
`evidence_sha256`) attached to each entry.

Per surface:

- **aot-backend** — retained as the differential benchmark harness. Its
  `aot_jit_matrix` harness emits the `rsscript.aot_jit_matrix.v1` records the SDK
  `native_jit_scorecard` consumes for interpreter/JIT/AOT parity. This is the
  "freeze as a benchmark harness" arm of its rule, not the remove arm.
- **aot-model** — `source_map_contract_round_trips` passes; the model is
  independently consumed by both `rsscript-aot-backend` and `rsscript-aot-runtime`,
  so it remains an independently consumed versioned model rather than being merged.
- **aot-runtime** — `runtime_services_execute_and_shutdown_independently_in_parallel`
  passes (76 unit tests); standalone-runtime parallel execute/shutdown semantics
  retain interpreter-level differential value.
- **reir-model** — `reir_tests` passes (28, plus the full crate suite);
  independent conformance value demonstrated.
- **selfhost-parity** — `parser_parity_tiny_sample` passes and the corpus runs
  168 passing parity tests. It still detects regressions beyond the canonical
  Rust frontend, so its cut condition does not hold. Remains an explicit Research
  feature under ADR 0133 and ADR 0135.
- **artifact-store-integration** — `bounded_read_rejects_oversized_artifact`
  passes; the bounded-read security boundary over persisted artifacts is
  exercised. It has **no supported project/CLI consumer yet** and is retained on
  the passing boundary test; it is the first to revisit for Cut if no consumer
  emerges by the next review.
- **reir-review-integration** — the public review-integration facade passes and
  stays isolated from compilation validity. It likewise has **no external
  consumer yet** and is retained on its passing integration test, to be revisited
  next review.

This ADR does not graduate any surface into the supported SDK, does not change
the native engine or interpreter, and does not re-introduce the removed research
JIT surfaces. `proven` here means the retention condition is demonstrated, not
that the surface is a product contract. The two consumer-less surfaces
(`artifact-store-integration`, `reir-review-integration`) are retained on
conformance/boundary evidence and explicitly flagged for the next review.

## Compatibility and migration

No language, Artifact, Provider ABI, SDK runtime API, or persisted-data contract
change. All seven surfaces remain in the isolated `experiments` workspace, off
every supported build and publish set. `xtask validate-ci` accepts the attached
evidence (it already accepted the entries before `decision_by`).

## Verifier and security impact

None. The surfaces stay isolated from the Core build; the `artifact-store`
bounded-read boundary test that guards untrusted-artifact size continues to run.

## Provider and backend impact

No Provider or VM behavior change. `aot-backend` continues to serve differential
evidence for the JIT scorecard; the AOT path remains experimental and outside the
Core SDK.

## Evidence

- `experiments/retention-evidence.json` records the commit and the passing named
  workload for each surface; reproduce with `cargo test --workspace` in
  `experiments/`.
- `xtask validate-ci` is green with all seven entries `proven` and evidence
  attached.
