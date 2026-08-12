# ADR 0111: Semantic ownership of the local flow-state lattice

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics::LocalFlowState` owns the local ownership-state lattice:
locals, managed values, read views, resources, move paths, freshness state, and
value-type projections. Parameter seeding and move/retention transitions move
with the state. The compiler retains a private `BodyState` alias while its CFG
construction and lattice-merge mechanics are migrated in subsequent steps.

## Consequences

This is behavior-preserving and makes the language's flow-state contract
available to future semantic and MIR consumers. Public fields are temporary
migration exposure for legacy compiler joins; future work will narrow them
behind semantic lattice operations. No source, artifact, Provider, or runtime
contract changes.
