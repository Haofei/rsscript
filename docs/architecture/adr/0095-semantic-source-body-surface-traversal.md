# ADR 0095: Semantic ownership of source body-surface traversal

## Status

Accepted.

## Decision

`rsscript-semantics` owns syntax-only statement and expression traversal,
including malformed forms, unsupported `spawn`, select-arm shape, and explicit
task-group context. Compiler no longer stores mutable task-group state or
constructs source-body language diagnostics; it only walks bodies to extract
alias-canonical type-reference facts for their separate semantic query.

## Compatibility and impact

Diagnostic code, span, label, cause, and fix are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests cover source-body `async let` and `spawn` rejection. Architecture
tests reject the former compiler source-body traversal helpers and state.
