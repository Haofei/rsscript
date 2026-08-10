# ADR 0016: resolved MIR list indexing

## Status

Accepted

## Problem

Typed MIR could construct a list but could not consume it through source index
syntax. Sending every index expression through a generic runtime operation
would lose the checked collection-kind fact and could accidentally treat map,
JSON, or future record indexing as list access.

## Decision and non-goals

MIR adds `ListGet { destination, list, index }`. The transitional projection
must carry a checked `List<...>` type name for the index base; otherwise lowering
fails closed. Both list and index are resolved `ValueId`s and codegen maps the
operation to v1 `ListGet`.

This does not add map/JSON/record indexing, field access, bounds-policy changes,
list mutation, or pattern matching. Those forms need their own resolved
operation and validation contract.

## Compatibility and migration

This additive pre-1.0 MIR operation targets the existing v1 `ListGet` opcode.
Existing Artifacts remain readable and runtimes lacking the opcode reject it
normally. Unsupported index bases remain on the explicit compatibility path or
fail during MIR lowering; they are not miscompiled.

## Verifier and security impact

The operation adds no authority or Provider behavior. MIR verifies both inputs
are defined on all control-flow paths; bytecode validates the exact bounded
register fields. Bounds errors retain the VM's existing script-error semantics.

## Provider and backend impact

Providers are unaffected. Backends must preserve list-index behavior or reject
the operation; they must not infer collection kind from source text.

## Evidence

Lowering unit coverage asserts the explicit `ListGet` instruction. The SDK
migration corpus compiles a typed local list index and compares its result across
the legacy VM, MIR oracle, and verifier-approved MIR bytecode VM.
