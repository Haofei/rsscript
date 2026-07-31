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
  dedicated JIT, ABI, and process crates.
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
- Third-party packages stop at static check, review, semantic diff, and REIR
  evidence. The project has no untrusted package execution profile or worker.

These controls reduce mistakes and denial-of-service exposure. They do not turn
host execution into isolation.

## Open Security And Correctness Work

| Area | Current limitation | Required closure |
| --- | --- | --- |
| Windows artifact integrity | Secure cache ACL validation remains incomplete for trusted native artifacts | SID/DACL validation or fail-closed cache disablement |
| Host authority | Trusted-local compatibility APIs outside the register VM still accept raw paths, URLs, commands, and DSNs | Migrate hosted adapters to `ScopedHostAdapters`; do not expose raw compatibility APIs to restricted execution |
| Capability evidence | Some native capability facts are author declarations | Independent verification and provenance |
| External integrations | Live PostgreSQL and broader hardware coverage are environment-gated | Dedicated, auditable integration runners |

## Open Maintainability Work

- Replace remaining global registries with explicit owner/session lifetimes.

These are not reported as correctness fixes until executable invariants and
regression tests exist.

## Refactoring Execution

This table records the current implementation batch for the refactoring work in
[the roadmap](roadmap.md). It is a state summary, not a chronological ledger.

| Batch | Scope | Status | Exit condition |
| --- | --- | --- | --- |
| R0 | Architecture dependency guards and behavior baselines | Complete | CI rejects forbidden dependency directions and current contract suites remain green |
| R1 | Complete package/dependency snapshot before review or execution | Complete | Check, review, lock, tree, lower, and build consume one immutable graph |
| R2 | `SourceSnapshot`, frontend budget, semantic database, and `ValidatedProgram` | Complete | Review, lowering, VM, and LSP consume one bounded frontend result; executable backends require validated checked facts |
| R3 | Mandatory `ExecutionContext` and scoped host capabilities | Complete | Restricted execution cannot reach ambient filesystem, network, process, or database authority |
| R4 | LSP, REIR, runtime, analyzer, package-native, VM, and JIT decomposition | Complete | Modules are split around tested state transitions without behavior changes |
| R5 | Public API contraction and explicit facades | Complete | Broad glob exports and duplicate compatibility entrypoints are removed |
| R6 | Former out-of-process execution experiment | Superseded | Removed from the supported product and codebase by R20 |
| R7 | Interned structural type semantics | Complete | Semantic signatures and fields use shared structural facts; generic substitution no longer depends on parameter-name heuristics |
| R8 | VM/JIT invariant boundary decomposition | Complete | Validation, executable memory, ABI, optimization, and deoptimization boundaries are independently testable |
| R9 | Explicit service ownership and session lifetimes | Complete | Stateful runtime and native services have explicit owners, instance isolation, and deterministic close/shutdown paths |
| R10 | Opaque host capability handles | Complete | Concrete host authorization returns execution-scoped path, endpoint, executable, and database handles |
| R11 | REIR adapter convergence | Complete | Adapters share bounded evidence construction, provenance, and explicit unknown coverage |
| R12 | Test-domain organization | Complete | Register-VM, JIT acceptance, and self-host parity suites are composed from independently owned semantic domains |
| R13 | Runtime compatibility owner isolation | Complete | Canonical async work uses explicit services and the generated ABI has exactly one isolated process-wide compatibility owner |
| R14 | Register-VM execution boundary decomposition | Complete | Tier selection, execution planning, lowering, and interpretation have independently testable owners |
| R15 | REIR adapter pipeline decomposition | Complete | Input, traversal, normalization, coverage, and fact projection are separate from bounded evidence construction |
| R16 | Semantic checker and lowering decomposition | Complete | Call, ownership, effect, closure, and emission responsibilities have stable module boundaries |
| R17 | Large test-domain decomposition | Complete | Remaining register-window, JIT, and backend suites have independently owned semantic domains |
| R18 | Host capability adapter enforcement | Complete | Canonical filesystem, network, process, and database adapters consume scoped authorized handles |
| R19 | Revision-scoped LSP index cache | Complete | Semantic indexes are reused only for matching document and package generations |
| R20 | Remove third-party safe-execution scope | Complete | No untrusted profile, worker protocol, worker binary, sandbox launcher, release artifact, or execution API remains |
| R21 | Remove Metal/GPU and tensor execution surfaces | Complete | No Metal crate, tensor runtime, GPU ABI, language interface, VM lowering, test domain, or release job remains |
| R22 | Remove package publish, vendor, and hosted-registry preview surfaces | Complete | No publish/vendor CLI, RSScript API/model, REIR collector, preview badge, archive-manifest, fixture, or package-manager prototype surface remains |
| R23 | Contract native package ecosystem demos | Complete | One dependency-free native ABI fixture covers mutable list write-back; SQLite, SQLx, Rayon, CLI, Crypto, and HTTP native package trees are removed |
| R24 | Remove fake database runtime and generic pooling | Complete | No synthetic database connection/error runtime, generic pooling language feature, compiler/VM model, stdlib interface, fixture, or generated crate remains |
| R25 | Remove simulated domain runtime facades | Complete | No bundled image, cache, config, counter, interpreter-object, or local HTTP handler facade remains; the real HTTP client, File/JSON/CSV, JIT, and self-host framework remain |

