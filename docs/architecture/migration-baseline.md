# Architecture migration baseline

This document freezes the behavior and dependency baseline used while RSScript
moves semantic ownership, executable IR, bytecode code generation, and VM
responsibilities. It is a migration guardrail, not a release plan.

## Scope freeze

Until the migration exit criteria below are met, Core accepts correctness,
security-boundary, conformance, diagnostics, and measured-regression changes.
Core does not add language syntax, qualifiers, public intrinsics, official
Providers, JIT tiers or speculation, AOT/native surfaces, package publishing,
language-level policy, or a registry.

The authoritative package maturity inventory is
[`workspace-tiers.toml`](workspace-tiers.toml). Architecture tests require every
workspace package to occur in exactly one tier and require only Core,
applications, and the reference runner to be root default members.

## Migration invariants

The following are mechanical exit conditions, not architectural aspirations:

1. Syntax does not depend on semantics, runtime, Providers, or review.
2. Semantic validation does not depend on a runtime, concrete Provider, review,
   JIT, or AOT implementation.
3. HIR remains source-shaped; the future MIR is typed, owned, CFG-shaped, has no
   syntax dependency, has no unresolved symbol identity, and has no `Unknown`
   execution node.
4. Compiler code generation does not depend on the VM interpreter.
5. The VM accepts only a verifier-created program and does not depend on syntax,
   HIR, semantic databases, package loading, or compiler orchestration.
6. The SDK exposes an explicit reviewed façade; it must not acquire new root
   glob exports from implementation crates.
7. Provider replacement cannot alter compiled Artifact bytes.
8. Analysis, Artifact, and semantic diff carry the same snapshot/module
   identity.
9. Existing and replacement execution paths remain differential-tested until
   the old path is deleted.
10. Experiments consume stable Core contracts and cannot add state to Core VM
    program types.

## Behavior preservation baseline

| Contract | Existing guard | Migration rule |
| --- | --- | --- |
| Source diagnostics | `static`, semantic property, hostile and fuzz corpora | Diagnostic code/span digest changes require an intentional fixture update |
| Source to Artifact | schema contracts and `migration_baseline` | Canonical bundle digest changes require an intentional fixture update |
| Artifact verification | bytecode properties, malformed corpus and fuzz targets | Unverified bytes never enter execution |
| VM behavior | runtime, VM parity, differential and soak suites | New and old lowering paths must produce equivalent reports |
| Cancellation and budgets | hostile, JIT acceptance, runtime and Core metrics | Termination reason and cleanup behavior cannot regress |
| Provider boundary | Provider conformance and replacement demo | Signature mismatch fails before execution |
| Runtime telemetry | execution report schema and Core metrics | Telemetry remains observational and redacted by policy |
| Determinism | package/schema tests and canonical Artifact encoding | Same snapshot must produce byte-identical bundle bytes |

`benchmarks/core/slo.v1.json` remains the performance regression envelope. It is
not a release gate and does not justify JIT expansion; it protects check,
compile, verify, execute, Provider-call, cancellation, and Artifact-size
baselines during internal refactoring.

## Current asset ownership

| Asset | Current owner | Migration disposition |
| --- | --- | --- |
| Parser/CST/AST | `rsscript-syntax` | Keep |
| Immutable snapshots, semantic database and validation phase types | `rsscript-semantics` | Migrated; compiler only assembles them through the analyzer boundary |
| Analyzer orchestration and most checks | `rsscript-compiler` | Move remaining semantic checks and queries to `rsscript-semantics` |
| Typed HIR model | `rsscript-semantics` | Keep source-shaped |
| Owned executable IR | `rsscript-exec-ir` | Transitional; replace source-shaped nodes with typed CFG MIR |
| HIR projection | `rsscript-lowering` | Evolve into HIR-to-MIR lowering |
| VM bytecode emission | `rsscript-vm` | Move to a codegen boundary after MIR exists |
| Artifact envelope/verifier | `rsscript-bytecode` | Keep; evolve through a versioned typed wire model |
| Interpreter, limits, scheduler | `rsscript-vm` | Keep only verified execution responsibilities |
| Dynamic Provider ABI/linking | `rsscript-provider-api` | Keep; tighten wire values and resource handles later |
| Stable embedding path | `rsscript-sdk` | Shrink to explicit phase APIs before public compatibility promises |
| Package capture and persistence | `rsscript-compiler` plus workspace loader | Move OS/persistence concerns out of compiler core |
| AOT/JIT/native/REIR/selfhost | Experimental/Integration/Research tiers | Frozen except correctness and differential value |

