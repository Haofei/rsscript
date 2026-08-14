# ADR 0188: Derive aggregate wire identities from linked signatures

- Status: Accepted
- Date: 2026-08-14

## Problem

ADR 0172 admitted scalar `WireValue` Provider calls, but correctly rejected
aggregate values because the legacy VM adapter had no trustworthy source for
their numeric type and variant identities.  Keeping lists and options on
`NativeValue` indefinitely would make the canonical Provider path unusable for
two common interface shapes.

## Decision and non-goals

`rsscript-abi-model` now derives a deterministic `WireCallTypeTable` from one
validated `FunctionSignature`: parameter types are visited in declaration
order, then the result type, and children are interned before their container.
The VM adapter and a `WireInterpreterFn` Provider derive the same table from
the descriptor that Provider linking has already validated.

This admits canonical `List<T>` and `Option<T>` values on synchronous linked
Provider calls. Lists carry the descriptor-scoped element type identity;
options carry the enclosing option type identity plus stable `Some`/`None`
variant ordinals. The adapter validates every identity before converting to or
from the legacy VM representation.

This is deliberately not an Artifact-wide type table. Named records, arbitrary
variants, tuples, results, resources, maps, JSON, chars, and asynchronous wire
calls remain fail-closed until their complete Artifact layout and lifecycle
contracts are available. A type ID from one function descriptor is never valid
for another descriptor.

## Compatibility and migration

The helper is additive. Existing `NativeInterpreterFn` Providers remain
supported behind the compatibility boundary. The official CLI and environment
Providers now use `WireInterpreterFn`, proving list and option migration while
their interface signatures remain unchanged. No bytecode, Artifact, or
Provider signature format changes.

## Verifier and security impact

The type table is formed only after descriptor preflight and from the exact
signature used to call the Provider. The VM rejects mismatched list element
identities, option type identities, option variant identities, and malformed
legacy option layouts before or after Provider code runs. This does not make
the in-process VM an isolation boundary; cancellation, deadline, payload,
trace, and non-reentrancy checks remain on the existing dispatch path.

## Evidence

ABI tests prove deterministic child-first identity assignment. VM tests round
trip list and option values and retain fail-closed named-record behavior. CLI
and environment Provider conformance tests now execute wire callables directly.
