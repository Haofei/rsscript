# ADR 0148: Workspace capture requires explicit bases

- Status: Accepted
- Date: 2026-08-14

## Problem

The workspace loader had both explicit-base capture APIs and compatibility
methods that resolved relative paths through the process current directory.
That ambient lookup made the package input depend on mutable process state and
left an avoidable path from editor/package capture to non-reproducible input.

## Decision and non-goals

`WorkspaceLoader` now exposes only `snapshot_from`,
`snapshot_from_with_operation`, and `load_from`. Every caller supplies the
base path used to resolve a relative package directory. The LSP captures from
its already-selected package root and a literal relative package path; it does
not consult process state.

This does not make package manifests fully typed, move package loading out of
the compiler compatibility layer, or alter dependency discovery. It only
removes ambient current-directory resolution from the loader contract.

## Compatibility and migration

The removed `snapshot`, `snapshot_with_operation`, and `load` methods were
pre-1.0 compatibility conveniences. Embedders migrate by passing the intended
base directory to the corresponding `_from` method. Snapshot bytes, content
digests, Artifact contracts, and Provider contracts are unchanged.

## Verifier and security impact

No bytecode or Artifact verifier behavior changes. Making filesystem capture
explicit reduces accidental capture of a different working directory and
preserves the existing boundary where compiler inputs are immutable snapshots.

## Provider and backend impact

Providers and execution backends are unaffected. The LSP and loader tests
exercise the same explicit capture boundary used by embedding callers.

## Evidence

Architecture tests reject ambient-current-directory loader APIs. Workspace
loader unit tests cover explicit-base capture and structured missing-root
errors; the LSP checks against the explicit loader API.
