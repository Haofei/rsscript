# ADR 0172: Admit linked scalar WireValue Provider dispatch

- Status: Accepted
- Date: 2026-08-14

## Problem

The Provider API exposed `WireInterpreterFn`, but the reference VM registry
only dispatched `NativeValue` callables. New Provider authors therefore could
not use the canonical wire model on the actual execution path.

## Decision and non-goals

`ProviderCallable` now admits a synchronous `WireSync` variant. The VM
dispatches it only after Provider linking and converts the exact linked
signature's scalar values (`Unit`, `Bool`, integer, float, `String`, `Bytes`)
between the legacy VM representation and `WireValue`.

Records, variants, resources, lists, maps, JSON, and chars fail closed in this
adapter. They require the upcoming linked Artifact type-table adapter; this
change must not fabricate numeric type IDs, field order, resource handles, or
string identities merely to widen support.

Async wire callables are also out of scope for this slice.

## Compatibility and migration

Existing `NativeInterpreterFn` and `AsyncInterpreterFn` providers keep their
behavior. `WireSync` is additive and requires an already linked descriptor;
constructing a raw `ExternalFunction` without that descriptor cannot execute a
wire callable. No Artifact, language, or Provider signature format changes.

## Verifier and security impact

The conversion uses the descriptor resolved during Provider preflight, so a
call cannot choose types dynamically. Unsupported structured values fail before
Provider code runs. Cancellation, deadline, payload budget, tracing,
non-reentrancy, and resource-registration checks remain in the same VM
dispatch path.

## Provider and backend impact

New scalar-only synchronous Provider implementations can use
`WireInterpreterFn`. Generated and official structured Providers remain on the
compatibility adapter until the type-table bridge is implemented. VM bytecode
and other backends are unchanged.

## Evidence

Provider API tests retain the wire callable cancellation gate. VM contract
tests register a linked scalar `WireInterpreterFn`, prove it receives
`WireValue::Int`, and verify the result returns through the execution boundary
as `NativeValue::Int`. Clippy runs on both Provider API and VM crates.
