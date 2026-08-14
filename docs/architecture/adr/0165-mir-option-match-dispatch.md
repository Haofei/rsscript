# ADR 0165: Lower canonical Option matches to MIR CFG dispatch

- Status: Accepted
- Date: 2026-08-14

## Problem

The direct checked-HIR route could not construct or dispatch `Some` and
`None`, so ordinary Option control flow still fell back to the legacy
executable-IR lowerer despite existing verified bytecode support.

## Decision

Add `MakeOption`, `UnwrapOption`, and
`MatchOption { value, some_target, none_target }` to MIR. `Some(value)` and
`None` are canonical Option operations, rather than user-defined variant
layouts. A `Some` match accepts one flat binding or wildcard and projects the
payload only in the selected block; `None` accepts no bindings.

VM codegen emits the existing v1 `MakeSome`, `LoadNone`, `UnwrapSome`, and
`MatchOption` operations. The MIR conformance interpreter implements the same
canonical two-state model.

## Non-goals

Nested Option patterns, guards, match-time moves, Option resource transfer,
and arbitrary pattern projections remain fail-closed on the direct MIR path.

## Compatibility and migration

This is an internal lowering expansion over existing bytecode v1 operations;
Artifact, Provider, runtime, and language compatibility versions do not
change. Unsupported forms still use the explicitly gated legacy path during
migration.

## Verifier and security impact

Both Option edges participate in normal CFG, dominance, resource-cleanup, and
task-closure checks. Option payload projection has explicit source/destination
value IDs. No Provider authority, isolation, or sandbox claim changes.

## Provider and backend impact

Providers are unaffected. The reference interpreter and bytecode VM consume
the same explicit MIR operations, giving future backends one semantic model to
implement.

## Evidence

The MIR migration corpus differentially runs `Some` statement and expression
matches with a payload binding through legacy VM, MIR interpreter, and
verified MIR bytecode VM.
