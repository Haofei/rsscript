# ADR 0028: Semantic ownership of the builtin type catalog

## Status

Accepted

## Problem

Compiler retained a duplicate-facing builtin type catalog even after unknown
type validation moved to semantics. Self-host metadata and signature validation
could therefore observe a different source of truth.

## Decision and non-goals

`rsscript-semantics::BUILTIN_TYPE_NAMES` and `is_builtin_type_name` are the
only canonical builtin type identities. Compiler consumers use them directly.
This does not change the set of language builtin types.

## Compatibility and migration

No Artifact, Provider ABI, SDK, or persisted-format change. Existing internal
compiler paths are redirected to the same catalog.

## Verifier and security impact

None.

## Provider and backend impact

None.

## Evidence

Strict compiler clippy and SDK architecture tests cover the new owner and the
self-host metadata generator.
