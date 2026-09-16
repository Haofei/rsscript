# ADR 0056: Semantic ownership of literal `match` pattern type diagnostics

## Status

Accepted.

## Problem

The compiler's recursive match-pattern traversal both identified literal
patterns and owned their compatibility diagnostic. The compatibility rule is
language semantics once the scrutinee type has been resolved.

## Decision and non-goals

`rsscript-semantics` owns `match_literal_type_diagnostic`, including literal
type identity and canonical diagnostic text. Compiler pattern traversal still
supplies the alias-expanded scrutinee type and recurses through nested patterns.

## Compatibility and migration

The diagnostic code, span, title, message, and cause remain unchanged. No
Artifact, Provider, SDK, or persisted-data contract changes.

## Verifier and security impact

None; invalid literal patterns continue to fail frontend validation before
lowering.

## Provider and backend impact

None.

## Evidence

Focused semantics tests cover incompatible and compatible literal patterns.
Architecture tests prevent diagnostic ownership from returning to compiler code.
