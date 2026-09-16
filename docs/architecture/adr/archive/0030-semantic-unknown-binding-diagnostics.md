# ADR 0030: Semantic ownership of unknown-binding diagnostics

## Status

Accepted

## Decision and non-goals

`rsscript-semantics::unknown_binding_diagnostics` owns lexical visibility and
unknown value-binding diagnostics.  It consumes immutable source declarations
and resolved HIR blocks, preserving existing scope behavior for parameters,
top-level constants and variants, local bindings, pattern bindings, closures,
resource scopes, loops, and select arms.  `rsscript-compiler` only orchestrates
the query and appends its diagnostics.

This decision does not change the language's existing source-shaped traversal
semantics.  In particular, it intentionally preserves the current diagnostic
coverage of aggregate literals and call receivers; extending that coverage is a
separate language-semantics change rather than an ownership migration.

## Compatibility and migration

Diagnostic codes, messages, fixes, and source spans remain unchanged.  No
Artifact, Provider, SDK, or runtime contract changes.

## Verifier and security impact

None.  Moving this check removes a duplicate compiler interpretation of lexical
scope and makes frontend clients consume the same diagnostic facts.

## Provider and backend impact

None.

## Evidence

The semantic unit test exercises parameter, local, global, and pattern scope
with a stable unknown-binding span.  Compiler and architecture suites verify
that the compiler delegates rather than retaining the recursive rule.
