# ADR 0082: Semantic ownership of match exhaustiveness diagnostics

## Status

Accepted.

## Problem

Compiler HIR traversal determines match coverage but also constructed the
language diagnostics for non-exhaustive match statements and expressions.

## Decision and non-goals

`rsscript-semantics::control_flow` owns the canonical diagnostic contract.
Compiler retains coverage fact extraction from its transitional HIR adapter.
This ADR does not yet move the coverage algorithm itself into semantics.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover statement and expression contracts. Architecture tests
require delegation and reject compiler diagnostic construction.
