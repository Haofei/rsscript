# ADR 0068: Semantic ownership of call-place conflict diagnostics

## Status

Accepted.

## Problem

Compiler call checking resolves local place paths, effects, and alias facts, but
also owned the language diagnostics that explain conflicting place accesses.

## Decision and non-goals

`rsscript-semantics::place` owns the five canonical call-place conflict
diagnostics. Compiler retains path extraction, disjointness analysis, and
resolved local/managed facts, then delegates strings, causes, and spans to the
semantic constructors. The place fact model itself remains compiler-owned during
this migration stage.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic unit tests cover every conflict kind. Architecture tests require the
compiler delegation and reject the deleted compiler diagnostic constructors.
