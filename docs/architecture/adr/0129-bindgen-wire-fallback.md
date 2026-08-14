# ADR 0129: Bindgen defaults unresolved named types to WireValue

- Status: Accepted
- Date: 2026-08-14

## Problem

Provider bindgen converted unresolved named interface types to `NativeValue`.
That made the legacy dynamic adapter the default for newly generated Provider
code, even where the ABI already has a canonical structural `WireValue`.

## Decision and non-goals

Generated trait signatures now use `rsscript_abi_model::WireValue` for named,
non-resource, and non-handle types that lack a generated resource wrapper.
Resource wrappers retain their explicit wire-handle conversions. Generated
mocks remain legacy registry adapters until the callable registry is migrated.

## Compatibility and migration

This affects newly generated source only. Existing generated Providers keep
compiling through NativeValue compatibility APIs; regenerating adopts the wire
type intentionally.

## Verifier and security impact

The generated default no longer carries arbitrary type-name or JSON escape
values into new Provider signatures.

## Provider and backend impact

Provider implementers gain a canonical ABI fallback. VM dispatch remains on the
legacy adapter until its registry migration is complete.

## Evidence

Bindgen unit tests and the architecture test continue to verify that bindgen
consumes semantic interface descriptors rather than syntax directly.
