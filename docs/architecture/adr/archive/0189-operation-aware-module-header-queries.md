# ADR 0189: Make module-header queries operation-aware

- Status: Accepted
- Date: 2026-08-14

## Problem

`CompilationSession` already applies its shared cancellation/deadline contract
to parse, HIR, workspace graph, workspace HIR, type-fact, and diagnostic
queries. Its public source and interface module-header queries were the
remaining cached syntax facts without an operation-aware form, allowing an
editor client to return a cached header after its request had ended.

## Decision and non-goals

Add `module_header_with_operation` and
`interface_module_header_with_operation`. Both check the operation before and
after reading the existing revision-keyed cache.

This does not introduce a second module graph, change import resolution, or
move the remaining full compiler diagnostic analyzer into semantics. It closes
only the query-boundary inconsistency for single-file header consumers.

## Compatibility and migration

The unchecked convenience queries remain available for callers without an
operation boundary. Request-serving clients should use the new methods, as
they already do for workspace module-graph queries.

## Evidence

The semantic session test suite pre-populates both source and interface header
caches, verifies cancelled and expired requests fail before reading them, and
then proves a live request reuses the same cached `Arc` values.
