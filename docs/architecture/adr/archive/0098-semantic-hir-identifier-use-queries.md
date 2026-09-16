# ADR 0098: Semantic ownership of HIR identifier-use queries

## Status

Accepted.

## Decision

`rsscript-semantics` owns the backend-neutral HIR queries for identifier uses
in a statement or block. They retain source order and assignment-target reads.
The compiler's local CFG, resource, and ownership passes consume the query and
derive only their flow-specific facts.

## Compatibility and impact

This preserves existing identifier-use behavior while removing duplicate HIR
traversal from the compiler. It does not change language syntax, Artifact,
Provider, SDK, verifier, or persisted-data contracts.
