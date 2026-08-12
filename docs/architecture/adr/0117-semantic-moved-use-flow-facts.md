# ADR 0117: Semantic ownership of moved-use flow facts

- Status: Accepted
- Date: 2026-08-12

## Context

Use-after-move diagnostics depend on checked HIR, exact expression evaluation
order, resolved field places, `take` pattern bindings, nested closure capture,
and the ownership state at each local-flow graph entry. Keeping that traversal
in the compiler duplicated the semantic rules that determine whether a value
has moved.

## Decision

`rsscript-semantics` derives `MovedUse` facts through
`moved_uses_from_flow(HirBlock, entry_states)`. The fact pass consumes the
semantic local-flow state and checked HIR, and covers ordered calls, field
paths, `match` bindings, and nested closures.

The compiler supplies the analyzed body and local-flow entry states, then
consumes the facts only to emit diagnostics. It no longer owns an
`checks/local/ownership.rs` traversal.

## Consequences

The local ownership dataflow model has one semantic owner. Compiler changes
cannot silently introduce a second moved-use interpretation, and backends can
reuse the same facts without importing compiler checks. This ADR does not
change source syntax, artifact format, provider contracts, or runtime
behavior.
