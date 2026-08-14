# ADR 0207: Lower logical short-circuiting through MIR CFG

- Status: Accepted
- Date: 2026-08-14

Logical `&&` and `||` are control-flow operations, not eager binary arithmetic.
Lowering them as a generic MIR binary operation forced MIR bytecode codegen to
reject the construct and caused supported programs to use the legacy
executable-IR fallback.

The checked-HIR lowerer now emits an explicit branch to either a right-hand
evaluation block or a short-circuit literal block. Both write a result place,
then join before the resulting value is read. This preserves the observable
property that the right side is not evaluated when the left side determines the
result, while keeping the bytecode backend free of source-level logical opcode
semantics.

The migration corpus compares the legacy VM, MIR reference interpreter, and
verified bytecode VM for this capability.
