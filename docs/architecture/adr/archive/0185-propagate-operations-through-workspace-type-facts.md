# ADR 0185: Propagate operations through workspace type facts

- Status: Accepted
- Date: 2026-08-14

## Problem

`workspace_type_facts_with_operation` previously delegated to the unchecked
workspace-HIR query on a cold type-fact cache. It appeared operation-aware at
its public boundary, but a cancellation or deadline could not interrupt the
HIR work needed to build the type facts.

## Decision and non-goals

The type-fact query now has one shared checked/unchecked implementation. Its
operation-aware path obtains HIR through `workspace_hir_with_operation`, polls
before cache publication, and retains pre/post checks for cache hits.

This covers structural declaration/signature type facts only. Full resolve/type
diagnostics remain a separate migration task.

## Compatibility and migration

No language, Artifact, Provider, or runtime contract changes. Cold operation
queries can terminate before caching type facts when their shared operation has
ended.

## Evidence

- `rsscript-semantics` workspace type-fact cache tests
- cancellation/deadline API checks through `CompilationSession`
