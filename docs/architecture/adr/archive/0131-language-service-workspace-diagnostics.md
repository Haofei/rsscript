# ADR 0131: LSP delegates workspace diagnostics to the language service

- Status: Accepted
- Date: 2026-08-14

## Problem

The LSP owned a second analysis orchestration path: it rebuilt interface/source
sets from its workspace overlay, called compiler analysis functions directly,
then applied its own linting and diagnostic de-duplication. That could diverge
from the revisioned `CompilationSession` used by the language service.

## Decision and non-goals

`rsscript-language-service` now owns a cached workspace diagnostics query.
The LSP supplies immutable workspace/overlay documents with their revisions and
converts the returned diagnostics to JSON-RPC positions; it does not call
analysis or lint entry points directly. This is not the final migration of all
semantic implementation from `rsscript-compiler` into `rsscript-semantics`.

## Compatibility and migration

The LSP protocol and diagnostic DTO are unchanged. Package overlays continue
to diagnose all visible source and interface files before filtering the result
for the requested URI. Query results invalidate whenever a document changes or
is removed.

## Verifier and security impact

This is an editor-only orchestration change. It preserves cancellation checks
around VFS ingestion and query execution, and introduces no execution,
Provider, or Artifact authority.

## Provider and backend impact

None. The language service stays frontend-only and does not depend on the VM,
Providers, or experimental backends.

## Evidence

Language-service tests cover workspace interface resolution, caching, and
invalidation. LSP package, overlay, cancellation, and architecture tests prove
the adapter consumes the single language-service boundary.
