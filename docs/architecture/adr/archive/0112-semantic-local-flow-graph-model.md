# ADR 0112: Semantic ownership of the local flow-graph model

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics` owns the neutral graph data model used by local ownership
analysis: step roles, value bindings, resource bindings, effect events, and
cleanup-carrying successor edges. The model is derived exclusively from checked
HIR and has no compiler, workspace, Provider, or runtime dependency.

## Consequences

The compiler temporarily constructs this semantic model and performs its
fixed-point traversal. Subsequent migrations will move graph construction and
lattice solving without changing the meaning of a local-flow step. No source,
artifact, Provider, or runtime contract changes.
