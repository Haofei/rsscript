# ADR 0232: Parser-owned generation oracle

## Status

Accepted; the v1 baseline is Experimental.

## Problem

An agent that authors RSScript needs feedback it can consume mechanically while
iterating on generated source. Existing diagnostics and compiler entry points
serve their current callers, but do not by themselves define one canonical query
contract, the context that makes a result reproducible, or the ownership of
facts across syntax, semantics, and compilation. Ad-hoc consumers could parse
the same input twice, infer semantic success from parser acceptance, or treat a
partial answer as an authorization to compile or run.

The product needs a generation oracle without making machine generation a trust
signal, creating a second frontend, or turning diagnostic output into a policy
or execution decision.

## Decision and non-goals

The generation oracle uses a syntax-owned, versioned prefix entry point. Syntax
owns prefix status, token/span coordinates, conservative cursor context, and
expected terminals. Semantic and compiler consumers must not maintain a second
keyword or grammar table.

The v1 baseline reparses a recovered immutable source snapshot once in the
semantic analyzer after the syntax-prefix query. This is a stated performance
and completeness limitation, not an incremental-parser claim. A future parser
checkpoint may remove that reparse, but only if it preserves the same syntax
ownership and observable v1 facts.

Ownership is deliberately staged:

```text
generation query
  -> syntax: source identity, parsing, syntax diagnostics
  -> semantics: name/type/ownership/contract facts and semantic diagnostics
  -> compiler: compilation facts only after semantic success
  -> canonical machine context + structured result
```

Semantics owns whether a syntactically valid program has the language facts
needed to proceed. The compiler owns compilation facts and compiler-stage
diagnostics, but it is not an alternate syntax or semantic oracle. The query
result preserves the completed stage and does not imply that later stages ran.

The initial contracts split machine context deliberately: `rss generate`
returns a bounded v1 prefix/continuation result with source length, Core policy,
interface revision, syntax/semantic completeness, and semantic validity;
generated language-card artifacts carry language/spec versions, grammar hash,
diagnostic explanations, and the Core interface catalog. `rss check --json`
and `rss fix --json` remain the structured diagnostic and instance-level edit
contracts. Future context identity can be strengthened without pretending the
current revision counter is a content digest.

The oracle chooses soundness over completeness. It may return `unknown`,
`not-run`, or an explicit bounded/unsupported result when it cannot establish a
fact within its contract. It must never report semantic or compiler success from
parser acceptance, conceal a failed stage as a successful later stage, or grant
Artifact admission, Provider selection, host authority, or execution authority.

This decision does not add language syntax, public intrinsics, host APIs,
policy evaluation, a self-modifying agent loop, or an alternate compiler. It
does not split the repository, remove an existing backend, or reorder the Cargo
workspace. It does not assign new feature work to Rust AOT, Cranelift JIT,
REIR, or self-hosting; those surfaces remain limited to correctness, security,
dependency, and regression maintenance.

## Compatibility and migration

This ADR introduces public Rust query types, two Experimental CLI JSON schemas,
generated machine-reference schemas, and an offline evaluation report schema.
It does not change source, Artifact, Provider ABI, SDK execution, or persisted
runtime data.

Further implementation proceeds behind versioned, opt-in query envelopes:

1. Expand parser-derived terminals and cursor contexts while retaining explicit
   `partial` completeness where coverage is not exhaustive.
2. Add content identities and stale-context rejection to the current immutable
   source/interface revision model.
3. Extend semantic candidates only when scope, type, and ownership facts can be
   proved at the cursor.
4. Grow agent-recovery and soundness evaluations from the checked-in offline
   corpus and caller-supplied candidates.

Existing parser, semantic, compiler, SDK, LSP, and CLI callers retain their
current contracts during this migration. Compatibility adapters, if needed,
read the new envelope at the boundary; they do not make legacy prose or an
unbound diagnostic set canonical. Any future public or persisted schema version,
writer/reader change, or removal of a legacy path requires its own compatibility
decision and fixtures.

## Verifier and security impact

Machine-authored source is untrusted input. The oracle is bounded by the same
input, cancellation, and resource constraints appropriate to its owned stages;
it returns an explicit non-success outcome when those bounds are reached.
Canonical context excludes host secrets, deployment policy, Provider
implementation state, and ambient authority. It is diagnostic evidence, not a
capability, a verifier bypass, or a substitute for Artifact admission and
isolated execution.

Bytecode verification, Provider signature validation, runner admission, and VM
limits are unchanged. A positive oracle result cannot cause any of them to be
skipped.

## Provider and backend impact

Providers receive no new authority and no query-time implementation-state
dependency. The reference VM remains the execution model. Rust AOT, Cranelift
JIT, REIR, and self-hosting neither own nor consume this contract as a feature
roadmap; maintenance may preserve their existing correctness, security,
dependency, and regression evidence only.

## Evidence

Before the Experimental baseline is considered ready for broader use, add or
extend focused evidence for:

- canonical context and diagnostic serialization across equivalent inputs;
- stage ownership and the absence of duplicate parsing;
- parser-success / semantic-failure and semantic-success / compiler-failure
  distinctions;
- bounded, cancelled, unsupported, and stale-context outcomes; and
- agent recovery evaluations that reject unsound acceptance and preserve the
  existing verifier, Provider, and execution boundaries.
