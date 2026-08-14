# ADR 0152: Inject transitional diagnostics into the language service

- Status: Accepted
- Date: 2026-08-14

## Problem

`rsscript-language-service` owned a `CompilationSession` but also directly
depended on `rsscript-compiler` to run the remaining full diagnostic analyzer.
That made an editor-facing frontend crate depend on the historical compiler
façade and embedded a second multi-file analysis sequence inside its own
implementation.

## Decision

`LanguageService` now requires an explicit `WorkspaceDiagnosticAnalyzer` at
construction. It owns document revisions, immutable source/interface snapshots,
module graph facts, operation checks, semantic-result caching, and local editor
queries. Both workspace and per-document diagnostics consume the same
session-owned workspace diagnostic result; document queries only filter by
source span and add their local lint diagnostics.

The service deliberately has no per-document semantic diagnostic cache.
Interface changes invalidate the session-owned workspace diagnostic query;
precise dependency invalidation belongs in `CompilationSession`, not in an
editor-side approximation.

The LSP application is the composition root for the temporary compiler-backed
adapter. The production language-service dependency closure contains only
syntax, semantics, diagnostics, and operation contracts. Test-only code may
continue to instantiate the compiler adapter while the remaining analyzer
orchestration migrates to semantics.

## Consequences

There is no implicit default semantic analyzer. Hosts must choose an analyzer
explicitly, which prevents a future semantic query engine from being hidden
behind a language-service dependency on the compiler. This is not the final
semantic migration: the compiler adapter remains transitional and is confined
to the LSP composition root.

## Evidence

Language-service tests cover session-shared whole-workspace and document
diagnostics, revision invalidation, cancellation, and deadlines. Cargo metadata
architecture tests reject a production compiler dependency from
`rsscript-language-service`; LSP architecture tests permit compiler analysis
only in the named composition-root adapter.
