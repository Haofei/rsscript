# ADR 0067: Semantic ownership of read-view and closure-contract diagnostics

## Status

Accepted.

## Problem

Compiler body passes detect local read views, closure captures, and noescape
place accesses, but also owned the language diagnostics for exclusive use and
capture contracts.

## Decision and non-goals

`rsscript-semantics` owns read-view mutation, noescape consuming capture, and
explicit closure capture diagnostics. Compiler continues to collect scoped HIR
and local-flow facts, resolve data effects, and delegate those facts. Place
alias-conflict diagnostics remain separate work.

## Compatibility and migration

Codes, spans, labels, causes, and fixes are unchanged. No Artifact, Provider,
SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests instantiate each diagnostic. Architecture tests require
delegation and prevent compiler diagnostic constructors from returning.
