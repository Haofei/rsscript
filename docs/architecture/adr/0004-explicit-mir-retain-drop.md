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

The same ownership model also has `TakePlace { destination, place }` for a
standalone checked `take local` expression. It defines the moved value while
transitioning the source place to unavailable, so source lowering cannot erase
that move before a backend observes it.

The direct checked-HIR lowerer emits `Retain` after a resolved call whose
semantic signature declares `retains(param)` and whose argument is a managed
local read view. The v1 bytecode encoder preserves this as verifier-visible MIR
ownership evidence without a separate runtime opcode: retention does not copy
or destroy the VM value. Source-level explicit `drop`, resource transfer, and
new bytecode ownership opcodes remain follow-up work.

## Compatibility and security impact

This is an internal MIR contract change guarded by construction-time
verification. It prevents a backend from silently treating explicit ownership
end points as ordinary reads. Existing bytecode and Provider ABI remain
unchanged.

## Evidence

MIR tests prove that a retained place remains readable and that reading a
dropped place fails validation. The migration suite proves a checked `.rssi`
`retains(value)` contract becomes a direct-HIR `Retain` instruction.
