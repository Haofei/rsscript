# ADR 0109: Semantic ownership of the HIR type-name projection

- Status: Accepted
- Date: 2026-08-12

## Decision

`rsscript-semantics::hir_expr_type_name` owns the legacy rendered type
projection for resolved HIR expressions. Compiler local CFG consumers use it
for temporary flow metadata rather than maintaining a second expression match.

## Consequences

This is transitional only: structural type IDs remain the target interface. It
removes duplicate HIR interpretation without changing source, artifact,
Provider, or runtime behavior.
