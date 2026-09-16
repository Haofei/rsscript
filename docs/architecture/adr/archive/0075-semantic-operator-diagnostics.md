# ADR 0075: Semantic ownership of builtin operator diagnostics

## Status

Accepted.

## Problem

The compiler traverses resolved HIR to extract operand types, but it also
constructed diagnostics for unsupported arithmetic overload attempts and
incompatible builtin operator operands.

## Decision and non-goals

`rsscript-semantics::operators` owns the canonical diagnostics for these
resolved facts. Compiler keeps HIR traversal, alias expansion, and operand-type
fact extraction. This ADR does not move numeric-type inference itself.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover both contracts. Architecture tests require compiler
delegation and reject the former compiler-owned text.
