# ADR 0218: Route the default CLI check through `CompilationSession`

- Status: Accepted
- Date: 2026-08-14

## Decision

The normal core-aware single-file `rss check` path now loads its already-read
source and interface buffers into `CompilationSession` and consumes the
session-owned workspace analysis. Filesystem access remains in the CLI, while
parse, resolve, type, HIR, and diagnostics share the semantic query boundary.

## Scope

The command still keeps `--no-core` on its explicit compatibility analyzer
path because that mode intentionally bypasses the standard core interface set.
Package inspection and experimental AOT callers remain separate migration
tasks. No Provider, runtime, or package filesystem dependency is introduced
to the default CLI closure.
