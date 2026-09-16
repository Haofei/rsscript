# ADR 0184: Poll operations during module-graph construction

- Status: Accepted
- Date: 2026-08-14

## Problem

The operation-aware workspace module-graph query checked cancellation and
deadline only around the complete cold build. Parsing headers for a large
source/interface set could continue after an editor or CLI request had ended.

## Decision and non-goals

The shared `CompilationSession` module-graph builder now polls before every
source and interface header query and before cache publication. Cache hits keep
their pre/post operation checks. The ordinary unchecked API shares this builder
without an operation context.

This is query-boundary work only; it does not change import resolution rules or
complete the remaining semantic diagnostic migration.

## Compatibility and migration

No source, Artifact, Provider, or runtime contract changes. Aborted cold graph
queries now stop earlier and cannot populate the session cache.

## Evidence

- `rsscript-semantics` cache and operation tests
- `rsscript-language-service` shared-module-graph tests
