# RSScript architecture

RSScript is split by semantic responsibility:

```text
source
  -> syntax
  -> semantic validation
  -> platform-neutral HIR/lowering
  -> VM bytecode or generated Rust

package interface + binding descriptor
  -> external symbol linker
  -> runtime provider registry
  -> optional host implementation

package analysis + binding/provider metadata
  -> optional review/REIR adapter
```

The required dependency direction is syntax → semantics → lowering. Runtime-core
consumes lowered programs. Host providers depend on the host API/runtime boundary;
the compiler does not depend on providers, deployment policy, or REIR.

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

## Runtime boundary

The VM owns values, managed handles, resource slots, scheduling, cancellation,
execution limits, bytecode dispatch, and external-call dispatch. `CallExternal`
records a symbol and arguments. `ExternalFunctionRegistry` resolves the symbol at
execution time.

Provider selection is runtime state and cannot change a compiled artifact.
Filesystem, environment, process, network, time, entropy, logging, CLI, and OS
handles belong behind providers.

Execution budgets and deadlines are availability controls. They are deliberately
separate from permissions and are not an isolation claim.

## Package and review boundary

Core package analysis uses `rsscript.package_analysis.v1`. It can report semantic
facts and external symbols but contains no host grants. Binding descriptors use
`rsscript.bindings.v1` and may carry optional review metadata.

Review/REIR combines validated semantic facts with binding, provider, deployment,
or runtime evidence. Disabling review must not change AST, HIR, validation,
lowering, or generated code.
