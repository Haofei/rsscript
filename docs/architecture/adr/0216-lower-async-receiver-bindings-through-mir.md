# ADR 0216: Lower async receiver bindings through MIR

- Status: Accepted
- Date: 2026-08-14

## Problem

The direct checked-HIR MIR lowerer supported resolved receiver calls only on
the synchronous path. An `async let` whose call used a receiver was rejected
and therefore required the source-shaped executable-IR compatibility path.

## Decision

`async let` now lowers a resolved receiver through the same checked receiver
argument conversion used by an ordinary direct call. The receiver becomes the
first `Spawn` argument and retains its resolved `read`, `mut`, or `take`
effect. Explicit call arguments remain in source evaluation order.

## Consequences

The generated MIR remains typed and verifier-owned; bytecode code generation
continues to consume only `VerifiedMir`. A differential migration test executes
the receiver task binding in both the legacy VM and the direct MIR bytecode
path, requiring the same result and usage accounting. Select, closures, and
other unsupported structured-concurrency shapes remain fail-closed.
