# ADR 0011: MIR structured-task primitives

## Status

Accepted

## Problem

Structured concurrency was a source-level semantic claim, but typed MIR had no
task identity, lexical group ownership, or verifier-visible close operation.
A backend could therefore erase an unjoined child task without a construction
error.

## Decision and non-goals

`rsscript-mir` now owns typed `TaskId` and `TaskGroupId` identities and models
`Spawn`, `Await`, `Cancel`, and `Join`. A spawn names a resolved asynchronous
internal function and its checked argument modes. Verification rejects duplicate
task IDs, invalid/non-async spawn targets, operations on non-live tasks, group
identity conflicts at CFG joins, and any reachable return with a live task.

This is the lifecycle/verifier primitive. It does not yet lower source async
syntax, model `select`, or define VM scheduling and cancellation delivery.
Those paths remain explicitly unsupported rather than acquiring accidental
runtime semantics.

## Compatibility and migration

The new identities and instructions are internal pre-1.0 MIR additions; no
Artifact, Provider, or SDK wire contract changes. Existing scalar MIR is
unchanged. Backends must implement the instructions or reject them.

## Verifier and security impact

The verifier makes lexical child-task ownership a pre-execution invariant on
normal control-flow returns. It does not claim process isolation and does not
yet prove error or cancellation cleanup paths.

## Provider and backend impact

The conformance interpreter and VM code generator reject structured
concurrency operations until scheduling is implemented. Providers are not
involved in this initial internal-task-only contract.

## Evidence

MIR unit tests cover an awaited child and a leaked child. MIR conformance,
codegen, and lowering tests demonstrate the explicit fail-closed boundary.
