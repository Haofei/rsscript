# ADR 0032: Semantic ownership of resource declaration boundaries

## Status

Accepted

## Decision and non-goals

`rsscript-semantics` owns diagnostics for raw `Fd` exposure, resources stored
in non-resource declarations, and weak fields that do not point to a class.
These are source and resolved-HIR semantic facts with no compiler, Provider, or
backend dependency. Compiler passes only append the query results in their
established diagnostic phases.

Generic resource containment in signatures and calls remains a separate
migration because it traverses additional source call forms and generic-context
state.

## Compatibility and migration

Diagnostic codes, messages, fixes, and spans are preserved. No Artifact,
Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic unit tests cover descriptor exposure, resource fields, and weak fields.
Architecture tests reject restoration of the compiler-owned implementations.
