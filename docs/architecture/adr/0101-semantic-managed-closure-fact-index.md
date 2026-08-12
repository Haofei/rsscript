# ADR 0101: Semantic ownership of managed-closure fact indexing

## Status

Accepted.

## Decision

`rsscript-semantics` owns the HIR traversal that indexes managed closure
capture uses by binding-statement span, including nested closure discovery.
The compiler local-flow pass only retrieves those neutral facts by span.

## Compatibility and impact

This preserves local capture behavior and diagnostic locations. It changes no
source, Artifact, Provider, SDK, verifier, or persisted-data contract.
