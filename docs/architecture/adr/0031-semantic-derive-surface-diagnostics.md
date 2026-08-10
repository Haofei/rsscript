# ADR 0031: Semantic ownership of derive-surface diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::derive_syntax_diagnostics` is the only owner of the
language derive catalog and the rule that resources cannot receive value
derives.  Compiler syntax traversal supplies declaration data and appends the
result; it no longer maintains a second derive catalog or resource restriction.

Per-field implementation support remains a separate semantic migration item.
This ADR moves only source-language derive names and the move-only resource
contract, with no change to accepted derives or diagnostics.

## Compatibility and migration

Diagnostic codes, messages, fixes, ordering, and spans are preserved. No
Artifact, Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic unit tests cover unknown and resource-incompatible derives in source
order. Architecture tests forbid restoration of the compiler-owned catalog.
