# ADR 0078: Semantic ownership of structural type compatibility

## Status

Accepted.

## Problem

Call, return, assignment, closure, and literal checking reused a compiler-owned
rendered-type comparison algorithm, including function qualifiers and parameter
effect normalization.

## Decision and non-goals

`rsscript-semantics::type_compatibility::type_compatible` owns the structural
compatibility rule. Compiler retains type alias expansion, substitution, and
HIR expression type extraction. The compatibility API remains rendered-string
based while legacy HIR fields are migrated to structural type identities.

## Compatibility and migration

The existing behavior for `fresh`, `owned`, `noescape`, function parameters,
containers, and open `Option`/`Result` variants is preserved. No Artifact,
Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover function-effect and nested-container normalization.
Architecture tests require compiler delegation and reject the former helpers.
