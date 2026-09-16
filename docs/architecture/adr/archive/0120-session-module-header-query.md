# ADR 0120: Session-owned module-header query

- Status: Accepted
- Date: 2026-08-14

## Context

Clients need module and import facts before full type checking, especially for
dependency invalidation. Re-parsing or scanning source text in every client
would create divergent cache and grammar behavior.

## Decision

CompilationSession owns a revision-keyed ModuleHeader query for source and
interface documents. The query consumes the session parse cache and exposes
only parsed module and import paths.

## Consequences

Language-service and future workspace queries can share one syntax-derived
dependency fact. The query is intentionally below HIR and does not make the
session a filesystem loader or an analyzer compatibility facade.
