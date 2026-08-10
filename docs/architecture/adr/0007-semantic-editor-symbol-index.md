# ADR 0007: Semantic ownership of the editor symbol index

## Status

Accepted

## Problem

The file-local declaration, scope, reference, definition, and document-symbol
index was implemented inside `rsscript-compiler`. Editor clients therefore
depended on a compiler façade even though the index consumes only source syntax
and diagnostics spans.

## Decision and non-goals

`rsscript-semantics` owns the complete editor symbol index and its scope and
reference tests. `rsscript-language-service` imports these types and queries
directly from semantics. `rsscript-compiler` retains only the execution-specific
adapter that attaches Rust-lowering names to semantic declarations.

This does not complete workspace-wide import resolution or migrate compiler
type/call/ownership diagnostics; those remain separate S02 work.

## Compatibility and migration

The compiler preserves transitional re-exports for callers while its execution
inventory keeps the same public shape. New language-service code must consume
the semantic exports. Existing cursor, definition, reference, and document
symbol behavior is retained by moving the implementation and its tests intact.

## Verifier and security impact

No executable artifact or verifier format changes. The move narrows the
frontend dependency direction so editor requests cannot acquire VM or Provider
dependencies through symbol indexing.

## Provider and backend impact

Providers and backends are unaffected. The compiler-side inventory adapter is
the only remaining consumer that knows backend lowering names.

## Evidence

`rsscript-semantics`, `rsscript-compiler`, and
`rsscript-language-service` unit suites run after the move; semantic symbol
index tests retain scope, pattern-binding, shadowing, reference, and document
outline coverage.