## Review convergence TODO

This is the single execution checklist for the architecture review. A checked
item must be backed by code and a mechanical guard; moving a file, adding a
crate, or documenting an intended boundary is not completion by itself.
Unchecked items remain open even when preparatory work exists. The order below
is the required dependency order unless a correctness or security fix must land
first.

### 0. Freeze and migration guardrails

- [x] **G01 — Classify every workspace package by maturity.**
  `workspace-tiers.toml` is exhaustive and architecture tests reject missing or
  duplicate classifications.
- [x] **G02 — Keep the supported path as the default Cargo build.** Root
  `default-members` contain only Core, applications, and the reference runner.
- [x] **G03 — Split Core and experimental CI feedback.** Core is the blocking
  default gate; experimental, JIT, and self-host checks use separate workflows.
- [x] **G04 — Remove disabled-code cemetery blocks.** CI rejects new
  `#[cfg(any())]` Rust code.
- [x] **G05 — Freeze behavior baselines.** Diagnostics, canonical Artifact,
  execution-report, cancellation, Provider, and Core SLO fixtures protect the
  migration.
- [ ] **G06 — Physically isolate experiments from the Core workspace.** Move
  JIT, AOT, native ABI, REIR, self-host, C/research fixtures, and test generation
  to an experiments workspace or repository. Completion means ordinary Core
  dependency resolution and release metadata do not include those packages.
- [ ] **G07 — Establish public API compatibility gates.** Check in a reviewed
  SDK API inventory, run semver/API-diff checks in CI, and reject experimental
  symbols from default SDK features.

### 1. Semantic ownership and query boundary

- [x] **S01 — Move semantic phase types to `rsscript-semantics`.** Immutable
  source snapshots, `SemanticDatabase`, completion state, `AnalysisResult`, and
  `ValidatedProgram` are owned there; architecture tests prevent compiler
  re-ownership.
- [ ] **S02 — Move name/type/call checks into semantics.** Migrate analyzer,
  ownership, retention, resource, task-group, call-binding, exhaustiveness, and
  type checks out of `rsscript-compiler`. Completion means compiler only
  orchestrates semantic queries and contains no semantic rule implementation.
- [ ] **S03 — Add one `CompilationSession` query boundary.** Introduce stable
  source/module/interface/definition/type identities, dependency tracking,
  cancellation, deadlines, and cached HIR/MIR queries shared by CLI, package
  compilation, tests, and editor tooling.
- [ ] **S04 — Make language service consume semantic queries directly.** It
  must not depend on the compiler compatibility façade, package persistence,
  VM, SDK, or Providers; revision invalidation and request cancellation require
  focused tests.
- [ ] **S05 — Finish compiler purity.** Compiler input is an explicit immutable
  `SourceSet`/`WorkspaceSnapshot`; package traversal, filesystem locking,
  temporary files, compression, Artifact persistence, review/risk, and Rust AOT
  lowering live outside the compiler dependency closure.

### 2. Typed CFG MIR

- [ ] **M01 — Define typed stable identities.** Add `FunctionId`, `TypeId`,
  `BlockId`, `ValueId`, `PlaceId`, `BuiltinId`, `ExternalSymbolId`, and
  `ResourceTypeId` without string identity at backend boundaries.
- [ ] **M02 — Define an owned CFG MIR.** Functions contain basic blocks,
  instructions, and terminators; MIR does not depend on syntax and contains no
  unresolved or `Unknown` execution node.
- [ ] **M03 — Make semantic operations explicit.** MIR represents move,
  read/mut borrow, retain, drop, resource acquire/release, spawn, await, join,
  cancellation, selection, external calls, and every cleanup/unwind edge.
- [ ] **M04 — Lower checked HIR to MIR exactly once.** Backend code cannot
  inspect syntax AST or reconstruct semantic facts. MIR verification rejects
  unresolved calls, invalid ownership state, incomplete cleanup, and malformed
  structured-task scopes.
- [ ] **M05 — Run old/new lowering differentially.** The same corpus must
  produce equivalent diagnostics, external imports, termination reasons,
  values, cleanup behavior, and deterministic usage reports.
- [ ] **M06 — Delete the source-shaped executable IR.** Remove nested
  `If`/`For`/`Match`/`With` backend nodes, string type/callee identities, and
  `ExecutableStmt::Unknown`/`ExecutableExpr::Unknown` only after M05 passes.

### 3. Code generation, verifier, and VM boundary

