# ADR 0048: Semantic ownership of try-operand diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::try_operand_diagnostic` owns the `?` operand rule and its
`INVALID_TRY_OPERATOR` diagnostic. Compiler supplies the resolved operand type
projection and source span. Traversal for a function's `Result` error-type
compatibility remains a separate migration.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data changes.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes.

## Evidence

Semantic tests cover accepted `Result`/`Option` values and rejected scalars;
compiler and architecture tests reject the old compiler-local implementation.
