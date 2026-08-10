# ADR 0013: MIR-owned list construction

## Status

Accepted

## Problem

The first typed MIR subset could only represent scalar values. Array literals
therefore forced supported source programs back through the transitional,
source-shaped executable-IR path even though the v1 bytecode VM already had a
bounded `MakeList` instruction.

## Decision and non-goals

MIR adds `MakeList { destination, items }`. The list inputs are resolved
`ValueId`s, so normal definition and dominance validation proves every item is
available before list construction. The VM code generator maps that instruction
to the pre-existing v1 `MakeList` opcode.

This ADR deliberately covers only ordered list literals. It does not add MIR
records, variants, maps, field/index access, destructuring, match dispatch,
list element type metadata, or source-level list ownership qualifiers. Those
constructs remain rejected by the MIR lowerer until their operations can be
represented without source AST nodes.

## Compatibility and migration

This is an additive pre-1.0 MIR operation using an existing v1 bytecode
instruction. Existing Artifacts remain readable; runtimes which do not
understand `MakeList` retain their fail-closed opcode behavior. The old
executable-IR bridge remains the compatibility path for the aggregate features
explicitly excluded above.

## Verifier and security impact

No new trust or authorization surface is introduced. The MIR verifier checks
item value dominance, while the ordinary bytecode verifier bounds and validates
the generated item-register array. Construction does not grant a Provider
capability or change cancellation, resource, or isolation semantics.

## Provider and backend impact

Providers are unaffected because lists are language values. Any experimental
backend that accepts this MIR operation must preserve item evaluation order and
must reject it explicitly until it can do so; it must not reconstruct a source
AST or silently use a different aggregate representation.

## Evidence

Lowering unit tests assert that array literals produce `MakeList`; codegen
tests decode and verify the emitted v1 instruction; the SDK migration fixture
compiles source list literals through MIR to a verified Artifact.
