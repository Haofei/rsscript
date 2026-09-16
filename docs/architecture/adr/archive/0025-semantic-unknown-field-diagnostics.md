# ADR 0025: Semantic ownership of unknown-field diagnostics

## Status

Accepted

## Problem

The compiler traversed resolved HIR field-access facts and rebuilt unknown-field
diagnostics, despite all identity and type information already belonging to
`rsscript-semantics`.

## Decision and non-goals

`rsscript-semantics::unknown_field_diagnostics` derives unresolved field facts
from resolved HIR function bodies and type metadata. Compiler aggregates the
result only. Unknown type names, lexical bindings, and generic constraints are
not included in this narrow move.

## Compatibility and migration

Diagnostic code, span, message, cause, and fix remain unchanged. No language,
Artifact, Provider, SDK, or runtime contract changes.

## Verifier and security impact

None; this is a pre-lowering diagnostic ownership move.

## Provider and backend impact

None.

## Evidence

Existing compiler HIR-field regression coverage remains green. Architecture
tests require the semantic query and reject the old compiler-local diagnostic
call.
