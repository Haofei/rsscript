# ADR 0001: Typed provider wire values

## Status

Accepted

## Problem

The legacy `NativeValue` compatibility representation permits free-form type
and field-name strings, JSON, maps, and `Native { type_name, id }`. It is useful
at trusted adapter boundaries but cannot be the canonical cross-provider ABI:
its identity is ambiguous and it permits stale resource handles to be encoded
without a typed resource contract.

## Decision and non-goals

`rsscript-abi-model` owns `WireValue`, numeric type/field/variant/resource IDs,
and `WireResourceHandle { resource_type, slot, generation }`. Canonical records
and variants are positional and carry no free-form executable identity. Payload
accounting is iterative and saturating.

This ADR does not remove `NativeValue`, add JSON to the canonical ABI, or change
the language type system. Generated Provider adapters remain the only intended
location for legacy-value conversion during migration.

## Compatibility and migration

The existing Provider ABI remains version 2 while generated adapters and
official Providers migrate incrementally. Resource wrappers pass their
descriptor-supplied `WireResourceTypeId` when converting a runtime handle; they
must not infer it from a legacy type-name. A future Provider ABI bump will make
the typed model the mandatory call payload and retain a read-only compatibility
adapter for older Providers.

## Verifier and security impact

Typed wire values eliminate string-derived resource identity at the canonical
boundary. Slot plus generation continues to reject stale handles. Iterative
payload accounting avoids recursion depth as an input-controlled cost before a
host applies request or response limits.

## Provider and backend impact

Provider bindgen and the runtime resource table can now exchange typed resource
handles. VM, AOT, JIT, and review integrations are unaffected until the
compatibility adapter migrates aggregate values.

## Evidence

`rsscript-abi-model` tests cover numeric identities, absence of type-name
fields, resource payload accounting, and deep nesting. `rsscript-provider-api`
tests cover runtime-handle/wire-handle round trips. Core CI enforces this ADR
directory for future contract changes.
