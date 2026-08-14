# ADR 0154: Artifact Bundles expose a typed analysis envelope

- Status: Accepted
- Date: 2026-08-14

## Problem

Bundle consumers previously inspected the `$schema` field of a raw JSON value
independently. This made the allowed analysis families an informal convention
despite being part of Artifact integrity and semantic-diff behavior.

## Decision

`rsscript-artifact` validates analysis payloads once into
`AnalysisEnvelopeV1`, whose `AnalysisSchemaV1` enum represents the supported
source and package analysis schemas. `ArtifactBundle::analysis()` remains the
compatibility JSON payload accessor; new consumers use `analysis_envelope()` to
select the validated schema before reading payload facts.

## Non-goals

This does not freeze every source/package analysis field or change Bundle v1
encoding. The JSON payload remains during the schema migration; typed per-schema
payload models will replace it incrementally.

## Compatibility and migration

Existing v1 Bundles and SDK `analysis()` callers retain the same serialized
payload. Unknown schemas remain fail-closed at Bundle loading. The new envelope
API is additive.

## Verifier, security, and backend impact

The envelope is an Artifact integrity boundary, not an execution or Provider
authority decision. VM and Provider linkage remain independent of analysis
payload interpretation; review and backend tools can rely on one validated
schema selection point.

## Evidence

Artifact round-trip tests assert the preserved source schema. SDK API snapshot,
schema, and CLI Artifact workflow tests cover the additive façade change.
