# ADR 0047: Semantic ownership of weak-field upgrade diagnostics

## Status

Accepted

## Problem

The compiler body checker recursively traversed checked HIR to reject weak
handle fields used without the explicit `Weak.upgrade` boundary. The rule has no
dataflow, Provider, runtime, or backend dependency.

## Decision and non-goals

`rsscript-semantics` now owns weak-upgrade call recognition, recursive HIR
traversal, and the `WEAK_FIELD_REQUIRES_UPGRADE` diagnostic. Compiler uses the
semantic upgrade predicate only to preserve its surrounding effect-context
orchestration.

This ADR does not migrate weak-resource lifecycle, ownership dataflow, Provider
resource handling, or backend lowering.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data changes.
Existing diagnostic code, wording, cause, and fix are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantics tests cover weak-field diagnostics and the explicit upgrade
identity. Compiler and architecture tests prevent local traversal from returning.
