# ADR 0124: Language service delegates module graph facts to CompilationSession

- Status: Accepted
- Date: 2026-08-14

## Problem

The language service already used `CompilationSession` for selected parse and
symbol queries, but it reparsed document text locally when deriving interface
modules, import edges, visibility, and invalidation. That duplicated frontend
grammar and allowed cache behavior to diverge from the shared session.

## Decision and non-goals

All language-service module/import facts now come from the revisioned
`CompilationSession` header query. The service retains only document overlays,
query-result caches, filename fallback for module-less interface files, and LSP
response shaping. This decision does not move workspace semantic diagnostics
out of the compiler façade; that remains the next S04 migration step.

## Compatibility and migration

No language, Artifact, Provider, SDK, or persisted-data format changes. The
observable dependency graph remains the parsed syntax graph, including the
existing interface filename fallback.

## Verifier and security impact

None. The change reduces duplicate parsing paths in editor tooling and keeps
revision invalidation tied to immutable session inputs.

## Provider and backend impact

None. Providers and execution backends do not participate in language-service
module graph queries.

## Evidence

`rsscript-language-service` tests cover comments/string literals, direct and
transitive interface invalidation, and unchanged local query caches. The SDK
architecture test rejects local parse helpers in the language service and
requires its session header-query use.
