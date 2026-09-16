# ADR 0115: Semantic ownership of fresh-return flow facts

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics` derives `FreshReturnIssue` values from checked HIR and
the local ownership-flow entry states. It owns the interpretation of clean,
fresh-returnable, managed, and local bindings at a return boundary.

## Consequences

The compiler only consumes semantic facts to construct diagnostics. Legacy
ownership traversal remains during the migration until the broader moved-use
and retained-closure fact passes are consolidated. No source, artifact,
Provider, or runtime contract changes.
