# ADR 0079: Semantic ownership of bounded generic substitution

## Status

Accepted.

## Problem

The compiler owned recursive rendered-type generic substitution, even though
the algorithm is language semantics. It also needed to participate in the
shared frontend cancellation and substitution budget.

## Decision and non-goals

`rsscript-semantics` owns the substitution algorithm and exposes the narrow
`SubstitutionBudget` trait. Compiler adapts its existing work budget to that
trait, preserving a single bounded operation budget without introducing a
semantics-to-compiler dependency. This ADR does not replace rendered legacy HIR
types with structural `TypeId` facts.

## Compatibility and migration

Substitution outputs and budget exhaustion behavior are unchanged. No Artifact,
Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

The substitution remains fail-closed when the shared recursion or operation
budget is exhausted.

## Provider and backend impact

None.

## Evidence

Semantics tests cover substitution output; compiler tests retain budget
exhaustion coverage. Architecture tests require the budget adapter and reject
the former compiler recursive helper.
