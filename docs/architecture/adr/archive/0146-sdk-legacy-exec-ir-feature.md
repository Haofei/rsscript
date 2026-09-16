# ADR 0146: SDK execution excludes legacy executable IR

- Status: Accepted
- Date: 2026-08-14

## Problem

The SDK's ordinary `execution` feature selected the VM's source-shaped
`legacy-exec-ir` lowering feature. This made the reviewed embedding path depend
on a migration compatibility backend even for hosts that require only verified
MIR bytecode execution.

## Decision and non-goals

`rsscript-sdk/execution` now selects the compiler, MIR codegen, verifier, VM,
and Provider contracts without legacy executable IR. A new
`rsscript-sdk/legacy-exec-ir` feature selects the VM compatibility lowering;
the legacy root `compatibility` feature opts into it for existing corpus and
callers.

If direct MIR or MIR codegen reports an unsupported construct under ordinary
execution, the SDK returns a clear error. It does not silently select or invoke
the old lowering path. This does not remove executable IR or make all language
constructs MIR-supported.

## Compatibility and migration

Existing legacy users selecting `compatibility` retain behavior. New reviewed
embedders using `execution` must restrict inputs to supported MIR constructs or
make a deliberate temporary `legacy-exec-ir` feature choice. Artifact and
Provider contracts do not change.

## Verifier and security impact

The normal execution closure now reaches the VM only through MIR codegen and
the bytecode verifier. This is an architectural narrowing, not an isolation or
sandbox claim.

## Provider and backend impact

Provider linkage is unchanged. Experimental backends must consume direct MIR
or explicitly declare any compatibility dependency.

## Evidence

Feature-matrix checks compile SDK execution without VM legacy IR and compile
compatibility with it. Architecture tests reject the legacy feature in ordinary
execution and require it in the explicit compatibility layer.
