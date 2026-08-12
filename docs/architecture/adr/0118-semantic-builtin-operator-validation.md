# ADR 0118: Semantic ownership of builtin operator validation

- Status: Accepted
- Date: 2026-08-12

## Context

Builtin operator validity depends on resolved expression types, type-alias
normalization, and recursive checked-HIR traversal. The compiler previously
owned that traversal and independently classified numeric operands, despite
rsscript-semantics already owning the operator diagnostic contract.

## Decision

rsscript-semantics::builtin_operator_diagnostics derives all builtin operator
diagnostics from Hir. It owns traversal through statements, closures,
collections, and match arms, plus alias normalization and operand
classification.

The compiler adapter invokes that query after source-surface checks and does
not reconstruct operator facts or diagnostic rules.

## Consequences

There is one interpretation of numeric, boolean, comparison, bitwise, and
arithmetic operator operands. The change affects neither source syntax nor
artifact/runtime/provider contracts.
