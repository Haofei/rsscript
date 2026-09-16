# ADR 0166: Guard bytecode backends from frontend dependencies

- Status: Accepted
- Date: 2026-08-14

## Problem

The intended execution boundary is checked HIR to MIR to verified bytecode.
Cargo metadata alone cannot detect source-level inclusion or an accidental
frontend import in a VM, codegen, or JIT backend, which would make language
semantics diverge between execution paths.

## Decision

Add an architecture test that recursively checks VM, MIR codegen, and JIT-lab
sources for frontend imports and verifies their direct Cargo dependencies do
not include compiler, syntax, semantics, or lowering crates. These backends
may consume MIR, bytecode, and their stable runtime contracts only.

## Non-goals

This does not claim Rust AOT has completed its separate migration to MIR or
bytecode; it remains experimental and is deliberately outside this guard.

## Compatibility and migration

The test changes no language, Artifact, Provider, or SDK wire contract. It
prevents new coupling while the legacy and experimental paths are removed in
their own migration steps.

## Verifier and security impact

Keeping bytecode execution detached from source representations reduces the
trusted surface of the verifier/VM boundary. It does not change isolation or
Provider authority.

## Evidence

`rsscript-sdk`'s architecture suite runs the recursive source and Cargo
metadata checks for the three bytecode backends.
