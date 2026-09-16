# ADR 0103: Semantic ownership of retained-closure argument resolution

## Status

Accepted.

## Decision

`rsscript-semantics` owns HIR resolution of a closure payload delivered to a
retaining parameter, including `read` and `Ok`/`Err`/`Some` wrapper forms.
Compiler CFG and retained-capture diagnostics consume this query rather than
reimplementing wrapper semantics.

## Compatibility and impact

The existing closure-wrapper behavior is retained. No source, Artifact,
Provider, SDK, verifier, or persisted-data contract changes.
