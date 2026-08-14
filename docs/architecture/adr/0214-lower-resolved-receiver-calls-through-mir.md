# ADR 0214: Lower resolved receiver calls through typed MIR

- Status: Accepted
- Date: 2026-08-14

## Decision

The checked HIR already resolves receiver calls and records the receiver's
`read`/`mut`/`take` effect independently from ordinary call arguments. Direct
MIR lowering now places that receiver at parameter slot zero, preserves its
borrow/move mode, and emits `Retain` when the resolved first parameter has a
retention contract. The remaining named arguments retain their semantic
evaluation order and parameter binding.

## Impact

No source syntax, Provider ABI, or bytecode opcode changes. The change removes
one executable-IR fallback condition and lets existing MIR ownership validation
cover receiver calls just as it covers ordinary calls.

## Evidence

The MIR migration corpus contains a checked record receiver call and compares
legacy execution, the MIR interpreter, and verified MIR-produced bytecode.
