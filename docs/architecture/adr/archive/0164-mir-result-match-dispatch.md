# ADR 0164: Lower canonical Result matches to MIR CFG dispatch

- Status: Accepted
- Date: 2026-08-14

## Problem

Direct checked-HIR lowering already represented `Ok(value)` and `Err(value)`
as `MakeResult`, but matching either constructor still selected the legacy
executable-IR path. That left a built-in, provider-neutral control-flow
semantic outside the typed MIR route.

## Decision

Add `MirTerminator::MatchResult { value, ok_target, err_target }` and
`MirInstruction::UnwrapResult { destination, source, ok }`. The lowerer
recognises only the canonical `Ok` and `Err` Result patterns, validates one
flat binding or wildcard, and projects a named arm payload only after the
corresponding CFG edge has been selected.

VM codegen reuses the existing verifier-checked `MatchResult` and
`UnwrapVariantValue` bytecode operations. The Result tags are MIR semantics,
not unresolved source callees or user-defined variant layouts.

## Non-goals

Nested Result patterns, guards, match-time moves, Result resource transfer,
and arbitrary variant payload projection remain fail-closed unless covered by
their own lowered operations and verification rules.

## Compatibility and migration

This adds an internal MIR lowering capability over existing bytecode v1
operations; Artifact, Provider, and runtime ABI versions remain unchanged.
The legacy executable-IR path remains explicitly gated for unsupported match
forms while the direct corpus grows.

## Verifier and security impact

The new terminator participates in CFG, dominance, resource-cleanup, and task
closure traversal through its explicit two edges. `UnwrapResult` has an owned
source and destination that are checked by ordinary value-definition and
dominance rules. No host authority, Provider behavior, or isolation guarantee
changes.

## Provider and backend impact

Providers are unaffected. The test-only MIR interpreter and VM bytecode
codegen share the same canonical Result branch and projection operations; AOT,
JIT, and other labs remain consumers of stable MIR/bytecode contracts only.

## Evidence

`rsscript-sdk`'s MIR migration corpus differentially executes Result matches
with a payload binding in statement and expression form across the legacy VM,
MIR conformance interpreter, and verified bytecode VM.
