# ADR 0036: Semantic ownership of protocol-bound diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::protocol_bound_diagnostics` owns unknown generic protocol
bound diagnostics across source and interface snapshots. Compiler protocol
checks consume the result and retain only implementation mapping and signature
comparison work.

## Compatibility and migration

Diagnostic code, message, fix, and source span are unchanged. No Artifact,
Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic tests cover unresolved bounds; architecture tests reject reintroducing
the compiler-owned helper.
