# ADR 0058: Semantic ownership of structured `match` effect diagnostics

## Status

Accepted; superseded in part on 2026-09-15. The structured-pattern effect
diagnostic this record moved into semantics was removed when the two pattern
forms were made consistent: an omitted scrutinee effect now means `read` for
every pattern form, the same rule as a bare call argument (see
`docs/spec/RSScript_Semantics_v0.7.md` §6.5). The ownership decision, that
pattern-effect rules belong to semantics, still stands.

## Problem

The compiler's match effect traversal diagnosed structured struct/list patterns
that omitted an explicit scrutinee `read`/`mut`/`take` effect. This is a direct
language semantic rule and does not require compiler flow state.

## Decision and non-goals

`rsscript-semantics` owns `structured_match_effect_diagnostic`. The compiler
passes the existing pattern, scrutinee effect, and arm span. Field-level effect
monotonicity, conflicts, and managed-class restrictions remain separate items.

## Compatibility and migration

Diagnostic code, span, message, cause, and manual fix remain unchanged. There
are no Artifact, Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None; source validation behavior remains pre-lowering.

## Provider and backend impact

None.

## Evidence

Focused semantics tests cover missing and explicit effects. Architecture tests
reject duplicate compiler diagnostics.
