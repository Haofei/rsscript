# ADR 0040: Semantic ownership of bare-return diagnostics

## Status

Accepted

## Problem

The compiler call checker owned diagnostics for `return` without a value in a
function declared to return a concrete non-`Unit` type.

## Decision and non-goals

`rsscript-semantics::missing_return_value_diagnostics` traverses resolved HIR
blocks and owns this control-flow rule. It preserves generic-return deferral and
the existing return mismatch diagnostic. Expression-return type compatibility
remains a separate migration.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data changes.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantic tests cover nested bare returns; compiler and semantics
regressions preserve orchestration.
