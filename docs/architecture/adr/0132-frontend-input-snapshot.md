# ADR 0132: Pure compilation consumes an immutable frontend input snapshot

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed embedding API accepted individual strings or path-based package
helpers. That made the intended pure compiler boundary easy to bypass and did
not give source/interface bytes a named shared identity before compilation.

## Decision and non-goals

`rsscript-semantics` owns `FrontendInputSnapshot`, which keeps immutable source
and interface snapshots in distinct roles. The SDK exposes
`Compiler::check_snapshot`, `Compiler::compile_snapshot`, and its
operation-aware form as the preferred in-memory embedding path. Existing
single-source and slice-based helpers build this snapshot and remain
compatibility conveniences. This does not yet migrate package graph capture,
manifest parsing, or Artifact persistence out of the compiler package boundary.

## Compatibility and migration

Existing `compile` and `compile_with_interfaces` calls continue to work with
the same output. Embedders may migrate by capturing their buffers once and
calling `compile_snapshot`; no filesystem path is required. Snapshot source
and interface pairs are normalized by logical path/content before frontend
analysis, so equivalent file enumeration orders produce the same Bundle bytes.

## Verifier and security impact

The snapshot is immutable and separates executable source from external
interface contracts. Compilation still emits a digest-bound Artifact, and the
normal verifier remains mandatory before execution. This introduces no host or
Provider authority.

## Provider and backend impact

Interfaces stay provider-neutral semantic input. Compiler, VM codegen, and
future backends receive the same validated content regardless of the eventual
Provider implementation.

## Evidence

Semantic tests prove the role separation. SDK tests retain the existing
compile/verify/run path, and both official embedding examples now construct the
snapshot explicitly before compiling.