Update this table in the same commit that changes a batch state. Do not create a
separate dated progress report.

R1 captures the complete development dependency graph before direct check,
review, lock, tree, CLI, register-VM, or native-loader work begins. Internal
captured entrypoints consume only that graph for their full
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
projections, not an independent semantic checker; R19 adds revision-scoped
source-index reuse without changing that semantic ownership boundary.

R3 gives every register-VM instance an explicit execution context and a unique
scope identity. Legacy embedding helpers construct a named trusted-local
context; restricted callers use the context-aware entrypoint. Every
host-touching intrinsic is classified and checked before dispatch. Trusted-CI
VM execution defaults to no host grants, can run pure bounded code, and denies
filesystem, environment, process, network, database, native, and JIT
effects before side effects occur. Capability objects support exact grants, but
the VM remains deliberately conservative: a restricted intrinsic stays denied
until that intrinsic validates its concrete resource through the scoped API.
Rust AOT remains denied outside `LocalTrusted`. No untrusted execution entrypoint
exists.

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

R6 historically introduced an isolated execution experiment. R20 supersedes
that batch and removes its protocol, worker, launcher, public authorization
surface, CI jobs, and release artifact. It is retained in this table only so
existing Git history and old review references remain understandable.

R7 adds an interned `TypeId`/`ResolvedType` arena owned by
`SemanticDatabase`. Function signatures and declared fields are captured once
from checked syntax and shared with validated Rust lowering. HIR generic
inference now performs recursive substitution over structural types, including
arbitrary declared parameter names such as `U` and `W`; rendered type strings
remain compatibility projections at diagnostics and emission boundaries.

R8 makes `vm-jit/lib.rs` a composition root over host ABI, public IR, sealed
validation proofs, analysis, code generation, deoptimization, module ownership,
and executable-memory accounting. Register-VM native passes are likewise split
by intrinsic facts, semantics, region optimization, scalar replacement, and
inlining. Raw IR cannot reach code generation without a mode-specific borrowed
`ValidatedJitFunction`.

R9 introduced explicit owners for native-library loading, process concurrency,
HTTP, database access, and the Tokio-backed runtime services. Instance APIs own
caches, limits, and shutdown; compatibility free functions delegate to a default
instance but no longer contain the core state machine. `OperationContext` can
carry an explicit `RuntimeServices` owner, so embedded executions and tests can
use independent lifecycle and policy state. R23 later removes the checked-in
native database and HTTP package implementations without weakening those
generic runtime ownership boundaries.

R10 replaces successful concrete authorization results with opaque
`AuthorizedPath`, `AuthorizedEndpoint`, `AuthorizedExecutable`, and
`AuthorizedDatabase` values. Each handle carries the unforgeable
`ExecutionScopeId` that issued it, and cross-scope reuse is rejected. Restricted
VM host effects remain fail-closed until their adapter accepts the corresponding
handle; trusted-local compatibility does not weaken restricted construction.

R11 routes RSScript and Terraform evidence through one bounded builder with
operation, fact, and string budgets. Producer provenance is validated at the
construction boundary, and unsupported Terraform resources produce explicit
unknown coverage instead of disappearing from the evidence set.

R12 leaves the test entrypoints as short composition roots. Register-VM tests
are divided into registry, resource, register-window, closure-cache, and
profiling domains; self-host parity is divided by compiler phase; JIT acceptance
is divided into core, optimization, and resource-limit contracts. Architecture
tests prevent those aggregators from growing back into monoliths.

R13 removes the second process-wide Tokio runtime and isolates the sole default
`RuntimeServices` owner in the generated-ABI compatibility module. Canonical
operations retain explicit services through `OperationContext`; runtime tests
create and shut down their own owners. An architecture test prevents global
runtime ownership from returning to the async state machine.

R14 separates static JIT eligibility planning, profiling facts, DeepCopy
taint/escape analysis, closure-capture lowering, register storage accounting,
OSR planning/materialization, and native compile telemetry from the Register VM
composition roots. Architecture tests pin those owners so later tier or
interpreter changes cannot silently recombine their state machines.

