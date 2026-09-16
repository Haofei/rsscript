# ADR 0222: Dispatch asynchronous wire Providers without native values

- Status: Accepted
- Date: 2026-08-15

## Problem

The reference VM already dispatched no-`mut` synchronous
`WireInterpreterFn` calls directly. An `AsyncWireInterpreterFn`, however,
entered the scheduler through the legacy asynchronous dispatcher, which first
adapted arguments to `NativeValue` and adapted the completed result back. That
kept a dynamic compatibility representation on the Provider call path even
when both the linked descriptor and Provider implementation were canonical.

## Decision and non-goals

The register VM now uses a distinct scheduler wait state for a descriptor-
linked no-`mut` `AsyncWireInterpreterFn`. It converts register arguments to
the exact linked `WireCallTypeTable`, drives a `WireProviderFuture`, and
converts the completed wire result only at the explicit legacy register edge.
The direct asynchronous dispatcher applies the same cancellation, deadline,
payload-budget, trace, non-reentrancy, panic-containment, and runtime-resource
registration checks as the legacy dispatcher.

This does not change bytecode, Artifact, or Provider signature schemas. It
does not yet eliminate `NativeValue` from legacy Provider callables or from
`mut` mutation-envelope semantics; those compatibility paths remain explicit
until the register representation can encode mutations canonically.

## Compatibility and migration

The change is additive for Providers that already register an
`AsyncWireInterpreterFn`. Existing `AsyncInterpreterFn` providers retain the
legacy scheduler path. A raw wire callable still fails closed unless registry
linking has attached its descriptor and exact signature/type layouts.

## Verifier and security impact

No untrusted input gains a new execution path. The VM uses the descriptor that
passed Provider preflight to derive all aggregate, named-layout, and resource
identities; malformed or mismatched values fail before provider code executes.
This is not a sandbox boundary: Providers remain trusted host code.

## Provider and backend impact

New async Provider implementations can keep canonical values through the
actual VM call boundary. Other backends and experimental runtimes are
unchanged. Provider authors still use the reviewed `provider_api` module
rather than compatibility exports.

## Evidence

The MIR migration fixture compiles and executes an awaited external call
through both the legacy and direct MIR-to-bytecode paths using only an
`AsyncWireInterpreterFn`; the Provider asserts that it receives wire values.
VM contract tests continue to cover descriptor-scoped aggregates, named
variants, generation-safe resources, cancellation, and resource contracts.
