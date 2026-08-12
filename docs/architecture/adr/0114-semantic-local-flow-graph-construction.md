# ADR 0114: Semantic ownership of local flow-graph construction

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics` lowers checked HIR to the local ownership-flow graph. The
lowering owns graph reachability, structured branch and loop edges, `with`
cleanup edges, fresh match/select bindings, and retained closure capture facts.

## Consequences

The compiler local-flow module is now a small compatibility adapter for legacy
body-check callers and tests. It no longer implements HIR traversal, graph
construction, or ownership fixed-point logic. No source, artifact, Provider,
or runtime contract changes.
