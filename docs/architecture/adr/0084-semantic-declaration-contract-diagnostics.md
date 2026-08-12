# ADR 0084: Semantic ownership of declaration-contract diagnostics

## Status

Accepted.

## Problem

Compiler declaration checks resolved protocol implementation mappings but also
constructed diagnostics for unresolved type names and contract mismatches.

## Decision and non-goals

`rsscript-semantics` owns the canonical unresolved-type and protocol
implementation mismatch diagnostic contracts. Compiler retains declaration
traversal and resolved mapping facts. This does not yet move protocol mapping
resolution itself into semantics.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover both diagnostic contracts. Architecture tests require
their explicit semantic exports and compiler delegation.
