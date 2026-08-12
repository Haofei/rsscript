# ADR 0054: Semantic ownership of `match` scrutinee diagnostics

## Status

Accepted.

## Problem

The compiler body checker owned the accepted-type rule and diagnostic for
`match` scrutinees. The rule itself is semantic, while current alias expansion
and declared type-kind lookup remain compiler HIR adapter facts during the
migration.

## Decision

`rsscript-semantics` owns `match_scrutinee_diagnostic`. The compiler expands a
known alias and supplies whether the resolved type is a declared
sum/struct/class; semantics determines support and creates the diagnostic.

## Compatibility

Diagnostic code, span, message, cause, and fix are unchanged. The temporary
compiler adapter is intentionally limited to obtaining already-resolved facts.

## Security and verification

No Provider, artifact, verifier, or runtime contract changes.

## Evidence

Focused semantic tests cover unsupported map, supported result, and supported
declared-pattern facts. Architecture tests ensure diagnostic ownership stays in
semantics.
