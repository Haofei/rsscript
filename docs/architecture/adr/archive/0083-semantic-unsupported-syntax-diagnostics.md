# ADR 0083: Semantic ownership of unsupported-syntax diagnostics

## Status

Accepted.

## Problem

The compiler's transitional syntax/HIR adapter detects source forms that
RSScript deliberately does not support, but also constructed the resulting
language diagnostic.

## Decision and non-goals

`rsscript-semantics::unsupported_syntax_diagnostic` owns the canonical
diagnostic contract. Compiler retains source-form discovery and appends that
semantic diagnostic. This does not yet move every source traversal into the
semantics crate.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics tests cover the canonical contract. Architecture tests prohibit
compiler check modules from constructing language diagnostics directly.
