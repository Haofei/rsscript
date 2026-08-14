# ADR 0161: Lower flat variant payload bindings to explicit MIR projections

- Status: Accepted
- Date: 2026-08-14

## Problem

Tag-only variant dispatch could reach direct MIR, but a match arm could not
use a declared variant payload without returning to the legacy executable-IR
path.

## Decision

For a resolved sum-variant pattern, the direct MIR lowerer validates that the
positional binding count equals the semantic variant layout. Each flat binding
or wildcard corresponds to one declared field. In the matching CFG block it
emits `GetField` and `WritePlace` for named bindings; wildcards emit no
projection. This keeps payload access in owned values and explicit locals,
with no source pattern node in MIR.

## Non-goals

Nested patterns, record patterns, list patterns, guards, match-time moves, and
variant mutation remain fail-closed. They require separate ownership,
projection, and cleanup semantics.

## Compatibility and migration

The change is internal MIR lowering over existing verified bytecode operations.
Artifact, Provider, and runtime ABI versions remain unchanged. The legacy path
remains an explicit compatibility fallback for unsupported pattern forms.

## Verifier, security, and backend impact

Existing value-definition, dominance, CFG, and field-validation rules check
the emitted operations. A dual-path migration case compares a user-defined
payload binding across the legacy VM, test-only MIR interpreter, and verified
MIR bytecode VM for both statement and expression arms. No host capability or
authorization semantics change.
