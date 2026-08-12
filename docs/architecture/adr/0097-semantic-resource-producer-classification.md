# ADR 0097: Semantic ownership of resource-producer classification

## Status

Accepted.

## Decision

`rsscript-semantics` owns the HIR query that classifies resource producers and
`Result<Resource, E>` producers, plus their context and missing-`?` diagnostics.
Compiler keeps only the transitional lexical HIR traversal.

## Compatibility and impact

Resource producer classification and all surrounding diagnostics keep their
previous behavior. No Artifact, Provider, SDK, verifier, or persisted-data
compatibility changes.
