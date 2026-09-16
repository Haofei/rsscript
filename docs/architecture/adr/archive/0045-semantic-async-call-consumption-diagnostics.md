# ADR 0045: Semantic ownership of async-call consumption diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::async_call_consumption_diagnostic` owns the diagnostic for
a resolved async call without an explicit consumption boundary. Compiler passes
the resolved async bit, source display name, span, and whether the surrounding
expression is `await` or `spawn`. Await liveness, cancellation, and scheduling
remain separate rules.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data changes.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantic tests and compiler/architecture regression tests preserve the
existing `ASYNC_CALL_NOT_CONSUMED` diagnostic and prohibit a compiler-local copy.
