# ADR 0064: Semantic ownership of resource-boundary diagnostics

## Status

Accepted.

## Problem

Compiler body checks derived resource and local-flow facts but also owned the
diagnostic wording for resource escape, managed closure capture, transient
resource producers, `with` result unwrapping, class-local bindings, and invalid
`manage`/`take` operands. These are language-level ownership and resource
contracts, not compiler orchestration policy.

## Decision and non-goals

`rsscript-semantics` owns the canonical diagnostic constructors for those
contracts. Compiler retains checked-HIR traversal, local-flow extraction, and
resolved-type adaptation, then passes facts and spans to semantics. Fresh-return
and explicit closure-capture contracts remain separate migration work.

## Compatibility and migration

Diagnostic codes, spans, labels, causes, and fixes are preserved. No Artifact,
Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover each new diagnostic constructor. Architecture tests
require compiler delegation and reject the removed compiler constructors.
