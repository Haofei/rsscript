# ADR 0019: Direct MIR lowering for literal match statements

## Status

Accepted

## Problem

Checked HIR carried resolved match arms, but the direct HIR-to-MIR path rejected
every match and fell back through `ExecutableIr`, even for simple scalar
dispatch that MIR can represent with existing equality and CFG primitives.

## Decision and non-goals

Direct lowering now accepts statement `match` arms using scalar literals or
`_`, provided no arm has a guard. Literal tests become a fresh `MirLiteral`,
`Binary(Equal)`, and `Branch`; each arm has an explicit block and falls through
to a join block only when it does not terminate. Failure to match every arm
reaches an explicit `Unreachable` terminator.

Expression-form scalar matches use the same dispatch CFG. Each non-terminating
arm writes its final expression to a synthetic MIR place, then the join reads
that place into the expression result `ValueId`; arms that explicitly return
remain terminal. This is a CFG normal form, not a hidden source-shaped match
node.

Variant, struct, list, and nested patterns; guards; and field extraction remain
fail-closed. They require dedicated typed MIR projection operations and may not
be reconstructed from syntax by a backend.

## Compatibility and security impact

This is an internal pre-1.0 MIR lowering extension. It adds no Provider ABI or
Artifact schema field. Existing MIR validation checks the generated CFG and
value dominance before code generation; unmatched execution is explicit rather
than silently selecting an arm.

## Evidence

The migration suite compiles checked-HIR statement and expression integer
literal matches directly to MIR, verifies their branch CFG, emits verified
bytecode, and executes selected arms with results `42` and `one`.
