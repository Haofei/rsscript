# ADR 0108: Semantic ownership of local-binding HIR facts

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics::local_binding_value_facts` owns the initializer projections
needed by ownership flow: source identity, handle-field source, and fresh-value
classification. The compiler converts the single result into its private CFG
binding node and retains ownership only of state transfer.

## Consequences

This removes four duplicate HIR rule helpers from compiler flow construction
without changing language, artifact, Provider, or runtime contracts. The
architecture test forbids their reintroduction.
