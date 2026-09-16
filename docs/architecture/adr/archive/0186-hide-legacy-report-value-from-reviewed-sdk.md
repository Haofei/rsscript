# ADR 0186: Hide legacy report values from the reviewed SDK

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed `ExecutionReport` exposed `Option<NativeValue>` even after the
reviewed Provider façade had moved new embedders to `WireValue`. This made the
legacy dynamic value representation part of the default Rust embedding API and
allowed new callers to depend on it accidentally.

## Decision and non-goals

The default/execution SDK builds retain the existing v1 JSON field privately
for machine-consumer compatibility, but no longer expose it as a public Rust
field. The explicit `compatibility` feature continues to expose the legacy
field for migration callers. New reviewed callers use the stable textual
result, telemetry, diagnostics, and termination evidence.

This is an API containment step, not the final report-value design. A future
versioned report outcome will carry a canonical typed result and permit removal
of the private v1 projection.

## Compatibility and migration

`rsscript.execution_report.v1` is unchanged, so checked-in fixtures and
existing JSON consumers continue to work. Rust callers that require
`NativeValue` must opt into `rsscript-sdk/compatibility` during the migration
window. No Provider descriptor, artifact, or VM linkage format changes.

## Verifier and security impact

No verification or execution authority changes. Keeping the v1 projection
private prevents a legacy dynamic value from becoming a new stable embedding
dependency while preserving existing report evidence.

## Provider and backend impact

Provider and VM compatibility adapters remain unchanged. The follow-up typed
report outcome must consume the canonical wire model rather than expose another
dynamic adapter.

## Evidence

- default/execution SDK serialization against the strict v1 schema
- SDK architecture guard for the compatibility-only public field
- reviewed SDK API inventory update
