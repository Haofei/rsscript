# ADR 0140: Compiler package dependencies are execution-gated

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's default frontend build declared package traversal, filesystem,
locking, temporary-directory, and content-hashing libraries unconditionally,
even though all direct use lives below the execution-only package and AOT
compatibility modules. This enlarged the `rss check` and language-tooling
closure without granting either path useful functionality.

## Decision and non-goals

The compiler now marks direct package-capture dependencies optional and selects
them exclusively through the existing `execution` feature. Direct dependencies
with no compiler source users are removed. `serde` and `serde_json` stay in the
default closure because generation and grammar APIs intentionally use them.

This does not yet remove package capture, review, native snapshots, generated
Rust, or their transitional compiler exports. It is a closure reduction, not a
claim that S05 compiler purity is complete.

## Compatibility and migration

The default compiler feature set remains frontend-only. Existing execution,
SDK compatibility, CLI build, and package APIs continue to enable `execution`
and therefore retain the same dependency capabilities. No source language,
Artifact, Provider, or runtime ABI changes.

## Verifier and security impact

The default frontend path no longer selects direct filesystem-locking or
temporary-directory libraries from compiler. Verification and runtime loading
are unchanged; this does not provide a new sandbox or authority boundary.

## Provider and backend impact

Providers, VM, AOT, JIT, and bytecode code generation are unchanged. The
execution compatibility path remains the only consumer of package I/O
dependencies.

## Evidence

Default and execution compiler checks both compile. The architecture gate
asserts every direct package/host dependency is optional and explicitly listed
in the execution feature, and the existing `rss check` Cargo-tree gate remains
frontend-only.
