# ADR 0099: Semantic ownership of HIR effect-event queries

## Status

Accepted.

## Decision

`rsscript-semantics` owns canonical HIR place-path resolution and effect-event
extraction for a statement. This includes explicit call effects, `manage`, and
synthetic `match take` events. The compiler CFG consumes those facts without
reimplementing HIR expression traversal.

## Compatibility and impact

The query preserves current event ordering and local-flow behavior. No source,
Artifact, Provider, SDK, verifier, or persisted-data contract changes.
