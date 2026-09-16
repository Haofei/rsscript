# ADR 0092: Semantic ownership of declaration surface rules

## Status

Accepted.

## Decision

`rsscript-semantics` owns declaration-level source and token rules: removed
implementation/effect markers, malformed top-level declarations, generated
name reservation, generic protocol reservation, and bodyless source-function
rejection. Compiler only invokes the query, then continues its recursive
syntax adaptation.

## Compatibility and impact

Diagnostic code, span, label, cause, and fix are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests cover generated declaration names and bodyless source
functions. Architecture tests reject the former compiler rule helpers.
