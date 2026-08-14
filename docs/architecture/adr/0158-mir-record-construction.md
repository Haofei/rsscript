# ADR 0158: Lower resolved record construction to typed MIR

- Status: Accepted
- Date: 2026-08-14

## Problem

The direct checked-HIR-to-MIR path could read aggregate fields but could not
construct a declared struct or class. That forced otherwise supported source
programs back to the source-shaped executable-IR compatibility route.

## Decision

Add `MirInstruction::MakeStruct { destination, ty, fields }`. `ty` is a
module-local `TypeId` that must resolve to `WireType::Named`; fields are
declaration-ordered layout data and their values are typed `ValueId`s.

The checked-HIR lowerer recognizes only resolved struct/class constructors.
It evaluates call arguments in source order, then emits fields in resolved
constructor-parameter order. The v1 code generator emits its existing
verifier-checked `MakeStruct` instruction with a layout derived from the
canonical named type. The MIR conformance interpreter mirrors record
construction and projection for differential testing.

## Non-goals

This does not add resource construction, sum-variant construction, record
mutation, or pattern projection. It also does not freeze a general record
layout ABI; v1 bytecode still uses its existing transitional layout encoding.

## Compatibility and migration

This is additive to the internal MIR and existing v1 bytecode. It does not
change Artifact, Provider, or runtime ABI versions. The executable-IR bridge
remains only as explicit compatibility support while remaining aggregate and
pattern forms reach parity.

## Verifier, security, and backend impact

MIR validation rejects non-named record types, empty field names, and duplicate
layout fields; normal definition/dominance validation checks every field value.
Bytecode verification validates the emitted layout and register operands.
Record construction conveys no Provider authority or deployment policy.

## Evidence

The direct migration corpus constructs `Box(count: 42)` and projects
`item.count`, comparing the legacy VM, test-only MIR interpreter, and
MIR-produced verified bytecode VM result. Invalid non-record MIR construction
is rejected by a targeted verifier test.
