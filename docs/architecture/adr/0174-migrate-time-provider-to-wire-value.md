# ADR 0174: Migrate the time Provider to canonical WireValue

- Status: Accepted
- Date: 2026-08-14

## Problem

The Provider API had a canonical typed `WireValue` model, but all official
Providers still implemented the legacy `NativeValue` callable. That left the
new model unproven outside VM adapter tests and made the conformance kit itself
an obstacle to provider migration.

## Decision and non-goals

The scalar `rsscript-provider-time` implementation now exposes
`WireInterpreterFn` and returns `WireValue::Int`. The conformance kit adds an
equivalent wire-callable path that verifies descriptor registration plus the
runtime-owned cancellation and deadline gates.

This is intentionally a scalar migration. It does not claim that lists,
records, variants, resources, JSON, or asynchronous providers can be converted
without the linked Artifact type-table adapter.

## Compatibility and migration

`ProviderRegistry` and the reviewed SDK already accept both native and wire
callables. Existing Providers retain their `NativeInterpreterFn` API. New
scalar synchronous Providers can choose `WireInterpreterFn`; providers with
structured values remain on the explicit compatibility adapter until their
typed layouts are available at link time.

## Security and architecture impact

The wire conformance path invokes the same provider context checks before
provider code. It does not weaken signature validation, cancellation,
deadlines, or payload limits, and it does not fabricate numeric identities for
structured values.

## Evidence

Provider conformance tests cover native and wire preflight paths. The time
Provider tests prove its descriptor conforms and that a direct call returns a
canonical wire integer. Targeted tests and clippy run for both crates.
