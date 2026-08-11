# ADR 0041: Semantic ownership of call argument diagnostics

## Status

Accepted

## Problem

The compiler call checker owned source argument naming, duplicate/unknown and
missing argument diagnostics, and call-site `read`/`mut`/`take` effect checks.
Those rules operate on already-resolved parameter facts and do not require
compiler orchestration or backend knowledge.

## Decision and non-goals

`rsscript-semantics::call_argument_diagnostics` owns the canonical diagnostics
and their wording, causes, and fixes. The compiler resolves a callee and its
argument-to-parameter mapping, then supplies small syntax-independent facts to
the semantic query. Receiver parameters remain in the canonical signature but
are marked as implicitly supplied, so receiver-call shorthand cannot create a
spurious missing-`self` diagnostic.

This ADR does not migrate external signature matching, type compatibility,
generic substitution, retention, escape, or provider linkage rules.

## Compatibility and migration

No language, Artifact, Provider, SDK, runtime, or persisted-data contract
changes. Existing diagnostic codes, wording, causes, and fixes are preserved.

## Verifier and security impact

No verifier, budget, resource, cancellation, or isolation changes.

## Provider and backend impact

No Provider or backend contract changes. Backends continue to consume the
shared call binding separately from these diagnostics.

## Evidence

Focused semantic tests cover argument shape, receiver slots, completeness, and
effect facts. Compiler/semantics regressions preserve orchestration, and an
architecture test rejects compiler-local copies of these rules.
