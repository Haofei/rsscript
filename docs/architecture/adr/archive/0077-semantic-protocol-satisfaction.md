# ADR 0077: Semantic ownership of protocol satisfaction

## Status

Accepted.

## Problem

The compiler used to decide protocol satisfaction for generic calls and
receivers, including builtin scalar, structural-container, generic-bound,
explicit implementation, and derive behavior.

## Decision and non-goals

`rsscript-semantics::generic_constraints` owns `ProtocolSatisfactionFacts` and
the satisfaction rule. Compiler derives the resolved actual type and passes the
visible source implementation inventory. This ADR does not move call
resolution, generic substitution, or alias expansion.

## Compatibility and migration

The rule preserves the existing builtin and derived protocol behavior. No
Artifact, Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic unit tests cover builtin, derived, and structural container cases.
Architecture tests require compiler delegation and reject the former compiler
rule helpers.