- [x] **V01 — Remove compiler-to-VM dependency.** Cargo architecture tests
  reject compiler dependencies on the VM and the VM cannot depend on compiler,
  syntax, semantics, or lowering internals.
- [ ] **V02 — Extract `rsscript-codegen-vm`.** The sole bytecode-emission path is
  `VerifiedMir -> BytecodeModule`; source, HIR, package, and SDK entry points are
  forbidden in the codegen crate.
- [ ] **V03 — Make the verifier construct the only executable program type.**
  Untrusted bytes decode and verify to a private-field `VerifiedModule`; public
  APIs cannot construct or mutate it and VM constructors accept nothing else.
- [ ] **V04 — Make the VM execution-only.** Remove MIR/executable-IR lowering,
  bytecode encoding, Artifact packaging, compiler/source helpers, and duplicate
  payload verification from `rsscript-vm`.
- [ ] **V05 — Remove experimental state from Core VM program objects.** JIT,
  OSR, deopt, branch/call profiles, and native tier state live in experiment-owned
  side tables keyed by stable function IDs.
- [ ] **V06 — Split VM primitives from deterministic core library.** VM Core
  keeps frames, registers, scheduler, cancellation, resource table, limits,
  dispatch, and external calls. JSON/YAML, regex, compression, encoding, hashes,
  date utilities, and collection algorithms move behind a versioned builtin
  registry or Core library runtime.
- [ ] **V07 — Classify the intrinsic catalog.** Every entry is exactly one of a
  VM primitive, deterministic builtin, or Provider external symbol; adding a
  library API must not silently change the VM instruction set.

### 4. Bytecode and compatibility contracts

- [x] **B01 — Establish a bounded sectioned Artifact envelope.** Required and
  optional sections, canonical ordering, length/count limits, hashes, checksum,
  unknown-section handling, malformed corpora, and fuzz coverage are present.
- [ ] **B02 — Define the typed bytecode v2 wire model.** Use bounded decoding,
  numeric opcodes, numeric IDs, fixed operand layouts, and separate type,
  constant, function, import, export, code, and optional debug tables.
- [ ] **B03 — Generate codec and verification rules from one instruction
  schema.** The schema generates Rust instruction types, encoder, bounded
  decoder, operand validation, documentation, and fuzz seeds; string field maps
  and `serde_json::Value` verification are removed.
- [ ] **B04 — Separate all compatibility versions.** Container format, language
  semantics, bytecode ISA, Core library ABI, Provider ABI, analysis schema, and
  compiler provenance have explicit independent values. Language compatibility
  must not be inferred from `CARGO_PKG_VERSION`.
- [ ] **B05 — Preserve a versioned compatibility corpus.** Keep read-only v1
  fixtures, malformed v1/v2 inputs, N-1 schema fixtures, deterministic
  cross-platform bytes, and explicit unknown-version/section fail-closed tests.

### 5. Provider contract and authoring SDK

- [x] **P01 — Use structured external signatures.** Artifact and Provider
  contracts use canonical `WireType`, data effects, retention, async shape,
  signature hashes, and an explicit runtime ABI.
- [x] **P02 — Carry runtime context into Provider calls.** Cancellation,
  monotonic deadline, byte/output budgets, call identity, trace sink, authority,
  blocking/async lanes, and runtime-owned resource registration reach the
  callable; errors are structured.
- [x] **P03 — Use generation-safe resource handles.** Runtime resource tables
  reject stale handles and report created, cleaned, live, peak, and cleanup
  failures.
- [ ] **P04 — Make semantic interface descriptors the bindgen input.** `.rssi`
  is parsed and canonicalized once by the semantic compiler into a versioned
  `InterfaceDescriptor`; bindgen must not duplicate syntax/type normalization.
- [ ] **P05 — Generate typed Rust Provider APIs.** Generate sync/async traits,
  typed parameters/results, resource wrappers, descriptor/signature constants,
  registration glue, mocks, completeness checks, and conformance skeletons.
  `NativeValue` remains only in generated adapters.
- [ ] **P06 — Tighten the canonical wire value model.** Replace JSON, string
  type/field identity, and generic `Native { type_name, id }` escape hatches
  with typed records, variants, lists, resources, and generation-safe handles;
  JSON becomes an explicitly declared extension codec.
- [ ] **P07 — Remove policy-shaped authority from Core ABI.** Rename it to a
  neutral host-call context or move authority scopes to runner/provider profiles;
  Core reports required symbols but does not interpret authorization policy.
