# ADR 0220: Complete the reviewed Provider registration surface

- Status: Accepted
- Date: 2026-08-15

## Problem

The reviewed `rsscript_sdk::provider_api` module exposed canonical Wire
callables and descriptors, but not all public types needed to construct a
descriptor. A normal Provider author had to bypass the reviewed SDK façade and
import the low-level ABI crate merely to specify parameter effects, runtime ABI
support, error mapping, or resource cleanup behavior.

## Decision and non-goals

The reviewed façade additionally exports `DataEffect`, `ParameterSignature`,
`ProviderErrorMapping`, `ResourceCleanupContract`, and `RUNTIME_ABI_VERSION`.
Together with the existing descriptor and `WireValue` exports, this is the
complete contract needed to register a canonical sync or async Wire Provider.
`ProviderResource` is also part of this boundary so a canonical Provider can
register run-owned cleanup with `ProviderCallContext` without importing a
low-level implementation crate.

This does not expose `NativeValue`, `NativeInterpreterFn`, raw VM conversion
helpers, or a JSON escape hatch. Those stay behind explicit compatibility
boundaries until the VM migration is complete.

## Compatibility and migration

This is an additive pre-1.0 SDK API change. Existing Provider implementations
need no changes. New implementations can now depend only on the reviewed SDK
façade rather than importing an implementation-layer crate.

## Verifier and Provider impact

The descriptor remains the source of signature, call-mode, ABI, cleanup, and
error-mapping validation. No Artifact, bytecode, or Provider wire schema
changes. The compatibility corpus exercises the façade by proving a signature
mismatch fails before a callable runs and two compatible Wire Providers link
the same verified Artifact.
