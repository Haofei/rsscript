# ADR 0065: Semantic ownership of `fresh` return diagnostics

## Status

Accepted.

## Problem

The compiler's local-flow analysis correctly determines whether a `fresh`
function return is clean, unknown, or invalid by declared type, but compiler
body checks also constructed the corresponding language diagnostics.

## Decision and non-goals

`rsscript-semantics` owns diagnostics for non-fresh returns, unknown freshness,
and invalid non-struct `fresh` targets. Compiler preserves LocalAnalysis fact
extraction, trusted-fresh resolution, and resolved type adaptation. Constructor
field, `fresh` expression, and closure contracts remain separate migration work.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover the three constructors; architecture tests forbid
their former compiler constructors and require delegation.
