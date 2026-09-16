# ADR 0049: Semantic ownership of try error-type compatibility

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::try_error_type_diagnostics` owns recursive checked-HIR
traversal for `?` and the requirement that a `Result` operand's error type exactly
matches the enclosing function's `Result` error type. Compiler provides the
already-resolved enclosing error type and appends the result.

This ADR does not add implicit error conversion, change `Option` semantics, or
migrate runtime error handling.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data changes.
Existing diagnostic code, wording, causes, and fix are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantics tests cover mismatched `Result` errors; compiler and
architecture tests reject compiler-local traversal and diagnostic construction.
