# ADR 0043: Semantic ownership of await-placement diagnostics

## Status

Accepted

## Problem

The compiler body checker recursively determined whether each `await` expression
occurred in an async function or a structured task-group/select boundary. This
is a checked-HIR control-flow rule with no compiler, provider, or backend input.

## Decision and non-goals

`rsscript-semantics::await_placement_diagnostics` owns the HIR traversal and
the `AWAIT_OUTSIDE_ASYNC` diagnostic. It preserves closure reset, task-group,
select-operation, and assignment-target semantics. The compiler only appends
the semantic diagnostics for each checked function body.

This ADR does not migrate await operand validation, async-call consumption,
resource/local liveness across suspension, cancellation, or runtime scheduling.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data contract
changes. Existing diagnostic code, wording, cause, and fix are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantic tests cover synchronous and async HIR contexts; compiler and
architecture tests ensure the compiler calls the semantic query and no longer
retains the placement traversal.
