# ADR 0071: Semantic ownership of callback contract diagnostics

## Status

Accepted.

## Problem

Compiler callback checking resolves parameter counts and types, returns,
freshness, operators, and retention facts, while also owning diagnostic wording
for every resulting contract mismatch.

## Decision and non-goals

`rsscript-semantics::callbacks` owns the canonical diagnostics for those
resolved callback facts. Compiler keeps the callback HIR traversal, type-pattern
matching, and fact extraction. Callback escape diagnostics remain a separate
migration unit because they carry context-specific escape facts.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover every callback mismatch kind. Architecture tests
require compiler delegation and reject former callback diagnostic text.
