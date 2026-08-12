# ADR 0060: Semantic ownership of pattern field effect diagnostics

## Status

Accepted.

## Problem

Compiler pattern checking owned diagnostics for mutable/taking access to a
managed class field and child effects that exceed a match scrutinee's effect.
Both are ownership/effect semantics once the relevant facts are resolved.

## Decision and non-goals

`rsscript-semantics` owns `managed_pattern_field_effect_diagnostic` and
`weakened_pattern_field_effect_diagnostic`. Compiler retains field traversal,
effect-default calculation, and resolved managed-class facts. Duplicate field
and declared-field checks remain separate migration items.

## Compatibility and migration

Existing codes, spans, messages, causes, and fixes are unchanged. No Artifact,
Provider, SDK, or persisted-data compatibility changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Focused semantics tests cover managed and weakened field effects. Architecture
tests prevent compiler diagnostic ownership from returning.
