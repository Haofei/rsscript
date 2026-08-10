# ADR 0039: Semantic ownership of function-fallthrough diagnostics

## Status

Accepted

## Problem

The compiler call-check module previously decided whether a checked function
body could reach its end without returning its declared non-`Unit` result.
This is resolved HIR control-flow semantics, not compiler orchestration.

## Decision and non-goals

`rsscript-semantics::function_fallthrough_diagnostics` owns the canonical
diagnostic. It preserves generic-return deferral and the existing diagnostic
code, wording, cause, and fix. The compiler appends the semantic result before
performing remaining call checks.

This ADR does not migrate return-expression typing, task/cancellation flow, or
backend lowering.

## Compatibility and migration

No accepted source, Artifact, Provider, SDK, runtime, or persisted-data
contract changes. Existing non-`Unit` fallthrough diagnostics are preserved.

## Verifier and security impact

No change to untrusted-bytecode verification, execution budgets, resources,
cancellation, or isolation boundaries.

## Provider and backend impact

Providers and backends receive the same already-validated language contract.

## Evidence

A focused semantic test covers non-`Unit` fallthrough; compiler/semantics
regressions cover orchestration; an SDK architecture test rejects restoring the
compiler-local control-flow helpers.
