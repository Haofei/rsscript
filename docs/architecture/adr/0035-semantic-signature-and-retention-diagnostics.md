# ADR 0035: Semantic ownership of signature and retention diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::signature_diagnostics` owns source signature explicitness,
method/protocol `self` shape, and declaration-level `retains` validation. This
includes Copy and `noescape` retention restrictions. Compiler declaration
orchestration only appends the semantic diagnostics.

Call-site effects, argument binding, and retention escape analysis remain
separate migrations.

## Compatibility and migration

Diagnostic codes, messages, fixes, spans, and ordering within each function are
preserved. No Artifact, Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantic tests cover missing return types and invalid Copy retention.
Architecture tests reject restoration of compiler-owned signature helpers.
