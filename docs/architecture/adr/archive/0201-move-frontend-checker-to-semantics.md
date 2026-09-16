# ADR 0201: Make semantics own the complete frontend checker

- Status: Accepted
- Date: 2026-08-14

## Problem

`rsscript-semantics` owned the semantic models and session contracts, while
the complete source analyzer and check implementation still lived in
`rsscript-compiler`. The language service could therefore avoid a direct
compiler dependency only by injecting a compiler callback through a semantic
contract. This left two contradictory ownership boundaries and made editor
and compiler migration depend on a compatibility facade.

## Decision

Move the complete analyzer and language checks into `rsscript-semantics`.
The semantic crate exports the source and immutable-workspace frontend entry
points, including the operation-aware diagnostic query. `rsscript-compiler`
keeps a crate-private forwarding module and public re-exports only as a
temporary compatibility surface for package and experimental AOT callers.

`rsscript-language-service` and `rss-lsp` must call the semantic query via
the language-service boundary and must not depend on `rsscript-compiler`.

## Consequences

There is one physical owner for parser-to-validated-program checking and for
the assembly of `SemanticDatabase`. Compiler purity work can now remove the
remaining compatibility re-exports incrementally without moving frontend
rules again. The semantic public surface temporarily expands; a later SDK/API
cleanup may narrow it behind intentionally versioned facade types.
