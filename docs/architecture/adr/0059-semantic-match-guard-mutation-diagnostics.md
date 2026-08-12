# ADR 0059: Semantic ownership of `match` guard mutation diagnostics

## Status

Accepted.

## Problem

The compiler's match-effect traversal both found mutating `read`/`mut`/`take`
facts in guards and constructed the read-only guard diagnostic. The diagnostic
contract is language semantics.

## Decision and non-goals

`rsscript-semantics` owns `match_guard_mutation_diagnostic`. The compiler
temporarily supplies the first discovered `DataEffect` and span while the
checked-HIR effect traversal itself remains in compiler code.

## Compatibility and migration

The code, span, message, cause, and manual fix are unchanged. No Artifact,
Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

A focused semantic test covers a taking guard; an architecture test prevents
the compiler from restoring the canonical diagnostic text.
