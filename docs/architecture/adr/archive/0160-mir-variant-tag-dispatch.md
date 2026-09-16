# ADR 0160: Lower resolved variant tags to MIR CFG dispatch

- Status: Accepted
- Date: 2026-08-14

## Problem

Direct checked-HIR lowering could construct a user-defined sum variant but
could not dispatch on its tag. That forced any variant `match` through the
legacy executable-IR path even when no payload binding was involved.

## Decision

Add `MirTerminator::MatchVariant { value, expected, match_target,
else_target }`. `expected` is a resolved semantic variant layout tag, not a
source pattern node. The lowerer accepts only payload-free variant patterns
whose tag is present in the checked-HIR variant table.

The MIR verifier rejects empty tags and invalid CFG targets. VM codegen emits
the existing verifier-checked `MatchVariant` bytecode operation, and the
test-only MIR interpreter follows the matching or fallback edge.

## Non-goals

Payload destructuring, binding projection, guards, exhaustiveness lowering,
and variant field mutation remain fail-closed on the direct MIR path.

## Compatibility and migration

This is an additive internal MIR capability using an existing bytecode v1
operation. Artifact, Provider, and runtime ABI versions do not change. The
legacy executable-IR path remains gated while more match forms migrate.

## Verifier, security, and backend impact

The terminator makes both dispatch edges explicit for control-flow, dominance,
and cleanup verification. It introduces no host capability or policy decision.
The direct migration corpus compares the legacy VM, MIR reference interpreter,
and verified MIR bytecode VM for payload-free sum statement and expression
matches.
