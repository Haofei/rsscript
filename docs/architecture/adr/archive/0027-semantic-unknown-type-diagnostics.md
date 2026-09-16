# ADR 0027: Semantic ownership of unknown type diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics` owns builtin type roots, HIR alias lookup, and recursive
unknown-source-type diagnostics. Compiler provides only immutable source and
visible-protocol snapshots. Generic constraints remain separate work.

## Compatibility and migration

No language, Artifact, Provider, SDK, or runtime wire contract changes.

## Verifier and security impact

None; this is a pre-lowering diagnostic migration.

## Provider and backend impact

None.

## Evidence

Compiler and semantics test suites preserve existing diagnostics; architecture
tests verify semantic ownership of the public query.
