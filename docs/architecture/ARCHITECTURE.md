# RSScript architecture

This is the code map. It says what each crate is for, where its entry points
are, what invariant it guards, and where to add things. The dependency direction
below is enforced by `crates/rsscript-sdk/tests/architecture/` and
`tools/rsscript-xtask/src/repository_architecture.rs`; changing it means
changing those tests in the same commit.

## The pipeline in one picture

```text
  .rss / .rssi text
     │  rsscript-syntax        lexer → parser → AST (+ desugars, formatter, prefix oracle)
     ▼
  Program AST
     │  rsscript-semantics     module isolation → HIR → checks → ValidatedProgram
     ▼
  checked HIR
     │  rsscript-lowering      one-way lowering to typed CFG MIR
     ▼
  VerifiedMir                   rsscript-mir owns the model and its structural verifier
     │  rsscript-codegen-vm    MIR → rsscript.bytecode.v1 + typed executable facts
     ▼
  BytecodeArtifact              rsscript-bytecode owns the wire format and the verifier
     │  rsscript-artifact       + analysis + provenance = Artifact Bundle
     ▼
  ArtifactBundle
     │  rsscript-sdk            verify → admit → link providers → execute → ExecutionReport
     ▼
  rsscript-vm                   register interpreter, scheduler, limits, optional Cranelift tier
```

Everything above the bundle is platform-neutral and never sees a provider,
a limit, or a host. Everything below the bundle never sees source text, an
AST, or the checker. The bundle is the only thing that crosses.

## Crates

### Vocabulary and contracts (no internal dependencies)

| Crate | Owns | Notes |
| --- | --- | --- |
| `rsscript-core-types` | `FileId`-style source identities, `CancellationToken`, `MonotonicDeadline`, `OperationContext`, and the total text helpers in `core_types::text` | Dependency-free. Shared by frontend and VM so neither depends on the other. |
| `rsscript-abi-model` | `ExternalSymbol`, `WireType`, `WireValue`, `DataEffect`, `FunctionSignature`, `RUNTIME_ABI_VERSION` | The canonical type vocabulary that crosses the compile/execute boundary. A type that cannot be spelled here cannot be a fact in an artifact. |
| `rsscript-diagnostics` | `Diagnostic`, `Span`, `Severity`, `Fix`, the code registry in `src/implementation.rs`, and every code's explanation | `docs/generated/diagnostic-catalog.*` is a pure projection of this registry; a code without an explanation fails a test. |

### Frontend

**`rsscript-syntax`** — lexer, parser (`src/parser/{items,stmt,expr,pattern,types,scan}.rs`), AST (`src/ast.rs`), formatter, lints, and the prefix oracle (`src/prefix.rs`) used by `rss generate`. Two desugars run before the checker sees the tree: `function_value_desugar.rs` turns named functions used as values into closures, and `async_await_hoist.rs` hoists awaits out of expression positions. Brace struct literals and comma-terminated match arms are parser productions that build the same AST as the canonical spellings, so `rss fmt` is the normalizer. The work budget for frontend queries lives here too.

**`rsscript-semantics`** — the checker, and the largest crate. Entry points are the `analyze_*` / `validate_*` functions in `src/analyzer.rs`; `CompilationSession` in `src/database/` caches results by document revision for the language service. The order of work, from `Analyzer::run`:

1. `module_isolation.rs` rewrites module-scoped names to globally unique ones and decides cross-module privacy (RS0019) before any other pass.
2. `hir/lower/` builds checked HIR from the AST; `hir/infer.rs` is the single bottom-up type inference.
3. `checks/declarations` (signatures, duplicates, generic constraints, `fresh` return rules, run over sources *and* supplied interfaces), `checks/types`, `checks/calls` (argument binding, effects, closure contracts, protocol bounds), `checks/body` (bindings, places, ownership effects, resources, `?`, closure captures), `checks/forbidden`.
4. `control_flow.rs` owns divergence, exhaustiveness, definite assignment, `break`/`continue` targets, and the `let … else` rule; `ownership.rs` and the `local_flow_*` modules own moves, `local`/`manage`, and use-after-move.

