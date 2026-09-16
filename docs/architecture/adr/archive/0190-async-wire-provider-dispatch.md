# ADR 0190: Admit linked asynchronous wire Provider dispatch

- Status: Accepted
- Date: 2026-08-14

## Problem

The canonical Provider boundary supported `WireInterpreterFn` only for
synchronous calls. Asynchronous Providers therefore had to expose
`NativeValue`, even when their descriptor signature used only values that the
linked wire adapter can represent safely.

## Decision and non-goals

Add `AsyncWireInterpreterFn`, `WireProviderFuture`, and `ProviderCallable::WireAsync`.
The reference VM derives the same descriptor-scoped type table used by
synchronous wire calls, converts native VM arguments before beginning the
future, contains both call-construction and polling panics, converts the
completed value back through the linked signature, and preserves the existing
cancellation, deadline, resource, payload, trace, blocking-lane, and
non-reentrancy checks.

This does not add an async ABI transport or Artifact-wide structural layouts.
Wire async calls support only the currently safe scalar and descriptor-scoped
aggregate forms (`List`, tuple, `Option`, `Result`). Named records, arbitrary
variants, resources, maps, JSON, and chars remain fail-closed.

## Compatibility and migration

The API is additive. Existing `AsyncInterpreterFn` implementations retain
their behavior. The reviewed SDK now exposes the canonical asynchronous wire
types beside the synchronous wire types; legacy native types remain
compatibility-only.

## Evidence

Provider API tests retain the existing cancellation gate. VM contract tests
link an async wire function, prove it receives a canonical integer wire value,
and prove its result crosses the async dispatcher as the expected legacy VM
value. Clippy runs on the Provider API and VM crates.
