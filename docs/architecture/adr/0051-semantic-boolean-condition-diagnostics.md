# ADR 0051: Semantic ownership of Boolean condition diagnostics

## Status

Accepted.

## Problem

The compiler body checker owned the rule that `if` and `while` conditions must
have checked-HIR type `Bool`. This is a language semantic rule rather than flow
or compiler orchestration.

## Decision

`rsscript-semantics` owns `bool_condition_diagnostic`. It derives a known
expression type from checked HIR and emits the existing control-flow diagnostic
only when that type is not `Bool`. The compiler invokes it before continuing
with its flow-state handling.

## Compatibility

Diagnostic code, span, message, cause, and manual fix remain unchanged. Unknown
types are left to the resolver/type checker exactly as before.

## Security and verification

This change has no Provider, artifact, verifier, or runtime ABI effect.

## Evidence

Focused semantic tests cover rejected numeric and accepted Boolean conditions.
Architecture tests reject a duplicate compiler-owned condition rule.
