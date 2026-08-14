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
and number encoding. New writers always produce this exact form. This decision
does not turn arbitrary JSON into a typed analysis schema or replace the v1
three-section container with v2. Bundle identity also uses an explicit
`artifact_bundle.v1` hash domain and names each length-delimited section, so it
cannot be confused with a hash from another sectioned protocol.

## Compatibility and migration

The first deployed v1 writer used compact `serde_json::to_vec` sections before
this canonical form was specified. The v1 reader therefore accepts that exact
historical compact serialization in addition to the canonical form; it rejects
whitespace-padded or otherwise arbitrary JSON. A reader that accepts the
historical form re-emits the Bundle in canonical form. This preserves the
read-only v1 compatibility fixture while making every newly written Bundle
canonical. The section digests continue to bind the exact accepted bytes, so
legacy acceptance does not make a checksum cover a normalized-but-different
section.

## Verifier and security impact

The parser still bounds every section before decoding. Canonical recomputation
rejects arbitrary alternate encodings, while the narrow legacy compact path is
needed only for pre-canonical v1 Bundles. The digest remains an integrity
identity, not an origin signature.

## Provider and backend impact

Provider requirements and executable bytes are unchanged. All backends and the
runner consume the same normalized Bundle identity.

## Evidence

Artifact tests prove nested objects serialize identically across insertion
orders, whitespace-padded or arbitrary noncanonical sections are rejected, and
a historical compact manifest is accepted then normalized on write. The SDK
also executes a checked-in, read-only v1 Bundle fixture.
