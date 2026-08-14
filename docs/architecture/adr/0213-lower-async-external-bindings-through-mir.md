# ADR 0213: Lower async external bindings through typed MIR wrappers

- Status: Accepted
- Date: 2026-08-14

## Problem

Direct checked-HIR lowering supported `await Host.call()` by emitting a
resolved external call, but rejected `async let value = Host.call()`. Falling
back to executable IR for that one structured-concurrency shape would keep the
legacy IR on the normal bytecode path.

## Decision

For each resolved asynchronous external import used by a checked HIR module,
the MIR lowerer emits one synthetic asynchronous function only when the import
appears in `async let`. Its only executable operation is a typed `CallExternal`
using the canonical import ID and signature; its parameters preserve the
import's `read`/`mut`/`take` modes. An `async let` binding spawns that function
through the ordinary `Spawn` instruction and the existing `Await`/`Join` task
lifecycle. A direct `await Host.call()` emits no wrapper.

This keeps Provider calls in the import table, lets the existing MIR verifier
check call modes and structured tasks, and lets codegen produce ordinary
`SpawnTask` plus `CallExternal` bytecode. No Provider-specific spawn opcode or
runtime linking path is introduced.

## Non-goals

This does not add cancellation syntax, permit mutable async arguments where
the bytecode backend still rejects them, or turn providers into MIR function
definitions. Direct awaited external calls remain a simpler single-task path.

## Evidence

The MIR migration suite compiles a task-group `async let` calling an async
Provider, verifies the generated wrapper name is present only as debug
metadata, and compares legacy and direct-MIR bytecode execution result, usage,
and stable Provider traces.
