# ADR 0126: Artifact Bundle schema is independent of the SDK

- Status: Accepted
- Date: 2026-08-14

## Problem

`ArtifactBundle` is a persisted, provider-neutral product contract, but its
wire format and integrity checks lived inside `rsscript-sdk`. Inspection,
runner, and review consumers therefore needed the embedding facade merely to
read a bundle.

## Decision and non-goals

`rsscript-artifact` owns the Bundle v1 envelope, provenance, interface
requirements, canonical analysis encoding, integrity validation, and complete
external import contracts. The SDK re-exports these contract types and retains
only build/verify/link/run phase composition. This does not redesign Bundle
v1's JSON sections; typed analysis and section-table v2 remain follow-up work.

## Compatibility and migration

The bundle bytes, magic, schema identifiers, digest calculation, validation
limits, and SDK names remain unchanged. Consumers can migrate directly to
`rsscript-artifact` without depending on the SDK.

## Verifier and security impact

The same fail-closed bundle checks now run in an execution-independent crate.
The bundle digest remains an integrity identity, not an origin signature.

## Provider and backend impact

Provider imports are preserved as exact Artifact facts. VM, AOT, JIT and runner
implementations can consume the bundle contract without depending on the SDK.

## Evidence

Artifact crate tests cover round-trip identity and tamper rejection. SDK
architecture tests require its stable façade to source the Artifact type from
the independent crate; existing SDK/CLI/runner verification tests retain the
same bytes and phase APIs.
