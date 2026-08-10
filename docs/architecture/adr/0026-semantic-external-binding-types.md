# ADR 0026: Semantic validation of external binding types

## Status

Accepted

## Problem

Validation of the platform-neutral `Dyn<Protocol>` type spelling was embedded
in compiler analyzer state, even though its shape, generic-parameter exception,
and protocol visibility result are frontend semantic facts.

## Decision and non-goals

`rsscript-semantics::external_binding_type_diagnostics` owns `Dyn<Protocol>`
shape and visibility diagnostics. Compiler supplies the composed visible
protocol set, then aggregates the returned diagnostics. General unknown type
names and generic constraints remain separate migration work.

## Compatibility and migration

Existing diagnostic behavior is retained; no Artifact, Provider, SDK, or
runtime ABI changes occur.

## Verifier and security impact

None; validation remains pre-lowering.

## Provider and backend impact

None.

## Evidence

Semantics tests cover missing and visible protocols. Compiler regression tests
remain green, and an architecture test requires use of the semantic query.
