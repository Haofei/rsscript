# ADR 0081: Semantic ownership of assignment diagnostics

## Status

Accepted.

## Problem

The compiler assignment traversal owns lexical scopes and place facts, but also
constructed language diagnostics for invalid assignment, mutability, type
compatibility, and indexed target support.

## Decision and non-goals

`rsscript-semantics::assignment` owns those diagnostic contracts. Compiler
retains scope construction, type inference, mutability/place fact extraction,
and uses the shared semantic type rules. This ADR does not yet move the full
assignment scope traversal into semantics.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover every diagnostic family. Architecture tests require
delegation and reject the former compiler diagnostic text.
