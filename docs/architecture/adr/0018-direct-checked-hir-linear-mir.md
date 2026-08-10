# ADR 0018: direct checked-HIR MIR lowering

## Status

Accepted

## Problem

The migration path still lowered checked HIR to source-shaped `ExecutableIr`
before creating MIR. That retained an unnecessary compatibility representation
in the backend path and made it too easy for a new feature to recover source
shapes instead of consuming checked semantic facts.

## Decision and non-goals

`rsscript-lowering` now provides a projection-free direct lowerer for the
checked-HIR subset: local bindings and assignment, literals, scalar binary
expressions, lists, maps, JSON objects, resolved list indexing, structured
`if`/`else` CFG branches, conditional loops with `break`/`continue`, return,
and resolved internal calls with
ordinary/read, `mut`, and `take` arguments. Call targets are looked up from checked
`CallResolution` in a deterministic `FunctionId` table. `CompiledIr::mir()`
prefers this path. Unsupported resource scopes, async, fields, records,
variants, and match explicitly return a
lowering error; only the existing compatibility caller may then choose the old
`ExecutableIr` bridge.

This is not the final HIR-to-MIR lowerer and does not change language syntax or
make `ExecutableIr` a new stable API.

## Compatibility and migration

The direct path is behavior-preserving for its covered pre-1.0 subset. Compiler
output keeps the compatibility IR only while unmigrated capabilities exist;
callers may request `checked_hir_mir()` to assert that no fallback occurred.
Artifact, Provider, and SDK wire schemas are unchanged.

## Verifier and security impact

The direct lowerer creates the same private, verifier-checked MIR model and
does not bypass bytecode verification. Unsupported semantic forms fail closed
at the lowering boundary. No authority, resource, cancellation, or isolation
claim changes.

## Provider and backend impact

Providers are unaffected. VM codegen consumes the resulting MIR unchanged;
experimental backends gain a semantic-HIR path for the covered subset without
reading source syntax or `ExecutableIr` nodes.

## Evidence

Compiler coverage asserts direct scalar, branch, and loop programs (including
`break`/`continue`) produce valid owned MIR with explicit CFG terminators, and
that resolved internal and external calls preserve their typed target and
`read`/`mut`/`take` identity. SDK migration tests compile all direct HIR MIR
shapes to
verifier-approved Artifacts and execute them in the VM with their expected
results.
