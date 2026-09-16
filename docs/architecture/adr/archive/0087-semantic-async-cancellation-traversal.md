# ADR 0087: Semantic ownership of async cancellation-token traversal

## Status

Accepted.

## Problem

Although the cancellation-token diagnostic contract had moved to semantics,
compiler still implemented the source-AST traversal that determines lexical
task-group ownership.

## Decision and non-goals

`rsscript-semantics::await_placement` owns the complete async-function source
query. It stops at nested task-group boundaries and emits the canonical
diagnostic only for calls with no lexical cancellation owner. Compiler merely
iterates source functions and appends the semantic result. This does not alter
task scheduling or runtime cancellation mechanics.

## Compatibility and migration

Diagnostic code, span, cause, and fix are preserved. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover the task-group boundary. Architecture tests reject the
former compiler traversal helpers.
