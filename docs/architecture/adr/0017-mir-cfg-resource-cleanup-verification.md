# ADR 0017: verify MIR resource cleanup on every CFG exit

## Status

Accepted

## Problem

Resource lifetime verification already propagated live resources across CFG
edges, but the migration corpus only demonstrated a linear release-before-
return case. A future change to branching or lowering could therefore regress
the requirement that each reachable return releases the resource.

## Decision and non-goals

Keep the existing conservative resource-liveness transfer rule and add a
targeted valid/invalid branch fixture: both branches releasing succeeds, while
one reachable return without release fails with `ResourceLeak`.

This does not add an instruction, language syntax, Provider cleanup callback,
cancellation edge, unwind edge, or new Artifact encoding. It documents and
tests an existing MIR validation contract only.

## Compatibility and migration

There is no serialized or public API change. Valid MIR has the same behavior;
malformed MIR that omitted release on a reachable return has always been
invalid and is now mechanically guarded during the migration.

## Verifier and security impact

The added fixture protects deterministic resource cleanup on normal CFG exits.
It does not alter trusted-host assumptions, isolation, authority, cancellation,
or Provider error semantics.

## Provider and backend impact

Providers and backends receive no new surface. Backends continue to rely on
verified MIR's guarantee that normal return paths do not retain a live resource.

## Evidence

`resource_cleanup_is_required_on_every_reachable_return_edge` builds a
three-block typed MIR function and proves the verifier accepts releases on
both branch exits while rejecting an omitted release.
