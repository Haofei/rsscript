# RSScript architecture

RSScript is split by semantic responsibility:

```text
source
  -> syntax
  -> semantic validation
  -> typed HIR
  -> one-way lowering
  -> owned executable IR
  -> verified VM bytecode or generated Rust

package interface + binding descriptor
  -> external symbol linker
  -> runtime provider registry
  -> optional host implementation

package analysis + binding/provider metadata
  -> optional review/REIR adapter
```

The required dependency direction is syntax → semantics → lowering → owned
executable IR. `rsscript-vm` consumes only owned executable IR and execution-layer
contracts; it does not depend on syntax, semantics, HIR, lowering, or the compiler.
Host providers depend on the host API/runtime boundary; the compiler does not
depend on concrete providers, deployment policy, or REIR.

`rsscript-abi-model` owns external symbols and semantic signature hashes without
depending on provider implementations. `rsscript-provider-api` owns versioned
provider descriptors and load-time linking. Concrete native or OS providers
adapt to that API and are never part of the semantic model.

## Compiler boundary

Parsing, validation, lowering, formatting, symbols, and package structural checks
are platform-neutral. The compiler records bodyless interface functions as
external symbols. Link/provider failures are not language type errors.

The default interface set contains deterministic core APIs only. Host packages
must be explicit dependencies and are never injected for single-file analysis.

`rsscript-compiler` is the frontend and lowering composition layer. It is
frontend-only by default and does not depend on the embedding SDK, VM, or CLI.
The SDK-owned adapter is the only module that combines compiler output with the
VM emitter; neither side depends back on the composition layer.
`rsscript-semantics` owns immutable source snapshots, `SemanticDatabase`, the
complete/incomplete analysis state, and the error-free `ValidatedProgram`
transition. The compiler analyzer is currently the sole integration point that
assembles those facts while the remaining semantic passes migrate.
`rsscript-lowering` owns the single validated-HIR to MIR transition.
`rsscript-mir` owns the frontend-free typed CFG model and its structural
verifier. Resource, async, and dynamic/provider operations are explicit MIR
instructions; source-shaped executable IR was retired after the parity corpus
passed.
The optional `rsscript-mir/conformance` feature is a test-only reference
interpreter: dual-path fixtures compare it with the legacy VM before a
capability can advance to MIR-only execution.
`rsscript-sdk` is the stable embedding façade; embedders do not depend on the
compiler's analyzer database, register VM, Rust AOT, JIT, package review, or
source-map types directly. `rsscript-cli` is the composition root.

`WorkspaceLoader` is the low-level OS/VFS capture adapter. `rsscript-project`
turns its captured file set into an immutable compiler-ready frontend snapshot
without widening the compiler dependency closure. The
`LanguageService` itself consumes explicit document revisions, rejects stale
updates, caches diagnostics by document plus interface revision, and accepts
cancellation, monotonic deadlines, and diagnostic budgets. It depends on the
frontend-only compiler API and never enables lowering or legacy execution-IR
features.

## Runtime boundary

The independent `rsscript-vm` crate owns values, managed handles, resource slots,
scheduling, cancellation, execution limits, bytecode dispatch, and external-call
dispatch. `rsscript-codegen-vm` produces Artifacts from verifier-owned MIR; the
VM accepts only verifier-owned bytecode and cannot observe frontend databases or
syntax nodes. `CallExternal` records a symbol and arguments.
`ExternalFunctionRegistry` resolves the symbol at execution time.

Provider selection is runtime state and cannot change a compiled artifact.
Filesystem, environment, process, network, time, entropy, logging, CLI, and OS
handles belong behind providers.

Execution budgets and deadlines are availability controls. They are deliberately
separate from permissions and are not an isolation claim.

## Package and review boundary

Core package analysis uses `rsscript.package_analysis.v1`. It can report semantic
facts and external symbols but contains no host grants. Binding descriptors use
`rsscript.bindings.v1` and may carry optional review metadata.

`WorkspaceSnapshot` derives analysis directly from captured sources, interfaces,
and the semantic database. It never invokes package review, risk classification,
provider selection, or native implementation inspection. Analysis and lowering
therefore share one immutable digest even when the optional review subsystem is
not compiled or run.

Review/REIR combines validated semantic facts with binding, provider, deployment,
or runtime evidence. Disabling review must not change AST, HIR, validation,
lowering, or generated code.
