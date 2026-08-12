# ADR 0090: Semantic ownership of protocol declaration rules

## Status

Accepted.

## Decision

`rsscript-semantics` owns source-level protocol declaration rules: the
qualified protocol-method index, the bodyless protocol-contract requirement,
and the restriction that `= _` is only valid on a protocol method. Compiler
only supplies the visible source/interface items and appends the semantic
diagnostics.

## Compatibility and impact

Diagnostic code, span, label, cause, and fix are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests cover rejected protocol bodies, free default markers, and
protocol method indexing. Architecture tests reject the former compiler rules.
