# ADR 0181: Lower direct builtin receivers through MIR

- Status: Accepted
- Date: 2026-08-14

## Problem

After direct core-library calls gained `BuiltinId`, receiver syntax such as
`input.to_uppercase()` still failed closed even though semantic HIR had already
resolved the receiver as parameter zero with an explicit `read`/`mut`/`take`
effect. That made equivalent static and receiver spellings follow different
backend paths.

## Decision and non-goals

The checked-HIR MIR lowerer now lowers a direct builtin receiver before the
remaining call arguments. It preserves the receiver's semantic effect as a
`BorrowRead`, `BorrowMut`, `Take`, or ordinary rvalue `Value`, then emits the
same `MirCallTarget::Builtin(BuiltinId)` used by qualified calls.

Only resolved direct builtins are covered. Dynamic protocol dispatch, generic
or typed intrinsic signatures, async builtins, and receiver calls to other
callee kinds remain outside this change.

## Compatibility and migration

The emitted v1 bytecode uses the same `CallIntrinsic` spelling as the existing
qualified form. No source, Artifact, Provider ABI, or runtime behavior changes
for previously supported paths; a formerly unsupported direct-MIR spelling now
uses the verified default pipeline.

## Verifier and security impact

Receiver effects remain explicit MIR call arguments and are checked by existing
move/borrow dataflow. No Provider, resource, cancellation, budget, or isolation
contract changes.

## Evidence

- receiver builtin → `BuiltinId` → verified-bytecode VM regression
- embedded report pipeline end-to-end execution
