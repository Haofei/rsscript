# ADR 0066: Semantic ownership of `fresh` and constructor diagnostics

## Status

Accepted.

## Problem

Compiler HIR traversal determined fresh temporary, weak-field, constructor-field,
managed-inline-field, and spawn-capture facts, but also contained the diagnostic
contracts for those ownership rules.

## Decision and non-goals

`rsscript-semantics` owns all five diagnostic constructors, including the
machine-applicable constructor field-effect edit. Compiler only derives resolved
field/effect/capture facts and attaches the resulting semantic diagnostics.
Read-view and explicit closure-capture rules remain separate work.

## Compatibility and migration

Codes, spans, labels, causes, fixes, and fix edits are preserved. No Artifact,
Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests construct every migrated diagnostic. Architecture tests
require compiler delegation and reject the former compiler text and constructors.
