# ADR 0159: Lower resolved sum variants to typed MIR

- Status: Accepted
- Date: 2026-08-14

## Problem

Declared sum-variant constructors were resolved by semantic HIR but the direct
MIR lowerer only recognized `Ok` and `Err`. User-defined variants therefore
could not reach the verified bytecode path without the legacy executable IR.

## Decision

Add `MirInstruction::MakeVariant { destination, ty, variant, fields }`.
`ty` is the canonical named owner sum type. `variant` and its declaration-order
field labels are layout data collected from semantic HIR's resolved variant
table. Arguments retain source evaluation order before being arranged into that
layout order.

The v1 code generator emits the existing verifier-checked `MakeVariant`
bytecode operation. The test-only MIR interpreter represents variants so the
migration corpus can add differential execution coverage as variant matching
is migrated.

## Non-goals

This does not lower `match` dispatch, variant destructuring, pattern guards,
variant field mutation, or a new variant Artifact ABI. Those constructs remain
fail-closed on the direct MIR route until each has explicit CFG semantics.

## Compatibility and migration

This is an additive internal MIR capability using an existing bytecode opcode.
Artifact, Provider, and runtime ABI versions are unchanged. The legacy
executable-IR path remains explicitly gated during the continuing match parity
migration.

## Verifier, security, and backend impact

The same named-type, non-empty-field, duplicate-field, definition, and
dominance rules used for record construction apply to variants. Variant
construction introduces no host capability or policy decision.

## Evidence

The migration test compiles `Value(count: 42)` from checked HIR, asserts a
`MakeVariant` MIR instruction, then emits and verifies the resulting bytecode.
