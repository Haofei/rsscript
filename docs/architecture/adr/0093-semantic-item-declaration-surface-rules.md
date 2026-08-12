# ADR 0093: Semantic ownership of item declaration surface rules

## Status

Accepted.

## Decision

`rsscript-semantics` owns item-local declaration diagnostics that require only
source syntax: malformed generic/parameter/field fragments, opaque and
managed-drop restrictions, and literal-only const initialization. Compiler
continues to own the transitional alias-aware callback placement adapter until
that resolved type fact has moved as a separate semantic query.

## Compatibility and impact

Diagnostic code, span, label, cause, and fix are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests cover literal-const rejection. Architecture tests require the
semantic query at the compiler call site.
