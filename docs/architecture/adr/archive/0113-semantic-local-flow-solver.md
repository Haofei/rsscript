# ADR 0113: Semantic ownership of local-flow fixed-point solving

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics` owns the local-flow fixed-point solver. It interprets the
neutral graph's bindings, effect events, and cleanup edges to derive reachable
entry ownership states, including conservative branch joins.

## Consequences

Compiler local checks now consume semantic entry states rather than owning the
ownership transfer contract. Graph construction remains temporarily in the
compiler because it still adapts the checked HIR shape; moving it is the next
S02 step. No source, artifact, Provider, or runtime contract changes.
