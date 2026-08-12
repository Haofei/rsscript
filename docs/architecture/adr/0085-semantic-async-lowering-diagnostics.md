# ADR 0085: Semantic ownership of async-lowering diagnostics

## Status

Accepted.

## Problem

Compiler source traversal detects nested await expressions that cannot be
lowered by the current structured-async model, plus cancellation-token requests
without a lexically owning task group, and previously constructed the language
diagnostics itself.

## Decision and non-goals

`rsscript-semantics::await_placement` owns both canonical diagnostic contracts.
Compiler retains source traversal and contributes only the invalid async facts.
This ADR does not yet move those syntax traversals into semantics.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover both contracts. Architecture tests require compiler
delegation and reject compiler construction of these diagnostics.
