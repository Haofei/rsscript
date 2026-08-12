# ADR 0110: Semantic ownership of structured flow state

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics` owns the `Flow` exit-state enum and the conservative
non-fallthrough merge rule used by structured block analysis. Compiler CFG and
body checks import that type rather than defining a compiler-private equivalent.

## Consequences

This establishes a neutral dataflow vocabulary for the remaining CFG migration.
It preserves current behavior and does not change source, artifact, Provider,
or runtime contracts.