R15 decomposes the RSScript and Terraform adapters into explicit input,
normalization, traversal, fact, coverage, provenance, and pipeline stages.
Terraform owns a separate budget stage. The shared `BoundedEvidenceBuilder`
remains the only bundle-construction boundary, unsupported resources remain
explicit unknown coverage, and exhausted budgets fail before returning partial
evidence. Architecture tests prevent adapter monoliths and direct bundle
construction from returning.

R16 separates call generic constraints, closure contracts, and type
compatibility; local ownership, resource escape, and control-flow state; HIR
signature, body, effect, and structural-type lowering; and Rust program,
expression, ownership, intrinsic, and structural-type emission.
`SemanticTypeFacts` remains the source for structured lowering, so module
decomposition does not reintroduce display-string semantic derivation.

R17 turns the remaining register-window and VM-JIT test monoliths into
composition roots. Register-VM coverage is grouped by lowering, translation,
tiering/memoization, ABI/heap behavior, OSR collections, closures, and
deoptimization/transactions. VM-JIT coverage is grouped by host memoization,
calls/ABI, deoptimization, validation, fuzzing, range proofs, and the sealed
compile boundary. Architecture tests pin both domain sets.

R18 introduces `ScopedHostAdapters` as the sole consumption boundary for
`AuthorizedPath`, `AuthorizedEndpoint`, `AuthorizedExecutable`, and
`AuthorizedDatabase`. Restricted Register-VM dispatch now derives the concrete
path, URL endpoint, executable, database identity, process working directory,
and process environment names before any host effect; it mints an exact
scope-bound handle and passes that handle through the adapter. Cross-scope
handles fail closed. Existing stream/file resources remain valid only inside
their creating VM, while trusted-local raw entrypoints remain explicitly
compatibility-only.

R19 adds an immutable source-index cache keyed by document revision and package
semantic generation. Editing, desynchronizing, saving, or invalidating package
inputs advances the relevant cache identity; work started against an old
generation cannot publish back into the cache. Hover, navigation, references,
rename, call hierarchy, workspace symbols, and semantic tokens share the same
index while diagnostics retain their existing cancellation and debounce model.

R20 contracts execution to operator-controlled source. The
`UntrustedIsolated` deployment profile, isolated execution library API, worker
wire protocol, worker binary, process-guard sandbox launcher, CI jobs, and
release assets were deleted. `LocalTrusted` remains the normal execution mode;
`TrustedCI` remains a deny-all-host pure-VM convenience for reviewed
organization repositories. Third-party packages are static-analysis inputs
only.

R21 removes the Metal/GPU and tensor product surface rather than carrying it as
an experimental compatibility layer. The Metal workspace crate, tensor runtime,
generated ABI hooks, language interfaces, Register-VM intrinsics and lowering,
parity suites, manifests, documentation, and CI/release jobs were deleted.
RSScript no longer claims hardware-accelerated tensor execution as part of its
review-first language scope.

R22 removes package publication, package vendoring, and hosted-registry previews
from the supported product. RSScript retains registry dependency grammar,
explicit unresolved registry graph nodes, lock source identity, native Cargo
registry lock/vendor validation, generic REIR `RegistryMetadata` and
`PublishedAs`, native ABI registries, the core package index, and snapshot
exclusions for `vendor` directories. Metadata dry-run and lock/tree/metadata
REIR remain the package evidence surfaces.

R23 contracts the checked-in native package ecosystem to
`packages/native-abi-fixture`, a dependency-free Rust crate with one
deterministic mutable-list operation. It preserves package checking, native
binding generation, dynamic ABI dispatch, review gating, and `mut List<Int>`
write-back coverage without shipping database, parallelism, CLI, cryptography,
or HTTP demo adapters.

R24 removes the synthetic executable database connection model and the generic
pooling language/runtime feature. Core `resource` declarations and `with`
ownership remain, as do the abstract `database.read`/`database.write`
capability taxonomy and Terraform/PostgreSQL review evidence demos.

R25 removes the simulated image, cache, config/rule, counter,
environment/function-object, and local HTTP handler runtime facades. The real
HTTP client remains backed by `HttpRequest`, `HttpResponse`, and `HttpError`;
File, JSON, CSV, JIT, and the self-host framework retain their supported
coverage.

## Experimental Status

- Native JIT has dedicated path-triggered, nightly, and release validation. It
  is not Core.
- Self-hosting proves substantial lexer/parser/checker parity but is not an
  independent compiler or release requirement.
- True multi-isolate execution and declarative rewrite
  systems are research, not committed product surface.

## Documentation Policy

This file replaces dated remediation ledgers and checked-in review reports.
When an item closes, update the relevant row and its tests in the same change.
Do not add a new dated status file.
