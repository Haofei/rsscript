# ADR 0004: Preserve retain and drop in MIR

## Status

Accepted

## Problem

The first MIR ownership subset represented `read`, `mut`, and `take` only as
call-argument modes. Retention and explicit ownership end points could
therefore disappear before a backend or verifier observed them.

## Decision and non-goals

MIR now has explicit `Retain { place }` and `Drop { place }` instructions.
`Retain` requires a live place without consuming it. `Drop` requires a live
place and transitions it to the same unavailable state as a move; assignment
can reinitialize that state. The existing CFG move dataflow applies this rule
at joins.

This decision does not yet lower source retention/resource syntax or add v1
bytecode opcodes. The code generator rejects these operations until their
runtime cleanup and conformance contracts are complete.

## Compatibility and security impact

This is an internal MIR contract change guarded by construction-time
verification. It prevents a backend from silently treating explicit ownership
end points as ordinary reads. Existing bytecode and Provider ABI remain
unchanged.

## Evidence

MIR tests prove that a retained place remains readable and that reading a
dropped place fails validation.
