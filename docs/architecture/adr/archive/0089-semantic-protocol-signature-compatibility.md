# ADR 0089: Semantic ownership of protocol implementation signature compatibility

## Status

Accepted.

## Decision

`rsscript-semantics` owns the resolved protocol-to-implementation signature
comparison, including `Self` substitution, effects, return types, freshness,
and retention. Compiler only resolves the two declared functions and reports
the semantic mismatch result.

## Compatibility and impact

Mismatch reasons and their surrounding diagnostics are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.
