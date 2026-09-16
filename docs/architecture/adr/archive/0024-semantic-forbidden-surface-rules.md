# ADR 0024: Semantic ownership of forbidden surface rules

## Status

Accepted

## Problem

The compiler owned three token-local language rules: rejecting `own struct`,
surface `&` references, and cast-style `as`. None required package loading,
lowering, or runtime knowledge, so equivalent frontend clients had to depend on
compiler orchestration.

## Decision and non-goals

`rsscript-semantics::forbidden_surface_syntax_diagnostics` now derives these
diagnostics from the canonical syntax token stream. It preserves the existing
exceptions for boolean/bitwise `&`, `with ... as`, and `use ... as`.

Operator overload and operand-type validation remains in compiler because it
currently depends on resolved HIR types. This ADR does not add or remove source
syntax.

## Compatibility and migration

The diagnostic codes, messages, fixes, and spans are unchanged. This is an
internal pre-1.0 migration with no Artifact, Provider ABI, SDK, or persisted
format impact.

## Verifier and security impact

None. These are compile-time source diagnostics only.

## Provider and backend impact

None. Providers and backends receive validated frontend results as before.

## Evidence

The semantic unit test covers all three rejections and valid boolean-and/import
aliases. Compiler regressions remain in place. The SDK architecture test
requires the semantics owner and rejects compiler-local copies of the rules.
