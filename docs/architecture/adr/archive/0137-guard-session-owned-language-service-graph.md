# ADR 0137: Guard the session-owned language-service module graph

- Status: Accepted
- Date: 2026-08-14

## Problem

The language service completed its migration from a local dependency cache to
`CompilationSession::WorkspaceModuleGraph`, but an architecture test still
required the deleted cache name. That made the intended frontend boundary
ambiguous and permitted a future reintroduction of a competing dependency
model.

## Decision and non-goals

Architecture tests now require language-service source to consume the shared
workspace module graph and reject both a local `dependency_cache` field and a
local string-based module matching helper. This is a regression guard for ADR
0136, not a new public query or a replacement for semantic name resolution.

## Compatibility and migration

No public API, Artifact schema, Provider contract, or runtime behavior changes.
Internal editor code must use `CompilationSession` for parsed module facts.

## Verifier and security impact

None. The change is a build-time dependency-boundary guard only.

## Provider and backend impact

None.

## Evidence

The architecture test executes in the default compatibility gate and verifies
the language-service source boundary directly.
