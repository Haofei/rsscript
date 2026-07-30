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
| Windows | Secure cache ACL and atomic Job attachment remain incomplete | SID/DACL validation and suspended process launch |
| Isolated execution portability | Verified worker launch is Linux/bubblewrap only; Metal has no verified macOS isolation backend | Add audited platform launchers without weakening fail-closed policy |
| Host authority | Adapters outside the register VM still accept some paths, URLs, commands, and DSNs as authority | Root/endpoint/executable/database capability handles |
| Capability evidence | Some native capability facts are author declarations | Independent verification and provenance |
| External integrations | Live PostgreSQL and broader hardware coverage are environment-gated | Dedicated, auditable integration runners |

## Open Maintainability Work

- Cache raw source indexes by LSP document revision; the checked semantic
  database intentionally stores semantic/desugared programs.
- Replace remaining global registries with explicit owner/session lifetimes.
- Extend isolated workers to audited Windows and macOS launchers without
  treating process limits or an ordinary container as equivalent isolation.

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
| R3 | Mandatory `ExecutionContext` and scoped host capabilities | Complete | Restricted execution cannot reach ambient filesystem, network, process, or database authority |
| R4 | LSP, REIR, runtime, analyzer, package-native, VM, and JIT decomposition | Complete | Modules are split around tested state transitions without behavior changes |
| R5 | Public API contraction and explicit facades | Complete | Broad glob exports and duplicate compatibility entrypoints are removed |
| R6 | Out-of-process native, JIT, and GPU execution | Complete | Untrusted execution uses killable workers with bounded IPC and OS policy |
| R7 | Interned structural type semantics | Complete | Semantic signatures and fields use shared structural facts; generic substitution no longer depends on parameter-name heuristics |
| R8 | VM/JIT invariant boundary decomposition | Complete | Validation, executable memory, ABI, optimization, and deoptimization boundaries are independently testable |
| R9 | Explicit service ownership and session lifetimes | Complete | Stateful runtime and native services have explicit owners, instance isolation, and deterministic close/shutdown paths |
| R10 | Opaque host capability handles | Pending | Restricted APIs no longer accept ambient paths, endpoints, executables, or database authority |
| R11 | REIR adapter convergence | Complete | Adapters share bounded evidence construction, provenance, and explicit unknown coverage |
| R12 | Test-domain organization | Pending | Large test aggregations are split by semantic domain without reducing coverage |

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

R3 gives every register-VM instance an explicit execution context and a unique
scope identity. Legacy embedding helpers construct a named trusted-local
context; restricted callers use the context-aware entrypoint. Every
host-touching intrinsic is classified and checked before dispatch. Trusted-CI
VM execution defaults to no host grants, can run pure bounded code, and denies
filesystem, environment, process, network, database, native, JIT, and GPU
effects before side effects occur. Capability objects support exact grants, but
the VM remains deliberately conservative: a restricted intrinsic stays denied
until that intrinsic validates its concrete resource through the scoped API.
Rust AOT remains denied outside `LocalTrusted`; untrusted execution is available
only through the R6 proof-gated worker entrypoints.

R4 turns the largest orchestration files into composition roots and invariant
owners without changing public behavior. LSP now separates documents, text,
workspace loading, scheduling, publication, diagnostics, features, and protocol
adaptation. REIR separates CLI I/O/rendering/bundle operations from indexed
reconciliation model, matching, and engine code. Runtime separates structured
data, network policy, and process policy/environment/capture/supervision.
Analyzer task-group traversal, lowering declaration/projection passes,
package-native bindings, native-loader shim/cache, VM tier admission/scratch/
recursion, and JIT analysis/executable-memory accounting each have dedicated
modules. Architecture tests prevent the composition roots and stateful types
from collapsing back into the previous monoliths.

R5 makes public surface growth explicit. `rsscript::api::v1` groups frontend,
diagnostics, review, package, and register-VM entrypoints and removes duplicate
VM compatibility names. `reir::api::v1` groups model, decision,
reconciliation, and rendering APIs while the crate root retains only the
minimal compatibility set used by RSScript. `rsscript-runtime` replaces
blanket exports with a generated-code ABI manifest plus curated `api::v1`,
`host`, and `net` facades; resource-aware network calls use one
`OperationContext`. Architecture tests reject new root glob exports and the
removed aliases. These versioned namespaces control API growth but do not
declare `0.1.x` SemVer stability.

R6 adds a dependency-neutral, versioned, length-bounded worker protocol and a
single-request execution worker for the reference VM, native JIT, digest-pinned
native ABI calls, and Metal operations. The host client validates request and
response identities, preserves complete VM output, bounds process stderr,
enforces a wall deadline, and kills the guarded process tree on every failure.
`UntrustedIsolated` cannot construct an in-process context and has no fallback.
On Linux, workers launch through a verified root-owned bubblewrap binary with
new user/PID/IPC/UTS/network namespaces, no capabilities or environment, a
private filesystem, explicit read-only inputs, and strict process limits.
Unsupported launchers and platforms fail closed. Metal transport and worker
dispatch are complete, but untrusted Metal execution remains unavailable until
an equivalent verified macOS launcher exists.

R7 adds an interned `TypeId`/`ResolvedType` arena owned by
`SemanticDatabase`. Function signatures and declared fields are captured once
from checked syntax and shared with validated Rust lowering. HIR generic
inference now performs recursive substitution over structural types, including
arbitrary declared parameter names such as `U` and `W`; rendered type strings
remain compatibility projections at diagnostics and emission boundaries.

R9 introduces explicit owners for SQLite and SQLx adapters, native-library
loading, Metal dispatch, process concurrency, HTTP, and the Tokio-backed runtime
services. Instance APIs own caches, limits, and shutdown; compatibility free
functions delegate to a default instance but no longer contain the core state
machine. `OperationContext` can carry an explicit `RuntimeServices` owner, so
embedded executions and tests can use independent lifecycle and policy state.

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
