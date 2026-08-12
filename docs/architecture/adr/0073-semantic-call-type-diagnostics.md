# ADR 0073: Semantic ownership of resolved call type diagnostics

## Status

Accepted.

## Problem

Compiler call checking resolves actual/expected types, literals, callee lookup,
and channel substitutions, but previously also owned diagnostic construction for
the resulting compatibility facts.

## Decision and non-goals

`rsscript-semantics::type_compatibility` owns the nine canonical diagnostics for
resolved binding, argument, container literal, callee resolution, and message
payload facts. Compiler retains matching, alias expansion, generic substitution,
and literal traversal. This ADR does not move compatibility algorithms yet.

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
