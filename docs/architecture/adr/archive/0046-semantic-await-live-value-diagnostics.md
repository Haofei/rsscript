# ADR 0046: Semantic ownership of await live-value diagnostics

## Status

Accepted

## Problem

Compiler body flow analysis already determines which resources and non-Copy
locals are live at an await boundary, but the compiler also constructed the
resulting `AWAIT_LIVE_LOCAL` diagnostics.

## Decision and non-goals

The compiler remains responsible for computing the flow state and liveness
facts. `rsscript-semantics::await_live_value_diagnostics` owns the diagnostic
contract over those facts. This makes the language-level suspension rule shared
without moving the still compiler-owned ownership dataflow engine prematurely.

This ADR does not migrate liveness analysis, resource cleanup, move/borrow
semantics, cancellation, or scheduler behavior.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data changes.
Existing diagnostic code, wording, cause, and fix are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantic tests cover resource and local facts; compiler and architecture
tests reject compiler-local reconstruction of the diagnostic.
