# ADR 0029: Semantic ownership of builtin value identities

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::is_builtin_value_ident` is the only owner of source value
identities such as `Unit`, `None`, `null`, and booleans. Compiler unresolved
binding checks consume that query. No language values are added or removed.

## Compatibility and migration

No Artifact, Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Compiler and semantics test suites preserve binding behavior.
