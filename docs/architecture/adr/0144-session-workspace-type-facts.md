# ADR 0144: Compilation sessions cache workspace type facts

- Status: Accepted
- Date: 2026-08-14

## Problem

`CompilationSession` cached parse trees, HIR, module headers, and diagnostics,
but a consumer needing declaration/signature type facts had no session query.
It could therefore reconstruct types from another workspace view and bypass
the session's revision and operation boundary.

## Decision and non-goals

The session owns `workspace_type_facts` and its operation-aware counterpart.
The query obtains the immutable `SemanticTypeFacts` already interned by the
namespace-isolated workspace HIR, caches that `Arc`, and invalidates it on any
source or interface revision change.

This is not a claim that all resolve/type diagnostics have migrated from the
compiler. The query exposes only the declaration and signature facts already
owned by HIR; full semantic validation remains a separate migration task.

## Compatibility and migration

The query is additive and has no language, Artifact, Provider, or execution
ABI change. Existing callers may retain transitional compiler diagnostics while
they migrate to semantic queries.

## Verifier and security impact

None. The query is pure in-memory frontend state and checks cancellation and
deadlines before and after accessing cached facts.

## Provider and backend impact

None. Backend consumers still require validated semantic output or MIR.

## Evidence

Semantics tests cover cache identity, revision invalidation, and cancellation.
The architecture test requires the session-owned cache and rejects a
language-service-local reconstruction of `SemanticTypeFacts`.
