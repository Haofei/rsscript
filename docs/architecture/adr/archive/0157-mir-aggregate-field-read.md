# ADR 0157: Represent aggregate field reads explicitly in MIR

- Status: Accepted
- Date: 2026-08-14

## Problem

The checked HIR already resolves field access, but the typed CFG MIR had no
field-read operation. A backend therefore had to reject the construct or
re-enter the source-shaped executable-IR compatibility path.

## Decision

Add `MirInstruction::GetField { destination, base, field }`. `base` and
`destination` are typed `ValueId`s, so ordinary MIR definition, dominance, and
data-flow verification applies. `field` is serialized aggregate data used by
the existing VM object representation; it is not a function, type, or other
executable identity.

The checked-HIR lowerer and the temporary executable-IR bridge both lower a
resolved field expression to this instruction. VM codegen emits the existing
verifier-checked `GetField` bytecode instruction. The test-only MIR
conformance interpreter implements the same JSON-object projection.

## Non-goals

This does not define typed record layouts, field mutation, variant projection,
pattern projection, or dynamic protocol dispatch. Those remain separate MIR
capabilities and continue to fail closed where no explicit operation exists.

## Compatibility and migration

This is an additive internal MIR and v1-codegen capability. Existing bytecode
already defines `GetField`; no Artifact schema or Provider ABI changes. The
source-shaped bridge remains only during the broader MIR parity migration.

## Verifier, security, and backend impact

The MIR verifier requires `base` to be defined and dominate the field read.
The ordinary bytecode verifier validates the emitted register operands and
field-string payload. No host authority, Provider contract, or runtime policy
is inferred from a field read.

## Evidence

MIR lowering, conformance interpreter, and MIR-only codegen tests cover an
object construction followed by `GetField`; the emitted Artifact is verified
through the ordinary bytecode verifier.
