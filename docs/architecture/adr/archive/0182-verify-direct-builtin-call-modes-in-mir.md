# ADR 0182: Verify direct builtin call modes in MIR

- Status: Accepted
- Date: 2026-08-14

## Problem

`BuiltinId` made direct core-library call identity explicit, but the initial
MIR verifier accepted whichever argument modes a caller supplied. A malformed
MIR producer could therefore turn a semantic `read` builtin parameter into a
different ownership effect without the verifier detecting it.

## Decision and non-goals

`MirCallTarget::Builtin` now carries the resolved parameter-mode sequence from
checked semantic HIR. The MIR verifier validates call arity and every
`Value`/`BorrowRead`/`BorrowMut`/`Take` argument against that sequence, using
the same mechanism as internal and external calls.

The parameter type table, retention facts, generic instantiation, and async
builtin contract remain follow-up work; this decision is specifically about
ownership-effect identity at the backend boundary.

## Compatibility and migration

No artifact or Provider ABI changes occur. Valid direct builtin calls retain
the same v1 bytecode encoding. Invalid caller-constructed MIR now fails before
code generation rather than relying on backend behavior.

## Verifier and security impact

The verifier becomes stricter: builtin argument effects cannot be silently
rewritten by a MIR producer. This affects no process isolation or Provider
authority boundary.

## Evidence

- direct receiver builtin regression asserts the resolved `read` mode
- MIR/lowering/codegen verification suites
