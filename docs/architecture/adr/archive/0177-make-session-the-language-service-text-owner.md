# ADR 0177: Make CompilationSession the language-service text owner

- Status: Accepted
- Date: 2026-08-14

## Problem

`LanguageService` already delegated parse, HIR, module-graph, and workspace
diagnostic queries to `CompilationSession`, but also retained a second `Arc<str>`
copy for every document. That made session revision state and editor document
state independently authoritative.

## Decision and non-goals

`CompilationSession` now exposes immutable per-file snapshots, including a
shared text handle. `LanguageService` retains only editor protocol revision and
source/interface role metadata; formatting, linting, symbols, and public
document snapshots read bytes from the session-owned source store.

This does not yet move the full semantic diagnostic analyzer into
`rsscript-semantics`, nor does it claim dependency-precise workspace diagnostic
invalidation.

## Compatibility and migration

The public `DocumentSnapshot` shape is unchanged. Its text now comes from the
same immutable revision used by the semantic queries, so a language-service
response cannot accidentally combine session analysis with a separate cached
buffer.

## Architecture impact

This removes one input-cache ownership path from the editor layer and narrows
the language service toward an LSP overlay/protocol adapter over the shared
frontend session. No OS loader, compiler, VM, Provider, or package dependency
is introduced.

## Evidence

Semantic session and language-service test suites cover file replacement,
deletion, parsing, diagnostics, formatting, and symbols. Targeted clippy passes
for both crates.
