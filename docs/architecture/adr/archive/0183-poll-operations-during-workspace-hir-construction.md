# ADR 0183: Poll operations during workspace HIR construction

- Status: Accepted
- Date: 2026-08-14

## Problem

`CompilationSession::workspace_hir_with_operation` checked cancellation and
deadline only before and after building a cold workspace HIR. A workspace with
many source/interface files could therefore continue parsing and isolating
after the caller had cancelled the editor or CLI request.

## Decision and non-goals

The operation-aware query now polls before each source/interface parse, before
and after namespace isolation, and before caching the resulting HIR. Cache-hit
queries retain the same pre/post checks. The unchecked convenience method uses
the same construction implementation with no operation context.

This does not yet migrate the complete compiler diagnostic analyzer into
semantics; it strengthens the session-owned HIR query that those future
diagnostics will consume.

## Compatibility and migration

No source, Artifact, Provider, or SDK wire contract changes. A cancelled or
expired operation can now stop a cold query earlier and does not populate its
workspace-HIR cache.

## Evidence

- `rsscript-semantics` session/cache/cancellation suite
- language-service architecture tests through the shared session API
