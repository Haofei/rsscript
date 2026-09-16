# ADR 0128: Add a canonical wire Provider callable

- Status: Accepted
- Date: 2026-08-14

## Problem

The ABI model already defines structural `WireValue`, but every Provider
callable exposed by `rsscript-provider-api` accepted legacy `NativeValue`.
That prevented new Provider adapters from adopting the canonical type model
without also depending on legacy strings, JSON, and native IDs.

## Decision and non-goals

`WireInterpreterFn` and `WireHostFn` provide a parallel callable boundary over
`WireValue`, sharing the existing provider call context and cancellation gate.
`NativeInterpreterFn` remains the VM compatibility adapter until registry and
dispatch migration can be made atomically. This ADR does not claim that the
stable Provider API has completed the NativeValue removal.

## Compatibility and migration

The change is additive. New bindgen output and Providers can target the wire
callable; existing Providers and artifacts continue to use NativeValue.

## Verifier and security impact

Wire calls cannot bypass cancellation/deadline context. Structural value IDs
remain defined by the interface/ABI contract rather than user-supplied names.

## Provider and backend impact

No VM behavior changes yet. The subsequent registry migration will adapt
verified interface signatures to wire callables and retain legacy support only
behind an explicit compatibility layer.

## Evidence

Provider API tests prove a cancelled wire call cannot enter Provider code; the
existing resource wire-handle and payload-accounting tests remain green.
