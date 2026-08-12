# ADR 0107: Semantic ownership of fresh match-binding facts

- Status: Accepted
- Date: 2026-08-12

## Context

Compiler CFG construction previously interpreted resolved HIR to recognize the
single-payload `Some`/`Ok` match form, parse its `Option`/`Result` payload type,
and decide whether the scrutinee was a fresh-returning call. These are language
facts, not graph-construction mechanics.

## Decision

`rsscript-semantics::fresh_match_binding` owns this narrow HIR contract and
returns a `FreshMatchBinding` fact containing the binding name, payload type,
source identity, and fresh-scrutinee status. Compiler CFG construction converts
the fact to its local flow-node representation without reinterpreting match
patterns or payload types.

## Consequences

This preserves the existing fresh-return flow behavior and moves the semantic
boundary nearer to a future typed MIR. It does not affect source, artifact,
Provider, or runtime contracts.
