# ADR 0141: Namespace isolation and workspace HIR belong to semantics

- Status: Accepted
- Date: 2026-08-14

## Problem

Compiler-owned namespace isolation ran before HIR construction, while the
session could only cache per-file HIR. A workspace client therefore had no
shared, interface-aware HIR query and could accidentally reconstruct a second
source/interface module graph with different name rewriting.

## Decision and non-goals

`rsscript-semantics` owns module namespace isolation, source/interface graph
partitioning, diagnostic demangling, and a cached `CompilationSession`
workspace-HIR query. The query reuses revision-keyed parse trees, isolates the
combined source/interface graph once, then constructs HIR while preserving the
interface role.

Compiler retains a thin compatibility re-export. This decision does not move
the remaining resolve/type diagnostic orchestration from compiler, nor does it
make workspace HIR a validated executable program.

## Compatibility and migration

The rewrite algorithm and public compiler helper behavior are preserved.
`workspace_hir` is a new semantic query with immutable session lifetime and no
Artifact, Provider, or runtime contract change.

## Verifier and security impact

The query performs no I/O and grants no authority. It observes cancellation
and deadline checks before and after cache access, so a cancelled request
cannot receive a stale cached HIR.

## Provider and backend impact

None. Backends continue to consume validated semantic output or MIR; this is a
frontend query boundary only.

## Evidence

Semantic tests cover namespace rewriting, source/interface partitioning,
workspace-HIR cache reuse, revision invalidation, and cancellation. The
architecture gate rejects a compiler-owned second isolation implementation.
