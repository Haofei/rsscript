# ADR 0130: Artifact Bundle JSON has an explicit canonical form

- Status: Accepted
- Date: 2026-08-14

## Problem

Bundle v1 calls its manifest and analysis JSON canonical, but a direct
`serde_json::to_vec` depends on the active `Map` implementation. In particular,
the workspace can enable `preserve_order`, making two semantically equal JSON
objects produce different bytes and therefore different Bundle identities.

## Decision and non-goals

Bundle v1 serializes every JSON object recursively with keys ordered by Unicode
scalar value, compact JSON punctuation, and `serde_json`'s valid JSON string
and number encoding. The decoder recomputes this exact form and rejects a
noncanonical manifest or analysis section. This decision does not turn
arbitrary JSON into a typed analysis schema or replace the v1 three-section
container with v2.

## Compatibility and migration

Existing valid Bundle v1 producers whose object insertion order was already
sorted remain byte-compatible. Producers with a different ordering must rebuild
the Bundle. This is intentional: Bundle digests are reproducibility evidence,
so accepting multiple byte representations for the same analysis would weaken
the contract.

## Verifier and security impact

The parser still bounds every section before decoding. Canonical recomputation
makes the analysis digest unambiguous and rejects alternate encodings before
the bundle is admitted to bytecode verification. The digest remains an
integrity identity, not an origin signature.

## Provider and backend impact

Provider requirements and executable bytes are unchanged. All backends and the
runner consume the same normalized Bundle identity.

## Evidence

Artifact tests prove nested objects serialize identically across insertion
orders and that syntactically valid but noncanonical manifest and analysis
sections are rejected.
