# ADR 0072: Semantic ownership of closure escape diagnostics

## Status

Accepted.

## Problem

Compiler escape traversal identified `noescape` and local closure uses in store,
return, ordinary-value, and forwarding contexts, but also owned the escape
context types and diagnostic text.

## Decision and non-goals

`rsscript-semantics::closure_escape` owns the neutral `ClosureEscapeContext`
and the noescape/local closure escape diagnostics. Compiler aliases the context
at its compatibility boundary, continues HIR traversal, and delegates resolved
name, span, and context facts. This does not alter which contexts count as an
escape.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover both closure categories. Architecture tests require the
semantic context/constructors and reject compiler-owned escape wording.
