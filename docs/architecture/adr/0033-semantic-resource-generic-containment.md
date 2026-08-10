# ADR 0033: Semantic ownership of resource generic containment

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::resource_generic_diagnostics` owns resource containment
validation in declaration type positions and explicit generic call namespaces.
It retains the language exception that only a direct function return of
`Result<Resource, E>` may carry a resource generic argument. Compiler now only
orchestrates the query.

This ADR does not move generic bounds whose semantics depend on the broader
signature and ownership passes.

## Compatibility and migration

Diagnostic codes, messages, fixes, spans, and the direct-result exception are
preserved. No Artifact, Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic tests cover rejected containment and the valid direct result return.
Architecture tests reject restoration of the compiler recursive traversal.
