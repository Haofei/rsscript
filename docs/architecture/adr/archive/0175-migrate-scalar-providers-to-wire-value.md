# ADR 0175: Migrate scalar Providers to canonical WireValue

- Status: Accepted
- Date: 2026-08-14

## Problem

After the first `time` Provider migration, other official Providers with only
scalar interface values still exposed `NativeInterpreterFn`. Maintaining that
legacy boundary where no structural type information is needed delays the
canonical wire-model cutover without adding compatibility value.

## Decision and non-goals

The `entropy` and `log` Providers now use `WireInterpreterFn`:

- entropy accepts `WireValue::Int` and returns `WireValue::Bytes`;
- log accepts `WireValue::String` and returns `WireValue::Unit`.

The existing Provider descriptors, symbols, and signatures are unchanged.
This decision deliberately excludes CLI arguments, environment results,
filesystem data, HTTP/process records, and resource values, because they need
linked aggregate layouts or asynchronous wire dispatch.

## Compatibility and migration

The reviewed registry accepts both legacy native and canonical wire callables,
so existing embedding hosts continue to register either form. The reference VM
only dispatches the wire form for already-linked scalar signatures; all
structured signatures remain fail-closed.

## Security and architecture impact

Calls still pass through descriptor preflight, cancellation/deadline checks,
payload accounting, tracing, and Provider error mapping. This migration does
not add ambient authority or a new provider capability.

## Evidence

The existing conformance tests now exercise both migrated Providers through the
wire callable path. Targeted crate tests and clippy pass.
