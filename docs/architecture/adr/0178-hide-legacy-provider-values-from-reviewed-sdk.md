# ADR 0178: Hide legacy Provider values from the reviewed SDK façade

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed `rsscript_sdk::provider_api` module re-exported both the canonical
wire Provider API and `NativeInterpreterFn`/`NativeValue`. New Provider authors
therefore encountered the legacy dynamic representation as an equally endorsed
SDK contract.

## Decision and non-goals

The reviewed Provider façade now exports `WireInterpreterFn` and `WireValue`,
but not the legacy native callable/value types. The root compatibility exports
remain behind the explicit SDK `compatibility` feature, and low-level adapters
may still depend on `rsscript-provider-api` directly during migration.

This does not yet remove `NativeValue` from VM compatibility adapters or the
legacy execution-report projection; those require the remaining structured
type-table and report-outcome work.

## Compatibility and migration

New Provider implementations use the reviewed wire API. Existing native
implementations can keep their current imports under `compatibility` or use the
low-level Provider crate explicitly. No interface descriptor, artifact, or
runtime linkage format changes.

## Architecture impact

The stable SDK surface now expresses the intended Provider direction without
pretending that all compatibility machinery has already been deleted.

## Evidence

The reviewed façade snapshot and inventory are updated deliberately; the SDK
architecture suite verifies the exact export set.
