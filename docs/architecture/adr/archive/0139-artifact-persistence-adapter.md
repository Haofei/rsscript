# ADR 0139: Artifact persistence is an external compiler adapter

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler package module owned lock acquisition, confined artifact reads,
directory creation, and atomic publication. Those operating-system operations
are not frontend semantics and unnecessarily widened the compiler execution
closure.

## Decision and non-goals

`rsscript-artifact-store` now owns the confined persistence implementation.
It provides `ArtifactStore` plus the existing one-shot atomic publication
helper. The adapter owns path confinement, symlink/reparse-point rejection,
cooperating-process locking, bounded reads, and atomic write durability.

Compiler package metadata imports the adapter directly. The compiler retains a
temporary execution-feature re-export solely for the existing compatibility
surface; new persistence callers should depend on the adapter, not compiler.
This decision does not move package graph capture, lock generation, review
presentation, native snapshots, or generated-Rust output.

## Compatibility and migration

The `ArtifactStore` API and publication behavior are unchanged. Existing
compiler and SDK compatibility imports continue to compile while consumers
migrate to `rsscript-artifact-store`. The adapter has no Artifact wire-format,
language, Provider, or runtime ABI authority.

## Verifier and security impact

The same fail-closed confinement checks remain in force: artifact paths must
be non-empty relatives, symlink/reparse-point paths are rejected, reads are
bounded, and publication uses a lock plus an atomic staged write. Moving the
implementation does not admit unverified Artifact bytes to the VM.

## Provider and backend impact

None. The adapter depends only on persistence primitives and is outside the
frontend, Provider, VM, and experimental backend boundaries.

## Evidence

`rsscript-artifact-store` retains its confinement, bounded-read, symlink,
locking, and concurrent-publication tests. The SDK architecture test verifies
the adapter does not depend on compiler or runtime code and that compiler
keeps it execution-only.
