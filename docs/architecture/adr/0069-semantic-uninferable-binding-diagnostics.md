# ADR 0069: Semantic ownership of uninferable binding diagnostics

## Status

Accepted.

## Problem

Compiler body traversal finds unused bindings initialized with open
`Ok`/`Err`/`None` constructors, but also owned the language diagnostic that
requires the otherwise ambiguous type to be constrained.

## Decision and non-goals

`rsscript-semantics` owns the canonical uninferable-binding diagnostic and fix.
Compiler retains raw reference collection and checked-HIR recognition of open
variant constructors. More general type inference remains outside this focused
ownership migration.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover the constructor. Architecture tests require compiler
delegation and reject its former diagnostic text.
