# ADR 0219: Route SDK snapshot compilation through `CompilationSession`

- Status: Accepted
- Date: 2026-08-14

## Decision

The reviewed in-memory SDK compiler captures normal immutable source/interface
snapshots in `CompilationSession`. Its check methods request the cached complete
analysis query; its compile methods request the phase-gated validated workspace
query before lowering to bytecode. The operation-aware forms use the
corresponding cancellation/deadline queries.

## Compatibility

The old in-memory surface permits an empty logical path. Session identities
intentionally reject empty paths, so that narrow legacy case remains on the
direct semantic entry point until the snapshot contract can reject it in a
separate compatibility change. All non-empty reviewed inputs now use the
single query boundary.
