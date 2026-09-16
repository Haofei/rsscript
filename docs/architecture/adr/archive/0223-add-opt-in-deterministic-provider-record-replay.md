# ADR 0223: Add opt-in deterministic Provider record/replay

- Status: Accepted
- Date: 2026-08-15

## Problem

RSScript can already identify an external symbol and exact semantic signature,
but host integration tests had no uniform way to prove that a deterministic
Provider call sequence could be replayed without invoking the host operation.
An unconstrained recording feature would risk persisting sensitive arguments,
reintroducing dynamic values, or implying that replay establishes security.

## Decision and non-goals

`rsscript-provider-api` provides explicit wrappers for synchronous and
asynchronous canonical wire callables. A wrapper records or replays the
external symbol, signature hash, exact `WireValue` arguments, and structured
result. Replay fails closed on an exhausted tape, symbol/signature mismatch, or
argument mismatch and never calls the wrapped Provider as a fallback.

The contract requires deterministic replayability, canonical wire-value
normalization, no redaction, no declared external state, and in-memory-only
retention. Tapes intentionally do not implement serialization. Hosts needing a
persistent, redacted, or environment-aware format own that separately.

This is not a trace format, authorization policy, source-provenance proof, or
security boundary. It does not make a non-deterministic Provider deterministic.

## Compatibility and migration

The wrappers are additive and operate only on `WireInterpreterFn` and
`AsyncWireInterpreterFn`. Existing native compatibility callables and existing
Provider descriptors continue unchanged. No Artifact, bytecode, or Provider
ABI schema changes.

## Verifier and security impact

The wrapper cannot bypass normal Provider registration or runtime
cancellation/deadline/resource gates. It keeps type identity in `WireValue`
and the linked signature hash, never in JSON or field/type name strings.
The in-memory rule avoids accidental artifact or disk persistence of raw host
requests and responses.

## Provider and backend impact

Provider authors may use the wrapper in conformance and regression tests. The
reference VM, runner, and experimental backends remain unchanged; they still
own execution scheduling and reporting.

## Evidence

Provider API tests record a canonical wire call, replay it without invoking the
real Provider, reject argument drift without consuming the tape, and reject a
Provider that did not explicitly opt into deterministic replay.
