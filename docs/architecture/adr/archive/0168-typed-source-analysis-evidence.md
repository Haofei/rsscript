# ADR 0168: Type direct source-build analysis evidence at the Artifact boundary

- Status: Accepted
- Date: 2026-08-14

## Problem

`ArtifactBundle` owns the persisted evidence contract, but direct SDK builds
previously constructed `rsscript.source_analysis.v1` as arbitrary
`serde_json::Value`. That made a stable source-analysis section depend on
callers remembering a discriminator and its required fields, rather than on an
Artifact-owned contract.

## Decision and non-goals

`rsscript-artifact` now owns `SourceAnalysisV1` and the
`AnalysisEnvelopeV1::source` constructor. Direct source/interface compilation
uses that typed constructor before a Bundle is created. Bundle readers decode a
source-analysis section through the same type and reject unknown or malformed
fields before accepting its canonical encoding.

The source list preserves the complete compiler input listing rather than
silently deduplicating it. Input normalization and duplicate-path rejection are
loader/compiler responsibilities; evidence must not rewrite the input it
describes.

Package analysis remains a JSON-shaped migration adapter behind the known
`Package` schema. This ADR does not claim that package analysis is fully typed,
does not change the Bundle v1 container, and does not add policy or authority
semantics to analysis evidence.

Artifact and SDK readers expose `source_analysis()` only when the envelope is
the source schema. This gives consumers typed access without conflating it with
the distinct package-analysis compatibility schema.

## Compatibility and migration

Bundle v1 bytes remain compatible. Existing source-analysis readers continue
to receive the same JSON payload, while writers now produce it only through
the typed model. Historical compact JSON encodings remain accepted as defined
by the v1 reader. Package-analysis compatibility producers explicitly convert
through `AnalysisEnvelopeV1::from_json` at the Artifact boundary.

`SourceAnalysisV1` is added to the reviewed SDK Artifact façade; the checked
public API inventory and snapshot record that deliberate surface change.

## Verifier and security impact

The change reduces the accepted shape of untrusted source-analysis evidence:
unknown source-analysis fields, missing required fields, and a wrong schema
are rejected before digest and canonical-encoding checks complete. It does not
change bytecode verification, Provider authorization, or process isolation.

## Provider and backend impact

Providers and execution backends do not consume source analysis while linking
or executing, so no ABI changes. Package/review adapters continue to use their
separate known schema until their typed schema migration is complete.

## Evidence

`rsscript-artifact` round-trip and malformed/noncanonical Bundle tests cover
typed source evidence. SDK direct and captured-project build tests prove the
compiler route constructs a Bundle from the Artifact-owned envelope. The SDK
public API architecture test checks the explicit façade export snapshot.
