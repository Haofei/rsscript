# ADR 0135: Guard the self-host Research boundary mechanically

## Status

Accepted

## Problem

The self-host parity corpus is intentionally Research-only. A feature gate and
dedicated workflow can silently drift, allowing the corpus to become part of a
supported release gate again.

## Decision and non-goals

The Core architecture suite verifies that `selfhost-parity` is explicit in the
compiler manifest and test-module gates, that the dedicated workflow enables
it, and that release validation has no self-host parity invocation.

This does not move the corpus into a separate repository or change its test
semantics.

## Compatibility and migration

No language, Artifact, Provider ABI, SDK runtime API, or persisted-data
contract changes. The guard preserves the previously documented maintainer
feature invocation.

## Verifier and security impact

None.

## Provider and backend impact

None. The reference VM remains available to the opt-in Research harness.

## Evidence

`selfhost_parity_is_an_explicit_research_feature_not_a_release_gate` runs in
the SDK architecture suite.
