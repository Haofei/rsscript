# ADR 0015: MIR JSON-object construction

## Status

Accepted

## Problem

JSON object literals were still handled only by the transitional
source-shaped executable-IR path despite the reference VM already exposing a
bounded `MakeObject` operation. Treating these JSON data values as language
records would incorrectly introduce unresolved record-layout identity into
MIR.

## Decision and non-goals

MIR adds `MakeObject { destination, fields }`. Each field pairs serialized JSON
data text with a resolved value `ValueId`; normal dominance validation covers
the values, and codegen emits the v1 `MakeObject` field/register payload.

This applies only to JSON object literals. It does not add language record or
variant layouts, record field IDs, typed field access, mutation, object
patterns, or match dispatch. Those features need explicit resolved layout IDs
and remain rejected by the MIR lowerer.

## Compatibility and migration

The new MIR operation is additive pre-1.0 and uses an existing v1 bytecode
opcode. Existing Artifacts remain readable; runtimes without the opcode reject
it normally. Legacy lowering remains available only for operations outside the
supported JSON-object construction subset.

## Verifier and security impact

Object field names are JSON payload data, not callable, type, Provider, or
record-layout identities. MIR validates the values and the bytecode verifier
checks the bounded field/register encoding. No Provider authority, resource,
cancellation, or isolation semantics change.

## Provider and backend impact

Providers are unaffected. Backends may implement this operation only with the
same JSON-data semantics; they must reject language record operations until a
separate typed layout contract exists.

## Evidence

The lowerer unit test checks explicit `MakeObject` construction. The migration
corpus compiles a source JSON object, runs the MIR oracle, legacy VM, and
verifier-approved MIR bytecode VM, then compares the canonical JSON result.
