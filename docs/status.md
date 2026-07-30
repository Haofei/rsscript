# Current Project Status

This is the single current engineering-status document. It records boundaries,
not a chronological work log. Detailed remediation history remains available in
Git.

## Product State

RSScript is a `0.1.x` review-first language and evidence prototype. It is useful
for trusted local development, semantic package review, REIR generation, and
controlled CI experiments. It is not a sandbox, formal proof system, stable
registry, or production multi-tenant execution platform.

The binding support and deployment matrix is [support.md](support.md).

## Established Boundaries

- Compiler and review core forbid unsafe Rust; unsafe code is isolated in
  dedicated JIT, ABI, process, and GPU crates.
- Unknown review evidence remains explicit and production REIR policy is
  fail-closed.
- Package inputs and artifacts use bounded traversal, content identities,
  no-follow checks, staged writes, and atomic publication where supported.
- Native builds preserve reviewed features, run offline/frozen, and load
  digest-verified artifacts from private storage.
- VM and process paths have default time, output, memory/work, host-call, and
  process-tree controls. Unlimited/native modes require explicit trusted flags.
- Runtime network, HTTP, filesystem, database, channel, and stream paths have
  bounded variants and typed errors.
- Native ABI buffers are shape-checked before ownership transfer and released
  through RAII on valid success paths.
- LSP uses immutable snapshots, revisions, cancellation, bounded scheduling,
  and publication outside global state locks.
- REIR reconciliation has indexed exact matching and an operation budget.
- CI pins actions/toolchains, audits dependencies, separates Core and
  Experimental coverage, and promotes the same validated release artifact.

These controls reduce mistakes and denial-of-service exposure. They do not turn
host execution into isolation.

## Open Security And Correctness Work

| Area | Current limitation | Required closure |
| --- | --- | --- |
| Package authorization | Review can precede capture of the complete dependency closure | Snapshot first; review, lower, build, and publish only that immutable graph |
| Deployment profile | CLI fails closed outside `local-trusted`, but embedding APIs do not carry one mandatory policy | End-to-end execution context and capability checks |
| Native/JIT/GPU | Trusted code still runs in the host process | Killable OS-isolated workers with bounded IPC |
| Windows | Secure cache ACL and atomic Job attachment remain incomplete | SID/DACL validation and suspended process launch |
| Host authority | Some APIs still accept paths, URLs, commands, and DSNs as authority | Root/endpoint/executable/database capability handles |
| Capability evidence | Some native capability facts are author declarations | Independent verification and provenance |
| Frontend budgets | Limits exist in several phases but are not one end-to-end contract | Unified source/token/depth/node/diagnostic budget |
| External integrations | Live PostgreSQL and broader hardware coverage are environment-gated | Dedicated, auditable integration runners |

## Open Maintainability Work

- Replace string-based generic type substitution.
- Cache raw source indexes by LSP document revision; the checked semantic
  database intentionally stores semantic/desugared programs.
- Continue migrating runtime APIs from `OperationContext` to a mandatory
  injected execution context.
- Split LSP, REIR, analyzer, lowering, runtime services, and VM/JIT by
  invariant.
- Reduce broad public re-exports before declaring API stability.
- Replace remaining global registries with explicit owner/session lifetimes.

These are not reported as correctness fixes until executable invariants and
regression tests exist.

## Refactoring Execution

This table records the current implementation batch for the refactoring work in
[the roadmap](roadmap.md). It is a state summary, not a chronological ledger.

| Batch | Scope | Status | Exit condition |
| --- | --- | --- | --- |
| R0 | Architecture dependency guards and behavior baselines | Complete | CI rejects forbidden dependency directions and current contract suites remain green |
| R1 | Complete package/dependency snapshot before review or execution | Complete | Check, review, lower, build, publish, and vendor consume one immutable graph |
| R2 | `SourceSnapshot`, frontend budget, semantic database, and `ValidatedProgram` | Complete | Review, lowering, VM, and LSP consume one bounded frontend result; executable backends require validated checked facts |
| R3 | Mandatory `ExecutionContext` and scoped host capabilities | Not started | Restricted execution cannot reach ambient filesystem, network, process, or database authority |
| R4 | LSP, REIR, runtime, analyzer, package-native, VM, and JIT decomposition | Not started | Modules are split around tested state transitions without behavior changes |
| R5 | Public API contraction and explicit facades | Not started | Broad glob exports and duplicate compatibility entrypoints are removed |
| R6 | Out-of-process native, JIT, and GPU execution | Not started | Untrusted execution uses killable workers with bounded IPC and OS policy |

Update this table in the same commit that changes a batch state. Do not create a
separate dated progress report.

R1 captures the complete development dependency graph before direct check,
review, lock, tree, CLI, register-VM, native-loader, publish, or vendor work
begins. Internal captured entrypoints consume only that graph for their full
lifetime. Public results map diagnostics and package identities back to checkout
paths, and regression tests cover later source mutation and absolute path
dependencies without exposing private snapshot paths.

R2 makes one frontend run own immutable source/interface bytes, parsed
per-file semantic programs, the merged namespace-isolated program, checked HIR,
diagnostics, and a typed completion state. `ValidatedProgram` is constructible
only from a complete result without error diagnostics. Register-VM compilation
consumes checked HIR directly; Rust lowering and package review reuse the same
parsed programs and no longer reparse built-in interfaces. LSP diagnostics use
the same result API. Code-generation-only declaration projections remain AST
projections, not an independent semantic checker; raw source indexes remain an
R4 LSP decomposition concern.

## Experimental Status

- Native JIT and Metal have dedicated path-triggered, nightly, and release
  validation. They are not Core.
- Self-hosting proves substantial lexer/parser/checker parity but is not an
  independent compiler or release requirement.
- Package publish remains a dry-run validation surface, not a hosted registry.
- True multi-isolate execution, general ML scheduling, and declarative rewrite
  systems are research, not committed product surface.

## Documentation Policy

This file replaces dated remediation ledgers and checked-in review reports.
When an item closes, update the relevant row and its tests in the same change.
Do not add a new dated status file.
