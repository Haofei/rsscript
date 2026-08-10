# ADR 0034: Semantic ownership of generic constraints

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::generic_constraint_diagnostics` owns source-level generic
constraints for resource declaration parameters and fields, plus `fresh`
generic function returns. Compiler declaration passes only append the query.

Call-site type substitution, effect compatibility, and protocol satisfaction
remain separate semantic migrations.

## Compatibility and migration

Diagnostic codes, messages, fixes, spans, and the `Self: Managed` fresh-return
exception are preserved. No Artifact, Provider, SDK, or runtime contract
changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic tests cover both resource generic fields and invalid fresh generic
returns. Architecture tests reject restoration of compiler-owned helpers.