The output is a `ValidatedProgram` only when there are no error diagnostics. `interface_catalog` holds the embedded core and standard-package `.rssi` sources that every single-file check sees. `symbols.rs` is name resolution plus the did-you-mean tables for RS0206/RS0203. `generation.rs` and `completion.rs` are the semantic half of the generation oracle.

**`rsscript-lowering`** — `src/mir.rs` plus `src/mir/lowerer*.rs`: the one-way transition from checked HIR to `VerifiedMir`. Anything the checker accepts must lower; a `MirLoweringError::Unsupported` is a bug, and `crates/rsscript-sdk/tests/fixture_build_corpus.rs` builds every pass fixture to keep it that way.

**`rsscript-mir`** — the typed CFG model (`MirInstruction`, `MirTerminator`, `BasicBlock`, `MirFunctionSignature`) and its structural verifier (`src/verify.rs`). It knows nothing about source. Resources, tasks, channels, places, and provider calls are explicit instructions here.

**`rsscript-compiler`** — a thin composition layer: `CompiledIr`, `compile_validated_to_bytecode`, the intrinsic catalog (`intrinsics.toml`), and re-exports of the frontend API. Frontend-only by default; the `lowering` and `bytecode` features pull in the two crates above. It never depends on the SDK or the VM.

### Executable format

**`rsscript-codegen-vm`** — MIR to `rsscript.bytecode.v1`. It emits the register instructions the VM executes and, in `src/facts.rs`, the typed executable facts the verifier and the native tier rely on. A fact must be provable from MIR; an unprovable type projects to `Unknown`, never to a guess.

**`rsscript-bytecode`** — the wire format (`BytecodeArtifact`), the `BytecodeVerifier`, and `src/typed_facts.rs`, which checks the emitted facts against the instruction stream. The VM constructs only from a `VerifiedBytecode`.

**`rsscript-artifact`** — the Artifact Bundle: bytecode bytes, the analysis envelope (`rsscript.source_analysis.v1` or `rsscript.package_analysis.v1`), external contracts, provenance, and one digest over all of it. Also the policy-neutral `SemanticDiffV2`.

### Execution

**`rsscript-vm`** — the reference execution engine and, behind the `native-jit` feature, the Cranelift tier. Read `src/reg_vm/mod.rs` first: `RegVmExecutable` wraps a verified unit; `RegVm` is the machine state. The interpreter is `exec_ops.rs::drive`, an explicit-frame loop over `RegInstr` (117 opcodes in `model.rs`); every instruction passes through `exec.rs::tick`, which is where step budgets, cancellation polling, and deadlines are charged. `scheduler.rs` is the cooperative task scheduler (`run_program`, FIFO ready queue, deterministic wake order). Values are `VmValue` in `src/vm_value.rs`. `intrinsics/` implements the core library calls; `src/corelib.rs` is the pure algorithm library behind them and may not name any VM type. External calls go through `ExternalFunctionRegistry` in `src/eval_types.rs`, which resolves a symbol to a provider at execution time and never at compile time.

The native tier lives in `src/reg_vm/native/` (translation, passes, typed regions, facts) and `src/reg_vm/tier*` (admission, OSR plans, deopt resume, tier-0 leaf execution). The rule is in `docs/spec/native-jit-contract.md`: the interpreter is the oracle, and a region whose steps, intrinsic calls, or allocations cannot be attributed exactly declines rather than runs unmetered. `rsscript-jit-cranelift` is the backend it drives: Cranelift codegen, executable memory, and guard/deopt machinery. It and `process-guard` are the only crates that contain unsafe code; the lint-inheritance check in xtask requires both to deny undocumented unsafe blocks.

