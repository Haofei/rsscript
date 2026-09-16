# ADR 0014: MIR-owned map construction

## Status

Accepted

## Problem

After list literals could lower through typed MIR, map literals still forced
supported source programs onto the transitional source-shaped execution path.
The v1 VM already has a checked `MakeMap` instruction, so retaining map AST
nodes at the backend boundary provided no additional semantic information.

## Decision and non-goals

MIR adds `MakeMap { destination, entries }`, where every entry is an ordered
pair of resolved key and value `ValueId`s. The verifier therefore applies the
same definition and dominance rules to both map sides before codegen maps them
to v1 `MakeMap` register pairs.

This ADR does not add map field/index access, map mutation, map-pattern
dispatch, record/variant literals, implicit key coercion, or a new map
ordering contract. Those features remain rejected by MIR lowering until they
have explicit backend-shaped operations and validation.

## Compatibility and migration

The MIR operation is additive pre-1.0 and targets an existing v1 bytecode
opcode. Existing Artifacts remain readable. A runtime without that opcode
rejects the Artifact through normal fail-closed validation. The compatibility
adapter continues to cover aggregate operations outside this limited map
construction subset.

## Verifier and security impact

No Provider capability, authority, or isolation boundary changes. MIR proves
both members of each entry are defined on every path to construction, and the
bytecode verifier validates the bounded register-pair payload. Key hashability
and map type compatibility remain checked semantic facts, not runtime policy.

## Provider and backend impact

Providers are unaffected: maps are language values. Backends which do not yet
preserve ordered key/value evaluation and map semantics must reject `MakeMap`
explicitly rather than rebuild source-shaped map expressions.

## Evidence

Lowering tests assert explicit `MakeMap` output. The SDK migration corpus
compiles a source map literal, executes both the MIR oracle and the legacy VM,
and executes verifier-approved MIR bytecode in the VM with matching output.
