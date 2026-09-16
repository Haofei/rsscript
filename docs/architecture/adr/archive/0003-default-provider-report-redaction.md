# ADR 0003: Default Provider failure reports are redacted

## Status

Accepted

## Problem

Provider error messages and `details` are supplied by host implementations.
They can contain request paths, endpoints, credentials, or response fragments.
Execution reports are designed to cross process and service boundaries, so
serializing those fields by default turns a diagnostic convenience into an
unbounded data-exposure channel.

## Decision and non-goals

The reviewed SDK converts an `EvalError::Provider` into a report-safe
`RuntimeError` containing only its stable Provider error code, rendered as
`provider call failed (<code>)`. Provider messages and `details` never enter
the default `ExecutionReport`; default trace policy also omits individual
Provider call traces. Aggregated telemetry retains provider identity, call
counts, byte accounting, duration, and failure counts.

This does not remove host diagnostics. A host that needs richer information
keeps it in a host-owned, explicitly redacted trace sink rather than relying on
the portable execution-report schema. This decision does not make an
in-process VM an isolation boundary.

## Compatibility and migration

Consumers must treat `RuntimeError.message` for Provider failures as a stable
redacted summary rather than a forwarding channel for Provider text. The
termination reason and Provider error category remain available to machine
consumers. Providers continue to receive and return structured errors at their
own boundary.

## Evidence

The SDK execution suite registers a Provider that returns a secret-bearing
message and JSON details, then proves the default serialized report contains
neither value while retaining the provider failure category and aggregate
telemetry.
