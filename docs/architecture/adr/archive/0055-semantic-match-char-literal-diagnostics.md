# ADR 0055: Semantic ownership of `match` character literal diagnostics

## Status

Accepted.

## Problem

The compiler's match-pattern traversal separately counted Unicode scalars in a
`Char` pattern and emitted its own literal validity diagnostic. This duplicated
the language-level `Char` validity rule already owned by semantics.

## Decision and non-goals

`rsscript-semantics` owns `match_char_literal_scalar_diagnostic`. It shares the
canonical code, title, cause, and scalar-count behavior with ordinary `Char`
literals while retaining the existing pattern-specific manual fix. Pattern type
compatibility remains a separate compiler-to-semantics migration item.

## Compatibility and migration

The diagnostic code, span, message, cause, and pattern-specific fix remain
unchanged. This has no SDK, Artifact, Provider, or persisted-data effect.

## Verifier and security impact

Invalid character patterns remain rejected before lowering. No verifier or
runtime behavior changes.

## Provider and backend impact

None.

## Evidence

Focused semantics tests cover invalid multi-scalar and valid single-scalar
patterns. Architecture tests prohibit the compiler from restoring scalar-count
logic.
