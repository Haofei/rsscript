# ADR 0192: Centralize the transitional session diagnostic adapter

- Status: Accepted
- Date: 2026-08-14

## Problem

The language service correctly owned immutable workspace snapshots and the
semantic diagnostic cache, but the LSP and language-service tests each rebuilt
source/interface lists and independently replayed the compiler's transitional
analysis sequence. That duplicated composition logic could drift in interface
visibility, cancellation polling, or diagnostics ordering.

## Decision

`rsscript-compiler` exposes one explicit transitional adapter,
`analyze_frontend_input_snapshot_with_operation`. It consumes the immutable
`FrontendInputSnapshot` captured by `CompilationSession` and performs the
historical compiler analysis sequence in one location. LSP and test
composition roots forward the snapshot unchanged through this adapter.

The adapter is not the final semantic query implementation: it remains a
compatibility boundary until complete diagnostic orchestration lives in
`rsscript-semantics`. It introduces no compiler dependency into
`rsscript-language-service`.

## Consequences

There is one audited compiler bridge to replace during semantic-query
migration. Architecture tests reject LSP reconstruction of snapshot roles or
direct calls to the lower-level compiler analysis entry points.
