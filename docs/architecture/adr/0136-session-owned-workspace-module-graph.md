# ADR 0136: CompilationSession owns the workspace module graph query

- Status: Accepted
- Date: 2026-08-14

## Problem

`rsscript-language-service` already reused `CompilationSession` parse trees,
but retained its own dependency cache and repeated the interface filename
fallback. That left an editor-visible module graph that could diverge from
other frontend users as imports, invalidation, or path normalization evolved.

## Decision and non-goals

`rsscript-semantics::CompilationSession` now exposes a cached
`WorkspaceModuleGraph` derived only from its immutable source and interface
snapshots. Each node contains parsed declared modules and imports. Interfaces
without an explicit module declaration use the established filename fallback
inside this query. The graph is invalidated with any source or interface
revision change and is available through an operation-aware API.

The graph also owns interface-visibility closure and transitive dependent-path
calculation. Those are parsed module facts, so keeping them beside the graph
ensures interface rename/removal invalidation cannot diverge between editor
clients.

The language service consumes this graph and no longer owns a separate
dependency cache. This is a syntax-level dependency query, not the final
semantic resolver: it does not prove import validity, type-check a workspace,
or replace the remaining compiler analysis callback.

## Compatibility and migration

The language-service public document API and existing import visibility rules
remain unchanged. Internal callers should obtain workspace import facts from
`CompilationSession` rather than parsing document text or caching dependency
lists independently.

## Verifier and security impact

The graph has no runtime, Provider, deployment, or artifact authority. Its
operation-aware accessor observes cancellation and deadline checks even when
serving cached data, preventing stale query results from escaping an aborted
editor request.

## Provider and backend impact

None. The query remains a frontend-only fact and cannot influence provider
selection, bytecode verification, or execution behavior.

## Evidence

Semantic tests cover graph construction, interface fallback, immutable-cache
reuse, invalidation, and cancelled access. Language-service tests verify that
dependency queries consume the session graph instead of maintaining a local
cache.
