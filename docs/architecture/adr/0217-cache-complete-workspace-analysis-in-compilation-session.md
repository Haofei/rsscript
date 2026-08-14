# ADR 0217: Cache complete workspace analysis in `CompilationSession`

- Status: Accepted
- Date: 2026-08-14

## Problem

`CompilationSession` cached parse trees, local HIR, workspace HIR, structural
type facts, and diagnostics, but callers that needed the complete checked
semantic result still had to instantiate a separate analyzer path. That split
made it possible for a future compiler or editor integration to observe facts
from a different revision than its diagnostics.

## Decision

The session owns a cached `AnalysisResult` query over its immutable
source/interface snapshots. It exposes an operation-aware analysis result and
a phase-gated validated projection. Both cold and cache-hit paths honor the
same cancellation and deadline checks; any source or interface replacement or
removal invalidates the query.

## Consequences

Resolve, type, checked-HIR, and source diagnostics now share one session cache
entry. The query is intentionally broad for now: dependency-precise resolve
and type invalidation remain follow-up work, rather than claiming precision
that has not been implemented. This provides the stable migration target for
CLI/package compiler callers without adding a second frontend cache protocol.
