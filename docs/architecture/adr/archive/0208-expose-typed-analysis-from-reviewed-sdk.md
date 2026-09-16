# ADR 0208: Expose typed Artifact analysis from the reviewed SDK

- Status: Accepted
- Date: 2026-08-14

Artifact analysis is a versioned contract, not an untyped side channel. The
reviewed `BuiltArtifact` API therefore exposes `AnalysisEnvelopeV1` plus typed
source/package analysis accessors. It no longer exposes a raw
`serde_json::Value` projection by default.

The raw projection remains hidden behind the explicit SDK compatibility feature
for legacy consumers, and the CLI serializes the envelope payload only at its
application-output boundary. This does not alter Bundle bytes or schemas; it
prevents new embeddings from bypassing schema selection and typed validation.
