# ADR 0053: Semantic ownership of `match` arm result diagnostics

## Status

Accepted.

## Problem

The compiler body checker owned the rule that a `match` expression's
value-producing arms must agree with its resolved result type. This rule uses
only checked HIR and does not depend on compiler flow orchestration.

## Decision

`rsscript-semantics` owns `match_expression_arm_type_diagnostics`. The compiler
passes the resolved match expression type and appends the returned diagnostics.

## Compatibility

Existing diagnostic code, span, text, cause, and manual fix are preserved.
Arms without a value-producing final statement continue to be ignored by this
rule.

## Security and verification

No Provider, artifact, verifier, or runtime contract changes.

## Evidence

A semantic test covers a mismatched `match` arm. Architecture tests prohibit
reintroducing the compiler-owned traversal.
