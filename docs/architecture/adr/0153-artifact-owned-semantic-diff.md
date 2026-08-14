# ADR 0153: Artifact owns semantic-diff schema and construction

- Status: Accepted
- Date: 2026-08-14

## Problem

`SemanticDiffV1` compared two provider-neutral Artifact Bundles, yet its
schema, fact types, and construction code lived in `rsscript-sdk`. That made
artifact inspection and review integrations depend on the embedding façade
instead of the persisted Artifact contract they consume.

## Decision

`rsscript-artifact` owns `rsscript.semantic_diff.v2`, all semantic-diff wire
types, and `SemanticDiffV1::between`. The SDK retains explicit re-exports from
its reviewed `analysis` module, so embedding callers keep a stable path while
CLI, runner, and integrations can consume Artifact facts without depending on
SDK implementation code.

## Non-goals

This does not make semantic diff an authorization or policy mechanism, change
the v2 schema, or replace the typed analysis-envelope work. It also does not
move execution reports into the Artifact crate: those are runtime observations,
not persisted build facts.

## Compatibility and migration

The public SDK names remain re-exported. Direct users of the implementation
module were internal-only and must use `rsscript_artifact` going forward. The
schema identifier and serialized output remain `rsscript.semantic_diff.v2`.

## Verifier, security, and backend impact

Semantic diff remains an optional, policy-neutral consumer of verified Bundle
facts. It cannot admit artifacts, choose Providers, or affect VM execution.
Backends and Provider implementations consume the same external-contract facts
without a dependency on SDK.

## Evidence

Artifact unit tests retain fact-normalization coverage. SDK schema tests and
CLI build/verify/diff workflow tests validate the re-exported v2 contract.
