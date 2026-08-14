# ADR 0176: Migrate the rooted filesystem Provider to WireValue

- Status: Accepted
- Date: 2026-08-14

## Problem

The official rooted filesystem Provider has a scalar interface (`String`
paths/text and `Unit`) but still exposed its callable through the legacy
`NativeValue` representation. This was unnecessary once the reference VM
supported linked scalar wire dispatch.

## Decision and non-goals

`RootedFsProvider` now implements `WireInterpreterFn`, accepting and returning
canonical `WireValue` scalars. Its interface descriptor, symbols, rooted-path
checks, byte-budget enforcement, cancellation checks, and I/O error mapping are
unchanged.

This does not turn filesystem paths into a language permission system, nor does
it cover structured filesystem APIs or resource handles.

## Compatibility and migration

Hosts may register the Provider through the existing generic registry without
API changes. Existing native providers remain supported. The VM dispatches this
wire Provider only after linked signature preflight; mismatched or structured
calls fail before Provider code executes.

## Security and architecture impact

The host-selected filesystem root remains an instance-owned authority. No
global current-directory behavior, ambient path access, or sandbox claim is
introduced by this ABI representation change.

## Evidence

The Provider conformance suite runs the wire cancellation/deadline gates.
Rooted-path and runtime byte-budget tests continue to pass, along with targeted
clippy for the filesystem Provider.
