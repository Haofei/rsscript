# ADR 0062: Semantic ownership of declared pattern field diagnostics

## Status

Accepted.

## Problem

Compiler pattern checking owned the diagnostics for an unknown structured field
and for omitting declared fields without `..`. The declaration lookup is an
adapter fact, but the constraints and diagnostics are language semantics.

## Decision and non-goals

`rsscript-semantics` owns `unknown_pattern_field_diagnostic` and
`omitted_pattern_fields_diagnostic`. Compiler retains declared-field lookup and
chooses the existing diagnostic span. Nested field type recursion remains
separate migration work.

## Compatibility and migration

All diagnostic code, span selection, title, message, cause, and fix contracts
are retained. No Artifact, Provider, SDK, or persisted-data changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Focused semantic tests cover both diagnostics; architecture tests reject their
reintroduction in compiler body checking.
