# ADR 0010: MIR resource lifetime primitives

## Status

Accepted

## Problem

Resource lifetimes were a semantic fact but had no typed MIR representation.
Backends therefore could not distinguish an ordinary place write from acquiring
or releasing a resource, and invalid hand-authored MIR could return with a live
resource.

## Decision and non-goals

`rsscript-mir` now represents `AcquireResource` and `ReleaseResource` with a
canonical `ResourceTypeId`; acquisition also names the already-defined source
value being placed under runtime resource ownership. MIR validation verifies that the ID resolves to a
`WireType::Resource`, rejects duplicate acquire and release-without-acquire,
and requires every reachable return path to release all live resources.

This establishes the normal-exit primitive only. Explicitly managed linear
`with` scopes lower to it, and VM bytecode codegen maps it to existing
`Manage`/`ResourceDrop` instructions. Transfer semantics and cleanup edges for
errors and cancellation remain separate milestones.

## Compatibility and migration

The change is internal to the pre-1.0 MIR contract and does not change an
Artifact, Provider, or SDK schema. Existing scalar MIR remains valid. New MIR
consumers must either implement resource operations or fail closed.

## Verifier and security impact

Resources can no longer silently cross a reachable normal return in verified
MIR. The verifier also prevents a non-resource ABI type from masquerading as a
resource lifetime. This is not a sandbox claim and does not yet prove cleanup
on provider errors or cancellation.

## Provider and backend impact

The conformance interpreter preserves the acquired source value for
control-flow tests. The VM code generator maps the linear resource subset to
`Manage` and `ResourceDrop`; scheduler/error/cancellation cleanup and providers
remain follow-up work.

## Evidence

`rsscript-mir` unit and conformance tests cover valid release and leaked
resource rejection. `rsscript-lowering` tests cover managed `with` lowering,
and `rsscript-codegen-vm` verifies the emitted resource Artifact.
