# ADR 0088: Semantic ownership of async lowering-shape traversal

## Status

Accepted.

## Decision

`rsscript-semantics::await_placement` owns the complete async-function source
query that distinguishes direct `await` boundaries from nonlinear awaits,
including assignment-target evaluation. Compiler only invokes it for source
functions.

## Compatibility and impact

Diagnostic code, span, cause, and fix are unchanged. There are no Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests cover direct and nested awaits. Architecture tests reject the
former compiler traversal helpers.
