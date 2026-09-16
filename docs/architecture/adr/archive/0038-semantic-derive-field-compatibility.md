# ADR 0038: Semantic ownership of derive-field compatibility diagnostics

## Status

Accepted

## Problem

The compiler previously interpreted whether fields support `Eq`, `Ord`, `Hash`,
`JsonEncode`, and `JsonDecode`. That structural type reasoning belongs with
language semantics rather than compiler orchestration or a Rust backend.

## Decision and non-goals

`rsscript-semantics::derive_field_diagnostics` owns the canonical derive-field
compatibility rule. It preserves the established handling for nested
containers, local generic value types, `handle`/`weak` fields, and type labels.
The compiler appends those diagnostics but does not reinterpret field support.

This does not add derives, change lowering, or make derives a Provider or
review-policy mechanism.

## Compatibility and migration

Accepted source programs and diagnostic codes remain unchanged. Artifact,
Provider, SDK, runtime, and persisted-data contracts are unaffected.

## Verifier and security impact

No untrusted-input, resource, cancellation, or isolation boundary changes.

## Provider and backend impact

Providers and execution backends consume the same already-validated source
semantics; no ABI or conformance change is required.

## Evidence

Focused semantic tests cover unsupported nested field types. Compiler and
semantics tests retain existing diagnostic fixtures, and the SDK architecture
test rejects a compiler-owned derive-support implementation.
