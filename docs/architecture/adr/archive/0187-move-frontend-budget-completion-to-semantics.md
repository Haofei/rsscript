# ADR 0187: Move frontend budget completion to semantics

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler compatibility analyzer owned the budget-aware diagnostic sink,
the `analysis_incomplete` diagnostic, and the conversion from an exhausted
frontend budget into a semantic completion fact. That duplicated the frontend
control plane between compiler composition and the semantic session boundary.

## Decision and non-goals

`rsscript-semantics` now owns `AnalysisDiagnostics`, terminal incomplete
diagnostic construction, and budget-to-completion conversion. The compiler
retains type aliases while its legacy analyzer is migrated incrementally, but
does not construct this diagnostic itself.

This does not move the entire compatibility analyzer or its remaining
resolve/type queries; it establishes one semantic owner for their shared
operation budget and terminal evidence.

## Compatibility and migration

The diagnostic code, message, fix, and `FrontendCompletion` values are
unchanged. The compiler aliases preserve internal call sites during migration;
there is no language, Artifact, Provider, or runtime wire-format change.

## Verifier and security impact

The same bounded operation controls apply. Centralizing the terminal fact
prevents CLI, editor, and future semantic-session callers from producing
different incomplete-analysis evidence for the same exhausted budget.

## Evidence

- `rsscript-semantics` diagnostic-budget terminal-fact test
- compiler/frontend regression tests
- architecture guard rejecting direct compiler budget diagnostics
