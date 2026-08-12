# ADR 0076: Compiler semantic-diagnostic ownership guard

## Status

Accepted.

## Problem

Semantic diagnostic ownership had been migrated incrementally, but no
mechanical check prevented a future compiler check from recreating language
diagnostic construction.

## Decision and non-goals

Architecture tests scan every compiler check module and reject direct
`Diagnostic::` construction except for two documented non-language boundaries:
frontend work-budget termination and Rust AOT `#lower_name` validation. Language
diagnostics must be constructed by `rsscript-semantics`. This guard does not
claim that compiler fact extraction, HIR adaptation, local flow, or backend
validation have disappeared.

## Compatibility and migration

No user-visible diagnostic contract changes. New compiler diagnostic ownership
requires an explicit architecture decision and test update.

## Verifier and security impact

The work-budget exception remains fail-closed; it cannot be reclassified as a
successful semantic analysis.

## Provider and backend impact

The `#lower_name` exception is scoped to Rust AOT symbol generation and does
not affect Provider-neutral compilation.

## Evidence

The architecture test asserts the exact allowlist and requires the boundary
comments that justify each exception.
