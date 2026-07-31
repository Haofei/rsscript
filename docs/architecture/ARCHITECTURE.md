# RSScript Architecture

RSScript is organized around one primary product path:

```text
source
  -> syntax
  -> checked semantics
  -> review evidence
  -> REIR decision or executable lowering
```

The review model is the core product boundary. Execution backends consume
checked semantics; they do not redefine the language or silently convert
unknown behavior into supported behavior.

## Layers

### Syntax

`crates/rsscript/src/syntax/` and the lexer own tokens, parsing, recovery, and
source-preserving AST shapes. They must not depend on package policy, review
risk, lowering, or runtime services.

### Semantic Analysis

`hir.rs`, `analyzer.rs`, and `checks/` own resolved language facts and
diagnostics. This layer validates types, effects, resources, handles, weak
references, and executable surfaces before a backend consumes them.

### Review

`review.rs`, package review code, and the `reir` crate own review facts,
semantic differences, reconciliation, and gate decisions. `Unknown` is a
first-class result and is never treated as low risk.

### Lowering And Execution

`rust_lower/` emits Rust from checked semantics. The register VM is the
reference interactive execution path. JIT and native plugins are experimental
trusted-code accelerators and explicit unsafe boundaries.

Lowering and execution must reject unsupported checked forms rather than
inventing backend-specific semantics.

### Package Domain

`package/` owns manifests, `.rssi` contracts, dependency graphs, semantic locks,
review aggregation and snapshots. Package operations
consume compiler and review APIs; they do not define language semantics.

Security-sensitive package operations must act on an immutable snapshot. A
reviewed path is not an execution capability.

### Runtime And Adapters

The `runtime` crate owns the host-facing runtime contract and resource
accounting. Network, process, filesystem, native, and JIT facilities are
adapters with explicit trust and resource policies.

The runtime is not a sandbox. Runtime execution is restricted to source and
dependencies controlled by the operator. Third-party package support ends at
static check, review, semantic diff, and evidence generation; no architecture
component authorizes or executes third-party code.

### Applications

The CLI, LSP, GitHub Action, and other entrypoints are composition roots. They
select deployment profiles, construct policies and budgets, and invoke library
use cases. Domain code should not discover policy through ambient process state.

## Dependency Rules

The intended dependency direction is:

```text
syntax
  -> semantics
  -> review/package domain
  -> use cases
  -> runtime and infrastructure adapters
  -> CLI, LSP, and CI entrypoints
```

The following rules are architectural invariants:

1. Syntax does not depend on package, review, runtime, or backend code.
2. Lowering consumes validated semantic facts.
3. Package tooling does not reinterpret the language.
4. Review decisions preserve incomplete and unknown evidence.
5. Host capabilities require explicit policy and bounded resources.
6. Unsafe implementation remains isolated in dedicated crates and adapters.
7. Security decisions bind to immutable content and producer provenance.
8. Restricted execution cannot accept trusted-only native or JIT handles.

## Stable Boundary Types

New cross-layer APIs should prefer validated or authorized values over raw
strings, paths, and booleans. Important boundary concepts include:

```text
SourceSnapshot
ParsedProgram
SemanticDatabase
AnalysisResult
ValidatedProgram
ValidatedBundle
AuthorizedPackage
ArtifactHandle
ExecutionPolicy
OperationContext
```

`SourceSnapshot`, `SemanticDatabase`, `AnalysisResult`, and `ValidatedProgram`
form the implemented frontend chain. Other names describe intended invariants
and do not imply that every boundary is already fully implemented.

## Current Hotspots

Large compiler, package, LSP, runtime, register-VM, and JIT modules remain
maintenance risks. They should be split around validated state transitions,
policy boundaries, and platform adapters, not around arbitrary line counts.

Active consolidation and security work is tracked only in
[the roadmap](../roadmap.md). Current support and unresolved boundaries are
recorded in [status](../status.md) and [support](../support.md).

## Non-Goals

- Package tooling must not introduce unimplemented language semantics.
- Lowering must not accept code the frontend cannot explain.
- Unknown review regions must not be classified as safe.
- In-process native code and JIT machine code are not security sandboxes.
- Experimental execution features do not become supported solely because an
  implementation exists.
