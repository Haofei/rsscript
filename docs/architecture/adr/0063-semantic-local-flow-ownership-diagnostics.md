# ADR 0063: Semantic ownership of LocalAnalysis ownership diagnostics

## Status

Accepted.

## Problem

`LocalAnalysis` correctly derives local flow facts, but compiler body checks
also owned the diagnostics for moved uses, managed-to-local binding, retention
of locals/captures, and consuming handle fields. Those diagnostics are language
semantics rather than flow extraction.

## Decision and non-goals

`rsscript-semantics` owns five diagnostic constructors: `moved_use_diagnostic`,
`managed_to_local_diagnostic`, `retained_local_diagnostic`,
`retained_closure_capture_diagnostic`, and `take_handle_field_diagnostic`.
Compiler continues to derive LocalAnalysis facts and passes their structured
fields to semantics. Fresh-return, resource-escape, and closure-contract rules
remain separate migration work.

## Compatibility and migration

Codes, spans, titles, causes, and fixes are preserved. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover every fact-derived diagnostic. Architecture tests
forbid the old compiler diagnostic constructors.
