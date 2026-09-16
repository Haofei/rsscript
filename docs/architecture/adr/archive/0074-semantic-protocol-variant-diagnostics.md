# ADR 0074: Semantic ownership of resolved protocol and variant diagnostics

## Status

Accepted.

## Problem

Generic call checking resolves protocol satisfaction, dynamic protocol
construction, and sum-variant field facts, but the compiler also constructed
the language diagnostics for those facts.

## Decision and non-goals

`rsscript-semantics::generic_constraints` owns the canonical diagnostics for
generic protocol-bound failures, `Dyn<Protocol>` construction, protocol
receiver failures, and sum-variant field shape/type failures. Compiler keeps
symbol resolution, substitution, protocol-implementation lookup, and literal
type matching. This ADR does not yet move those resolution algorithms.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover every diagnostic family. Architecture tests require
compiler delegation and reject the former diagnostic text.
