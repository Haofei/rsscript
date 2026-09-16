# ADR 0150: Separate compiler lowering from package compatibility

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's `execution` feature selected both the provider-neutral
HIR-to-MIR lowering path and filesystem/package/review/persistence
compatibility code. Consequently, even the reviewed SDK's in-memory execution
path compiled package I/O dependencies that it never used.

## Decision and non-goals

`rsscript-compiler` now has a `lowering` feature for compiler output, typed
MIR, and provider-neutral lowering contracts, and a `package` feature for
filesystem package capture, review, persistence, and package-specific
lowering input. `package` includes `lowering`; the historical `execution`
feature includes `package` for compatibility. The reviewed SDK `execution`
feature selects only compiler `lowering`, while the explicit SDK `project`
feature selects compiler `package`.

This does not yet move package implementation modules into a new crate or
remove the historical compatibility feature. It narrows the build dependency
closure at the public embedding boundary first.

## Compatibility and migration

No language, Artifact, bytecode, Provider, or package format changes occur.
Existing consumers of compiler `execution` retain its complete closure.
Embedders using the reviewed SDK continue to use `execution` for immutable
snapshots and add `project` only when they require path/package convenience.

## Verifier and security impact

Verifier and runtime semantics are unchanged. The split removes filesystem and
persistence code from the normal in-memory compilation closure, reducing the
trusted dependency surface of hosts that never capture package paths.

## Provider and backend impact

MIR lowering retains its existing provider-neutral import contracts. Providers,
the VM, and experimental backends do not select compiler package code unless a
composition root explicitly opts into the project compatibility path.

## Evidence

Architecture tests assert the `lowering`/`package` dependency split and reject
compiler package capture from reviewed SDK `execution`. Compiler and SDK are
compiled under default, lowering, package, compatibility, and project feature
sets.
