# ADR 0197: Move the workspace diagnostic query contract to semantics

- Status: Accepted
- Date: 2026-08-14

## Problem

`CompilationSession` already owned the immutable frontend input, operation
checks, and diagnostic cache. However, the callback trait used to fill that
cache was defined by `rsscript-language-service`. That made the editor layer
the owner of a frontend-query contract and forced every future semantic-query
client to depend on an LSP-oriented abstraction.

The full analyzer is still being migrated from `rsscript-compiler`, so the
implementation cannot yet move wholesale without reintroducing compiler-private
checks into the semantic crate.

## Decision

`rsscript-semantics` owns `WorkspaceDiagnosticQuery`. The contract accepts the
session-produced `FrontendInputSnapshot` and `OperationContext`, then returns
diagnostics or an operation abort. `CompilationSession` accepts this contract
directly and owns both cache invalidation and operation polling.

`rsscript-language-service` stores an injected semantic query implementation;
it no longer declares a local diagnostic-analyzer protocol. LSP composition may
temporarily inject the single compiler adapter defined by ADR 0192, but that
adapter is now an implementation of a semantic contract rather than a language
service API.

## Consequences

The next migration can replace the compiler adapter with semantic-owned resolve
and type queries without changing language-service APIs. This does not claim
that full workspace diagnostics already live in `rsscript-semantics`; only the
query boundary, immutable input, operation handling, and cache ownership have
moved.
