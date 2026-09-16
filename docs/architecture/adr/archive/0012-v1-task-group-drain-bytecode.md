# ADR 0012: v1 task-group drain bytecode

## Status

Accepted

## Problem

Typed MIR could prove that a lexical task group is joined, but the v1 codegen
path rejected `Join`. This left verified structured-concurrency facts unable to
reach the reference VM and encouraged a fallback to the legacy source-shaped
lowerer.

## Decision and non-goals

The v1 payload now has `JoinTasks { handles }`. MIR codegen derives its handles
only from `Spawn` instructions with the same resolved `TaskGroupId`. The VM
parks the parent until every still-live child is complete, then reaps completed
children and resumes the parent without exposing child values. A task already
awaited by the parent is absent from the task table and is treated as already
drained.

This ADR does not add source-level `join` syntax, cancellation delivery,
external async calls, or `select`. `Cancel` continues to fail closed in
codegen. It also does not make the in-process VM a sandbox.

## Compatibility and migration

`JoinTasks` is an additive, checked v1 instruction in the pre-1.0 payload. Old
Artifacts remain readable because they do not contain it; a runtime that lacks
the instruction rejects it through the existing fail-closed opcode validation.
MIR group joins can now use the normal codegen -> verifier -> VM path rather
than a legacy execution fallback.

## Verifier and security impact

The bytecode verifier validates the exact `handles` register field. The MIR
verifier remains the owner of lexical group membership and live-task closure;
the VM defensively handles already-reaped handles while never treating a
running child as drained. No authorization or isolation guarantee changes.

## Provider and backend impact

Providers are not involved: this is internal async scheduling. Experimental
backends must either implement equivalent group-drain semantics or reject MIR
`Join`; they must not silently run a parent ahead of its children.

## Evidence

The migration suite builds a manual typed MIR group, verifies its Artifact,
runs it through the VM, and asserts that both root and child complete with no
live tasks remaining. Bytecode and VM unit suites cover the updated instruction
contract and exhaustive register accounting.
