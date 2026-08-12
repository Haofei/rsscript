# ADR 0070: Semantic ownership of return type diagnostics

## Status

Accepted.

## Problem

Compiler return checking resolves expected and actual types, including
`Result`/`Option` constructor payloads, but also owned the language diagnostics
for mismatches.

## Decision and non-goals

`rsscript-semantics` owns canonical whole-return and variant-payload mismatch
diagnostics. Compiler continues to resolve type aliases, traverse return forms,
and provide the already-resolved type facts and spans. Type compatibility
algorithms remain a separate migration concern.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover both diagnostics. Architecture tests require compiler
delegation and reject the former compiler return diagnostic text.