- [ ] **P08 — Complete async/resource conformance.** Test cancellation during
  suspension, deadline expiry, blocking-lane enforcement, cleanup exactly once
  on success/error/cancel/deadline, reentrancy, panic containment, redaction, and
  request/response limits.

### 6. Stable SDK and product workflows

- [x] **A01 — Establish phase-typed execution.** The supported path is immutable
  snapshot/build bundle -> verify -> link -> bounded execution report, with
  verification and link errors kept distinct.
- [x] **A02 — Remove SDK root glob re-exports.** Implementation crates cannot
  silently add SDK public symbols.
- [ ] **A03 — Shrink the SDK to reviewed façade modules.** Default public API is
  limited to compiler/check, Artifact/verification, Provider registration,
  runtime/linking, execution request/limits/report, diagnostics, and operation
  control. Package review/risk, AOT, JIT/OSR, register VM, opcode, and legacy
  convenience APIs are not re-exported.
- [ ] **A04 — Remove invalid phase states and report-losing paths.** Public types
  do not use optional fields to represent incompatible phases; script,
  Provider, cancellation, deadline, and budget termination always return a full
  execution report. Only host/protocol/internal-invariant failures use outer
  errors.
- [x] **A05 — Make execution bounded by default.** Unbounded execution requires
  an explicitly named trusted-host constructor; per-run limits live on the
  execution request.
- [x] **A06 — Ship Artifact Bundle, `rss verify`, and neutral `rss diff`.** Both
  single-file and package builds produce analysis/provenance-bound bundles.
- [ ] **A07 — Complete semantic diff evidence.** Add read/mut/take, retention and
  escape, resource acquire/transfer/cleanup, structured-task fan-out and
  cancellation, call graph/recursion, Provider requirements, and diagnostic
  additions/removals while remaining policy-neutral.
- [x] **A08 — Run scripts out of process by default.** `rss run` uses the
  versioned child protocol; trusted in-process execution is explicit.
- [ ] **A09 — Harden the reference Linux runner profile.** Add allowlisted
  Provider profiles, namespace/syscall/filesystem/network controls where
  available, parent-enforced kill-on-deadline, protocol/disconnect fuzzing, and
  tests separating runner termination from VM termination. Continue to state
  that this is defense in depth rather than a universal sandbox.

### 7. Adoption, evidence, and maintenance

- [x] **E01 — Gate representative Core performance.** CI records check,
  compile, Artifact verify, VM, Provider boundary, cancellation, Artifact size,
  and deterministic usage metrics against the checked SLO fixture.
- [ ] **E02 — Add two complete product examples.** Keep the embedded Provider
  replacement pipeline and add a reviewable async/resource workflow; each must
  contain source, interfaces, generated Provider contract, memory and
  production-like Providers, Artifact identity, semantic-diff fixture, and
  success/failure reports for trusted and isolated execution.
- [ ] **E03 — Establish compatibility and conformance corpora.** Add source to
  diagnostic/HIR/MIR goldens, MIR to bytecode fixtures, old Artifact readers,
  cross-platform deterministic builds, Provider ABI compatibility, resource
  cleanup state machines, and interpreter/experimental-backend differential
  tests.
- [ ] **E04 — Require ADR/RFC records for contract changes.** Language semantics,
  MIR, bytecode ISA, Provider ABI, Artifact format, and stable SDK changes must
  state problem, non-goals, compatibility, migration, verifier/security impact,
  and Provider/backend impact.
- [ ] **E05 — Add opt-in deterministic Provider record/replay.** This remains P2
  until Core boundaries are stable and must define replayability, normalization,
  redaction, external-state dependence, and persistence rules without claiming
  a security proof.

### Explicitly deferred

Release publication, crates.io distribution, registry/publish workflows, new
language syntax, new qualifiers, new public intrinsics, new official Providers,
new execution backends, new JIT tiers/speculation, full self-hosting, C backend
coverage, and built-in AI/Agent frameworks are not part of this TODO. They stay
frozen until the unchecked Core items above are complete and a separate product
decision reopens them.

## Exit criteria for this preparation phase

- Root default commands select only Core, applications, and the runner.
- Full `--workspace --all-features` maintenance tests remain available.
- CI has separate Core and experimental workflows.
- Workspace classification and dependency direction are machine checked.
- A canonical compilation/diagnostic baseline is checked in.
- New disabled `#[cfg(any())]` cemetery code is rejected.
- Scope freeze and migration ownership are visible from the roadmap.
