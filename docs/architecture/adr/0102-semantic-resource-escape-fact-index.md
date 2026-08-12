# ADR 0102: Semantic ownership of resource-escape fact indexing

## Status

Accepted.

## Decision

`rsscript-semantics` owns recursive HIR indexing of resource escape and capture
facts for each `with` statement. The index includes `manage`, retaining calls,
wrapper values, managed closure captures, and the intentional ownership
transfer of `TempDir.keep(take dir)`. Compiler local-flow only reads the facts.

## Compatibility and impact

The existing diagnostic facts and the `TempDir.keep` exception are preserved.
No source, Artifact, Provider, SDK, verifier, or persisted-data contract
changes.
