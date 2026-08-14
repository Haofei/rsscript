# ADR 0123: Explicit workspace snapshot capture

- Status: Accepted
- Date: 2026-08-14

## Context

Compiler purity requires filesystem capture to end before semantic compilation
begins. A loader API that implicitly resolves relative paths through the
process current directory makes embedding and reproducibility harder to reason
about.

## Decision

The workspace loader owns immutable WorkspaceSnapshot capture and provides
snapshot_from/load_from APIs that take an explicit base path. The historical
load API remains as a compatibility convenience while callers migrate.

## Consequences

New hosts can capture project input without ambient current-directory state.
The compiler can later accept snapshots without adding filesystem dependencies.
