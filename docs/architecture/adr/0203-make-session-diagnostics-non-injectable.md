# ADR 0203: Make workspace diagnostics a non-injectable semantic query

- Status: Accepted
- Date: 2026-08-14

## Problem

After the analyzer moved into `rsscript-semantics`, `LanguageService` still
accepted an injected `WorkspaceDiagnosticQuery`. That permitted an editor
adapter to choose a different implementation for the same session snapshot and
kept a transitional compiler-composition concept in the public service API.

## Decision

`CompilationSession` now exposes
`semantic_workspace_diagnostics_with_operation`, which uses the semantic-owned
frontend analyzer and the session-owned immutable input/cache. `LanguageService`
has a parameterless constructor and consumes that query directly. LSP code only
manages document overlays and protocol conversion.

## Consequences

There is one diagnostic implementation for normal workspace requests. The
generic query hook remains internal migration machinery for focused session
tests only and is no longer exposed through the language-service boundary.