**`rsscript-provider-api`** — how a host supplies a service: `ProviderDescriptor`, `ProviderFunction`, the `WireInterpreterFn` family, resource tables, replay contracts, and `ProviderRegistry`, which links a symbol only when the semantic signature hash matches. `providers/*` are the official implementations (fs, env, process, http, time, entropy, log, cli). `rsscript-provider-bindgen` generates their contract code from `.rssi`; `rsscript-provider-conformance` is the kit every official provider must pass.

**`rsscript-sdk`** — the embedding façade and the only place compile and execute meet. The reviewed path is `Compiler::compile_snapshot` → `BuiltArtifact` → `ArtifactVerifier::verify` → `VerifiedArtifact::admit` → `Runtime::link` → `LinkedArtifact::execute(ExecutionRequest)` → `ExecutionReport`. `RunLimits` is the whole control vocabulary. The public surface is pinned by `sdk-api-inventory.md` and `sdk-api-snapshot.v1.toml`; adding an export means updating both.

**`rsscript-runner-protocol`** and **`process-guard`** — the isolated runner's request/response schemas with their size caps, the versioned `ExecutionReportV2`, and the Unix `pre_exec` limits boundary. `rss run` uses these by default; `--trusted-in-process` bypasses them.

### Tools and applications

`rsscript-cli` is the composition root for `rss`; `src/cli/inputs.rs` is the one place that assembles sources, interfaces, and the standard prelude, and every command goes through it. `rsscript-project` and `rsscript-workspace-loader` turn a directory into an immutable `FrontendInputSnapshot`. `rsscript-language-service` and `lsp` are the editor path, frontend-only. `rsscript-review` is the optional package-review integration; nothing in the workspace depends on it today, and the CLI does not expose it. `tools/rsscript-xtask` generates `docs/generated/*`, validates CI and the architecture rules, scores the eval corpus in `evals/`, and runs tier commands.

## Invariants worth knowing before you change anything

- **Check equals build.** A program `rss check` accepts must build, verify, and run. The build corpus test and the pass fixtures enforce it; a new lowering gap is a bug, not a limitation to document.
- **Facts are proofs.** Typed executable facts, native region facts, and analysis are derived from MIR and the verifier, never from the checker's guesses. Unknown is always acceptable; wrong is never.
- **Providers are runtime state.** Compilation records symbols; the artifact is identical whichever provider is linked later. Signature hashes, not names, decide linkage.
- **Limits are availability controls, not permissions.** Step, memory, intrinsic, provider-call, output, depth, cancellation, and deadline limits protect the host; they are not an isolation claim. Untrusted code goes through the runner.
- **The interpreter is the oracle.** Any other engine must produce the same outcome, the same termination reason, and the same usage counts, or decline.
- **One source of truth per catalog.** Keywords come from the lexer and parser tables, diagnostics from the registry, interfaces from the embedded `.rssi` sources, intrinsics from `intrinsics.toml`; `docs/generated/` is regenerated, never edited.

## Where to add things

| You want to… | Start in |
| --- | --- |
| Add syntax | `rsscript-syntax/src/parser/`, then the formatter, then `docs/roadmap.md` says whether it is allowed at all |
| Add a checker rule | the matching `rsscript-semantics/src/checks/` module, a code in `rsscript-diagnostics`, fixtures under `rsscript-sdk/tests/fixtures/{pass,fail}` |
| Add a core library function | its `.rssi` under `stdlib/`, `intrinsics.toml`, the VM intrinsic in `rsscript-vm/src/reg_vm/intrinsics/`, and a MIR builtin descriptor |
| Add a host service | a new provider under `providers/` built from its `.rssi` with `rsscript-provider-bindgen`, plus conformance tests |
| Change what the artifact carries | `rsscript-bytecode` or `rsscript-artifact`, with an ADR; these are versioned wire contracts |
| Touch limits or telemetry | `rsscript-sdk/src/execution.rs` (`RunLimits`, `ExecutionReport`) and `rsscript-runner-protocol` (`ExecutionReportV2`); both are contracts |
| Make the JIT cover more | `rsscript-vm/src/reg_vm/native/`, with a differential test in `rsscript-sdk/tests/native_jit_differential.rs` before anything else |
