# ADR 0021: Direct MIR lowering for awaited external Provider calls

## Status

Accepted

## Problem

The direct checked-HIR-to-MIR path supported internal task handles but rejected
`await Host.call()` even though the verified VM already suspends the current
task for an asynchronous `CallExternal` Provider binding. This forced an
otherwise supported Provider-neutral program through legacy executable IR.

## Decision and non-goals

When checked HIR contains `await` around a resolved asynchronous external call,
the direct lowerer emits the normal resolved external MIR `Call`. It does not
invent an internal task or a second await operation. The bytecode VM's existing
`CallExternal` dispatch is the single owner of parking and resuming the current
task around that Provider future.

Awaiting a named internal task retains the explicit MIR `Await` operation.
Async bindings to external calls, cancellation syntax, select, and new
Provider ABI fields are outside this decision and remain fail-closed.

## Compatibility and security impact

This is an internal pre-1.0 lowering extension. It introduces no new Artifact
opcode or Provider ABI and preserves the same import signature/linking checks.
The VM still executes only verifier-owned bytecode; Provider cancellation and
deadline behavior remain governed by the linked Provider contract.

## Evidence

The MIR migration suite compares legacy and direct-MIR execution of a
cooperative asynchronous Provider that yields once before returning. Both paths
complete with the same value and Provider-call usage. A second fixture cancels
the shared execution token while the Provider future is pending; both paths
preserve the same structured Provider cancellation failure and usage report.
Both fixtures compare stable Provider-trace fields while deliberately excluding
wall-clock elapsed telemetry. A third fixture crosses a shared monotonic
deadline inside an async Provider and proves both paths retain the same
structured Provider deadline failure.
