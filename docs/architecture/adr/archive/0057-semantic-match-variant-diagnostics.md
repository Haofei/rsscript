# ADR 0057: Semantic ownership of variant `match` diagnostics

## Status

Accepted.

## Problem

The compiler pattern traversal owned three language diagnostics: a pattern that
cannot match the scrutinee, a variant outside the allowed family, and a variant
whose positional bindings have the wrong arity.

## Decision and non-goals

`rsscript-semantics` owns `match_pattern_type_diagnostic`,
`match_variant_family_diagnostic`, and `variant_pattern_arity_diagnostic`.
Compiler continues to look up declared variants and fields, then supplies those
resolved facts to the semantic diagnostic constructors. Nested-pattern traversal
is not moved by this ADR.

## Compatibility and migration

All diagnostic codes, spans, titles, text, causes, and manual fixes are
preserved. No Artifact, Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None. Invalid patterns remain rejected during frontend validation.

## Provider and backend impact

None.

## Evidence

Focused semantic tests cover family, arity, and general pattern failures.
Architecture tests prohibit duplicate compiler diagnostic helpers.
