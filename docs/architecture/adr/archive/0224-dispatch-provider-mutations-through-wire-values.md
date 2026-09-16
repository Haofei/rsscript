# ADR 0224: Dispatch Provider mutations through canonical wire values

- Status: Accepted
- Date: 2026-08-15

## Problem

The canonical Provider ABI already carried scalar, aggregate, record, variant,
and resource values as `WireValue`, but a function with a `mut` parameter still
had to use a legacy `NativeValue` list envelope to return its result and
write-backs. That kept a dynamic representation on an ordinary Core execution
path and left generated mocks unable to model a descriptor-declared mutation.

## Decision

`rsscript-provider-api` exposes `WireMutationInterpreterFn` and
`AsyncWireMutationInterpreterFn`. They receive canonical wire arguments and
return `WireMutationResult { result, mutated }`, where `mutated` is ordered by
the descriptor's `mut` parameters. The register VM validates the exact count
and each descriptor-scoped wire type before converting only at its explicitly
named legacy register compatibility edge.

Bindgen chooses the corresponding sync or async mutation callable for generated
mocks whenever an interface signature contains `mut`. New Provider-facing APIs
therefore never need a `NativeValue` mutation envelope.

## Non-goals and compatibility

This does not remove the legacy VM register representation or the explicit
`compatibility` Native callable adapter. Those remain for legacy callers only.
It does not change Provider signatures, artifact schemas, or scheduling gates.
Replay remains limited to non-mutation canonical callables until it can model
write-backs without weakening its strict tape contract.

## Evidence

Direct checked-HIR-to-MIR bytecode tests execute both synchronous and
asynchronous `mut` external calls through canonical wire callables and verify
the write-back. Bindgen tests assert that generated mutation mocks select the
matching wire callable variants.
