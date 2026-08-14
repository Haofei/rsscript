# ADR 0127: Runner responses are a fail-closed state machine

- Status: Accepted
- Date: 2026-08-14

## Problem

The runner response JSON Schema described mutually exclusive success and
rejection shapes, but Rust protocol decoding only checked the schema string.
A malformed or malicious child frame could therefore present a completed state
without a report, or a rejection alongside an arbitrary report.

## Decision and non-goals

The protocol validates response state at both write and read boundaries. A
completed response requires exactly a report; every non-completed termination
requires exactly an error. This does not change runner profile authority,
Artifact verification, or OS isolation controls.

## Compatibility and migration

The frame format and v1 JSON fields are unchanged. Previously invalid states
are now rejected consistently with the checked-in schema.

## Verifier and security impact

The parent cannot accidentally treat an ambiguous child response as a valid
execution report. This is protocol hardening, not a sandbox claim.

## Provider and backend impact

None. Providers and VM termination remain represented inside a valid execution
report after successful runner completion.

## Evidence

Protocol tests cover rejected contradictory completed/rejected response shapes,
alongside existing bounded-frame and truncated-frame tests.
