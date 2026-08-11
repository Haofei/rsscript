# ADR 0044: Semantic ownership of await-operand diagnostics

## Status

Accepted

## Problem

The compiler body checker owned the rule that an `await` operand must be a
resolved async call or a not-yet-consumed async-let binding from the surrounding
structured task group. That rule operates only on checked HIR and the group's
pending-binding state.

## Decision and non-goals

`rsscript-semantics::await_operand_diagnostic` owns the async-call detection,
async-let unwrapping, consumption, and `AWAIT_NON_ASYNC` diagnostic. Compiler
body traversal supplies the mutable pending-binding list and appends an optional
diagnostic.

This ADR does not migrate await placement, liveness across suspension, resource
cleanup, cancellation propagation, or runtime scheduling.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data contract
changes. Existing diagnostic code, wording, cause, and fix are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantic tests cover exactly-once async-let consumption. Compiler and
architecture tests verify semantic invocation and reject compiler-local copies.
