# ADR 0116: Semantic ownership of retained-closure flow facts

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics` derives retained-closure capture facts from checked HIR,
call-retention contracts, inline closure captures, and local ownership-flow
entry states. It owns the source display projection used by this semantic fact.

## Consequences

The compiler consumes `RetainedClosureCapture` facts only for diagnostics. No
source, artifact, Provider, or runtime contract changes.
