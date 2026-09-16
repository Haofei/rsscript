# ADR 0080: Semantic ownership of unresolved generic detection

## Status

Accepted.

## Problem

Call and assignment checking carried separate recursive rules for deciding
whether a rendered type still contained an unresolved generic parameter.

## Decision and non-goals

`rsscript-semantics::type_compatibility` owns `UnresolvedGenericFacts` and the
recursive detection rule. Compiler derives declared types and currently active
generic parameter names. This ADR does not eliminate rendered type strings or
replace HIR type extraction with `TypeId` throughout the frontend.

## Compatibility and migration

Existing suppression of comparisons against unresolved generic types is
preserved. No Artifact, Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover declared, active, nested, and implicit generic facts.
Architecture tests reject the former compiler recursive helpers.
