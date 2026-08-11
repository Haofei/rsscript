# ADR 0042: Semantic ownership of receiver-call effect diagnostics

## Status

Accepted

## Problem

The compiler generic-call checker owned the implicit `self` effect diagnostics
for receiver-call shorthand. The rule depends only on a resolved receiver label,
the supplied effect, and the first resolved parameter's effect.

## Decision and non-goals

`rsscript-semantics::receiver_call_effect_diagnostics` owns missing receiver
parameter, missing receiver effect, and mismatched receiver effect diagnostics.
The compiler resolves the source receiver and method signature, then supplies a
syntax-independent fact. This preserves the distinction between an absent
receiver parameter and a parameter with no data effect.

This ADR does not migrate receiver method resolution, protocol satisfaction,
argument type compatibility, generic substitution, retention, or escape rules.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data contract
changes. Existing diagnostic codes, wording, causes, and fixes are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Focused semantic tests cover mismatched and missing receiver facts; compiler
and architecture tests verify the compiler invokes the semantic query and no
longer retains the local implementation.
