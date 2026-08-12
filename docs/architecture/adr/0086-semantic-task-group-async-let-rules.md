# ADR 0086: Semantic ownership of task-group async-let rules

## Status

Accepted.

## Problem

Compiler-owned AST collectors implemented the language rules for task-group
`async let` declarations: lexical ownership, direct await placement,
declaration order, and exactly-once consumption.

## Decision and non-goals

`rsscript-semantics::task_groups` now owns the complete source-AST rule and its
canonical diagnostics. Nested `task_group` and `select` bodies remain separate
structured-concurrency boundaries. Compiler only invokes this semantic query
while it traverses unsupported syntax. This ADR does not change lowering or VM
task scheduling.

## Compatibility and migration

The existing diagnostic code, labels, causes, fixes, and source spans are
preserved. No Artifact, Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover nested and repeated consumption cases. Architecture
tests require the semantic owner and reject the former compiler module.
