# ADR 0138: Workspace capture observes the shared operation contract

- Status: Accepted
- Date: 2026-08-14

## Problem

The independent workspace loader formed the intended filesystem boundary, but
long recursive capture and manifest traversal could not observe an embedding
request's cancellation token or deadline.

## Decision and non-goals

`WorkspaceLoader` provides `snapshot_from_with_operation` and
`snapshot_with_operation`. Capture checks the shared `OperationContext` before
and throughout directory scanning, dependency traversal, manifest parsing, and
source reads. Cancellation and deadline exhaustion use structured loader error
codes.

The existing non-operation APIs remain compatibility conveniences. This does
not yet migrate the compiler's richer package graph, lock, native snapshot, or
artifact persistence implementation into the loader.

## Compatibility and migration

Existing loader results are unchanged for live requests. New I/O-bound callers
should use the explicit-base operation-aware API; this avoids both ambient
current-directory reliance and uninterruptible capture.

## Verifier and security impact

Aborted capture returns no snapshot, so compiler and Artifact stages cannot
consume a partial input. The change grants no filesystem or Provider authority
beyond the caller's existing loader invocation.

## Provider and backend impact

None. The loader remains outside compiler, VM, Provider, and backend
dependency closures.

## Evidence

Loader tests prove cancellation and deadline failure before capture. The
architecture gate verifies the operation dependency and public capture API.
