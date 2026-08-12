# ADR 0106: Semantic ownership of fresh-return HIR projections

- Status: Accepted
- Date: 2026-08-12

## Context

Fresh-return diagnostics combine CFG-derived local state with several pure HIR
questions: whether a value field has a local base, whether an expression
contains a handle or weak field, and which source span should receive the
diagnostic. Those questions lived alongside compiler state propagation despite
being language-wide expression rules.

## Decision

`rsscript-semantics` owns `fresh_field_access_base`,
`fresh_handle_or_weak_field_path`, and `fresh_return_value_span`. It also owns
the private wrapper and display-path rules required to derive those values.
Compiler local ownership analysis supplies only the CFG state used to decide
whether an otherwise valid local is clean and fresh-returnable.

## Consequences

The migration preserves diagnostics and their user-facing span selection while
making the expression contract reusable by future semantic and MIR consumers.
It has no source, artifact, Provider, or runtime compatibility impact.
