# ADR 0061: Semantic ownership of duplicate pattern field diagnostics

## Status

Accepted.

## Problem

Compiler pattern traversal owned diagnostics for a repeated field projection and
for repeated mutable/taking projection of the same field. The compiler's local
map is a fact source, but the resulting constraints are language semantics.

## Decision and non-goals

`rsscript-semantics` owns `duplicate_pattern_field_diagnostic` and
`conflicting_pattern_field_effect_diagnostic`. Compiler keeps its ordered map
of preceding field effects/spans and delegates the diagnostic construction.
Unknown-field and omitted-field rules remain separate migration work.

## Compatibility and migration

Codes, spans, titles, causes, and manual fixes are preserved. No Artifact,
Provider, SDK, or persisted-data contract changes.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Semantics unit tests cover both diagnostics; architecture tests prohibit their
text from reappearing in compiler pattern checking.
