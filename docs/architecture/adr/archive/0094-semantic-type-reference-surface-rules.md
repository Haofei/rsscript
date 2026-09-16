# ADR 0094: Semantic ownership of type-reference surface rules

## Status

Accepted.

## Decision

`rsscript-semantics` owns recursive callback qualifier placement and malformed
type-argument diagnostics. Compiler may canonicalize aliases first, then passes
the resulting `TypeRef` and its source-position allowance facts to semantics.

## Compatibility and impact

Diagnostic code, span, label, cause, and fix are unchanged. No Artifact,
Provider, SDK, verifier, or persisted-data compatibility changes.

## Evidence

Semantics tests cover rejected `noescape Fn` return positions. Architecture
tests reject the former compiler recursive diagnostic traversal.
