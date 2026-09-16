# ADR 0105: Semantic ownership of taken handle-field facts

- Status: Accepted
- Date: 2026-08-12

## Context

The compiler's local ownership module traversed every HIR statement and
expression form to find `take` operations applied to handle fields. The
resulting `TakeHandleField` facts already belonged to `rsscript-semantics`, but
the language traversal that produced them did not.

## Decision

`rsscript-semantics::take_handle_fields` now owns the complete nested HIR
traversal and de-duplicates its results in source traversal order. Compiler
local analysis caches and supplies the resulting facts to its existing
diagnostic path; it does not traverse HIR to reinterpret this ownership rule.

An architecture test prevents the old ownership-module collector functions
from returning.

## Consequences

This is behavior-preserving. It does not modify source syntax, artifact
encoding, Provider ABI, or VM execution; it establishes the semantic fact as a
reusable input for future MIR and verifier work.
