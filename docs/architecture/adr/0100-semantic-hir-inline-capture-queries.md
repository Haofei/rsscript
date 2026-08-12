# ADR 0100: Semantic ownership of HIR inline-capture queries

## Status

Accepted.

## Decision

`rsscript-semantics` owns the handle-aware HIR traversal that derives inline
closure capture uses. It traverses nested control-flow bodies and deliberately
does not treat handle-field projections as captures. Compiler local-flow and
resource passes consume the resulting neutral facts.

## Compatibility and impact

The query preserves the current managed/retained closure behavior. It changes
no source, Artifact, Provider, SDK, verifier, or persisted-data contract.
