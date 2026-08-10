# ADR 0023: Semantic ownership of cyclic type-alias diagnostics

## Status

Accepted

## Problem

Cycle detection for type aliases was implemented inside compiler type checks
despite requiring only parsed interfaces, the source program, and diagnostics.
This made a platform-neutral type-graph rule unavailable to other frontend
consumers and left the compiler responsible for semantic interpretation.

## Decision and non-goals

`rsscript-semantics::cyclic_type_alias_diagnostics` owns alias dependency graph
construction and cycle diagnostics. It accepts immutable interface programs and
the immutable source program, respects generic parameter shadowing, and returns
the existing structured diagnostic facts. The compiler only incorporates that
result into its aggregate diagnostics.

Unknown type names, field validation, generic constraints, and backend lowering
remain separate migration work.

## Compatibility and migration

The diagnostic code, source span, cause, and fix are unchanged. This internal
pre-1.0 refactor does not alter language syntax, Artifact encoding, Provider
ABI, or SDK API.

## Verifier and security impact

None. This runs before lowering and does not change untrusted Artifact
validation or runtime limits.

## Provider and backend impact

None. Backends continue to receive only validated semantic results.

## Evidence

Semantics tests cover a multi-alias cycle and generic parameter shadowing. The
existing compiler regression test covers interface-originated cycles. An SDK
architecture test requires semantic ownership and rejects compiler-local cycle
graph helpers.
