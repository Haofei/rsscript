# ADR 0134: Make workspace diagnostics a session-owned query

## Status

Accepted

## Problem

`CompilationSession` already owned revisioned source/interface input and local
parse/HIR queries, while `rsscript-language-service` maintained a separate
workspace-diagnostic cache. Those caches could otherwise represent different
document sets and cancellation boundaries.

## Decision and non-goals

`CompilationSession::workspace_diagnostics_with_operation` owns the immutable
frontend input snapshot, revision invalidation, cache lifetime, and operation
checks for a complete workspace diagnostic query. The language service supplies
the transitional compiler analyzer as a callback and retains only LSP-facing
diagnostic/lint presentation work.

This does not finish semantic analyzer migration: resolve/type and
interface-aware workspace HIR still use the compiler transition path. It also
does not yet implement dependency-precise invalidation for whole-workspace
diagnostics.

## Compatibility and migration

No language, Artifact, Provider ABI, SDK, or persisted-data contract changes.
The public semantic session gains an additive query API. Existing language
service callers retain their `workspace_diagnostics` API.

## Verifier and security impact

No verifier or execution behavior changes. Cached results cannot escape a
cancelled or expired operation because the session checks the operation before
cache access and before storing or returning the result.

## Provider and backend impact

None.

## Evidence

Semantic tests prove cache reuse, immutable source/interface capture, and
revision invalidation. Language-service and LSP tests exercise the same
workspace diagnostics path.
