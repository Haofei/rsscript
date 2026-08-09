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

Parent items are architecture milestones. Their indented child items are the
planning units: each should fit in one focused change set with targeted tests.
A parent may be checked only after every child is checked and its stated
mechanical acceptance condition holds.

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
  - [ ] **S02.1 — Move declaration/name-resolution facts.** Relocate symbol,
    namespace, import, and declaration validation with identical diagnostic
    fixtures and spans.
  - [ ] **S02.2 — Move type and generic-constraint checks.** Relocate type
    inference, substitutions, generic constraints, and call result facts behind
    the semantic crate API.
  - [ ] **S02.3 — Move call binding and effect checks.** Relocate positional and
    named argument binding, `read`/`mut`/`take`, external signature matching,
    and retention facts.
  - [ ] **S02.4 — Move ownership and resource checks.** Relocate moves, escapes,
    borrows, `fresh`/`owned`/`noescape`, resource declarations, and cleanup
    validation with property and hostile-corpus coverage.
  - [ ] **S02.5 — Move async and control-flow checks.** Relocate task groups,
    cancellation, await/select, assignment, exhaustiveness, and reachability
    checks.
  - [ ] **S02.6 — Delete compiler semantic-rule modules.** Add architecture
    tests that permit compiler orchestration only and reject semantic-rule
    implementations outside `rsscript-semantics`.
- [ ] **S03 — Add one `CompilationSession` query boundary.** Introduce stable
  source/module/interface/definition/type identities, dependency tracking,
  cancellation, deadlines, and cached HIR/MIR queries shared by CLI, package
  compilation, tests, and editor tooling.
  - [ ] **S03.1 — Define source and semantic identities.** Add stable source,
    revision, module, interface, definition, and type IDs with deterministic
    construction and serialization tests.
  - [ ] **S03.2 — Capture revisions in a session-owned source store.** Replace
    ad-hoc frontend inputs with immutable revisions and explicit replacement or
    removal operations.
  - [ ] **S03.3 — Cache parse, resolve, type, HIR, and diagnostic queries.**
    Record dependencies so unrelated file changes do not invalidate a workspace.
  - [ ] **S03.4 — Thread cancellation and deadlines through every query.** Add
    cancellation, deadline, and diagnostic-budget tests for cold and cached
    paths.
  - [ ] **S03.5 — Migrate CLI/package/test callers.** All frontend consumers use
    the session API; direct analyzer construction becomes private.
- [ ] **S04 — Make language service consume semantic queries directly.** It
  must not depend on the compiler compatibility façade, package persistence,
  VM, SDK, or Providers; revision invalidation and request cancellation require
  focused tests.
  - [ ] **S04.1 — Replace the compiler façade dependency with syntax/semantic
    query dependencies.** Cargo metadata tests reject a language-service edge to
    compiler, VM, SDK, package persistence, or concrete Providers.
  - [ ] **S04.2 — Add document revision and invalidation tests.** Verify edits,
    deletes, interface changes, cancellation, and deadlines through the LSP
    adapter.
- [ ] **S05 — Finish compiler purity.** Compiler input is an explicit immutable
  `SourceSet`/`WorkspaceSnapshot`; package traversal, filesystem locking,
  temporary files, compression, Artifact persistence, review/risk, and Rust AOT
  lowering live outside the compiler dependency closure.
  - [ ] **S05.1 — Move workspace capture to `rsscript-workspace-loader`.** Move
    directory traversal, manifest/dependency discovery, path normalization, and
    snapshot capture from compiler.
  - [ ] **S05.2 — Move Artifact persistence to an adapter.** Relocate locks,
    atomic writes, temporary files, compression, and artifact-store policy out
    of compiler.
  - [ ] **S05.3 — Move review, risk, and package presentation out of compiler.**
    Keep neutral analysis facts; make review formatting and policy adapters
    optional consumers.
  - [ ] **S05.4 — Move Rust/AOT lowering to its experimental boundary.** Compiler
    no longer exposes generated Rust or native lowering APIs.
  - [ ] **S05.5 — Enforce a frontend-only compiler dependency closure.** Cargo
    metadata and `cargo tree` tests reject OS, persistence, Provider, VM, review,
    JIT, and AOT dependencies.

### 2. Typed CFG MIR

- [ ] **M01 — Define typed stable identities.** Add `FunctionId`, `TypeId`,
  `BlockId`, `ValueId`, `PlaceId`, `BuiltinId`, `ExternalSymbolId`, and
  `ResourceTypeId` without string identity at backend boundaries.
  - [ ] **M01.1 — Define index/newtype IDs and ownership tables.** IDs are local
    to one MIR module, non-string, deterministic, and cannot be mixed by type.
  - [ ] **M01.2 — Lower semantic names and `WireType` references into IDs.**
    Backend inputs contain resolved function, external symbol, builtin, and
    resource identities only.
  - [ ] **M01.3 — Add stable display/debug/source-map side tables.** Human names
    remain available without becoming executable identity.
- [ ] **M02 — Define an owned CFG MIR.** Functions contain basic blocks,
  instructions, and terminators; MIR does not depend on syntax and contains no
  unresolved or `Unknown` execution node.
  - [ ] **M02.1 — Introduce `MirModule`, `MirFunction`, `BasicBlock`,
    `Instruction`, and `Terminator`.** Only the lowerer and verifier may
    construct valid modules.
  - [ ] **M02.2 — Lower the pure scalar subset.** Cover constants, locals,
    arithmetic, calls, returns, branches, loops, and explicit block edges.
  - [ ] **M02.3 — Lower aggregate and pattern operations.** Cover records,
    variants, collections, field/index operations, and match dispatch without
    source AST nodes in MIR.
  - [ ] **M02.4 — Add MIR structural validation.** Reject dangling blocks,
    invalid IDs, unterminated blocks, undefined values, and malformed CFG edges.
- [ ] **M03 — Make semantic operations explicit.** MIR represents move,
  read/mut borrow, retain, drop, resource acquire/release, spawn, await, join,
  cancellation, selection, external calls, and every cleanup/unwind edge.
  - [ ] **M03.1 — Add explicit ownership instructions.** Model move, read borrow,
    mutable borrow, retain, and drop, then test use-after-move rejection on CFG
    joins.
  - [ ] **M03.2 — Add resource lifetime instructions and cleanup edges.** Model
    acquire/manage/release and verify cleanup for normal return, branch exit,
    error, and cancellation.
  - [ ] **M03.3 — Add structured-concurrency instructions.** Model spawn, await,
    join, cancel, and select with lexical task-group ownership.
  - [ ] **M03.4 — Add resolved builtin and external-call instructions.** Include
    signature/effect/retention identity and no unresolved callee text.
- [ ] **M04 — Lower checked HIR to MIR exactly once.** Backend code cannot
  inspect syntax AST or reconstruct semantic facts. MIR verification rejects
  unresolved calls, invalid ownership state, incomplete cleanup, and malformed
  structured-task scopes.
  - [ ] **M04.1 — Create the one-way HIR-to-MIR lowerer.** It consumes checked
    semantic facts, not syntax AST projections.
  - [ ] **M04.2 — Verify MIR ownership, resources, and task scopes.** Add a
    verifier pass with targeted invalid-MIR fixtures.
  - [ ] **M04.3 — Enforce backend input boundaries.** Architecture tests reject
    syntax/HIR imports in VM, codegen, AOT, and JIT backend code.
- [ ] **M05 — Run old/new lowering differentially.** The same corpus must
  produce equivalent diagnostics, external imports, termination reasons,
  values, cleanup behavior, and deterministic usage reports.
  - [ ] **M05.1 — Add pure-control-flow differential fixtures.** Compare return
    values, errors, and usage reports.
  - [ ] **M05.2 — Add ownership/resource differential fixtures.** Compare move
    failures, retain behavior, cleanup counts, and resource limits.
  - [ ] **M05.3 — Add async/provider differential fixtures.** Compare task
    cancellation, external-call order, deadlines, and Provider traces.
  - [ ] **M05.4 — Gate replacement on corpus parity.** New lowering cannot become
    default until all supported Core fixtures agree.
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
  - [ ] **V02.1 — Create a MIR-only codegen crate.** Its manifest may depend on
    MIR, ABI, and bytecode model but not VM, compiler, syntax, package, or SDK.
  - [ ] **V02.2 — Lower the scalar MIR subset to bytecode.** Preserve source maps
    and deterministic module ordering.
  - [ ] **V02.3 — Lower resources, async, builtins, and external calls.** Add
    codegen fixtures for every Core MIR instruction.
  - [ ] **V02.4 — Switch SDK build to codegen-vm.** Remove VM compile helpers
    from the supported compilation path and add dependency tests.
- [ ] **V03 — Make the verifier construct the only executable program type.**
  Untrusted bytes decode and verify to a private-field `VerifiedModule`; public
  APIs cannot construct or mutate it and VM constructors accept nothing else.
  - [ ] **V03.1 — Define private-field `VerifiedModule`.** Only bytecode verifier
    code can create it from bounded decoded bytes.
  - [ ] **V03.2 — Move instruction/data-flow verification into bytecode.** VM no
    longer independently validates decoded program structure.
  - [ ] **V03.3 — Restrict VM constructors.** Delete constructors accepting raw
    bytecode, executable IR, or decoded mutable instruction vectors.
- [ ] **V04 — Make the VM execution-only.** Remove MIR/executable-IR lowering,
  bytecode encoding, Artifact packaging, compiler/source helpers, and duplicate
  payload verification from `rsscript-vm`.
  - [ ] **V04.1 — Delete VM source/HIR/executable-IR compile entry points.**
    Preserve only load/link/execute APIs over `VerifiedModule`.
  - [ ] **V04.2 — Delete VM bytecode encoder and Artifact assembly.** Move all
    production encode logic to codegen/Artifact crates.
  - [ ] **V04.3 — Delete duplicate VM payload verifier.** Keep runtime defensive
    assertions only; malformed-byte handling belongs to bytecode verifier.
- [ ] **V05 — Remove experimental state from Core VM program objects.** JIT,
  OSR, deopt, branch/call profiles, and native tier state live in experiment-owned
  side tables keyed by stable function IDs.
  - [ ] **V05.1 — Introduce experiment-owned `JitState` side tables.** Key state
    by stable function IDs and lifetime-bound execution instances.
  - [ ] **V05.2 — Move profiles, OSR, deopt, and native code handles.** Remove
    these fields from `RegFunction` and program types.
  - [ ] **V05.3 — Make Core VM build without JIT data structures.** Add a
    dependency and layout regression test.
- [ ] **V06 — Split VM primitives from deterministic core library.** VM Core
  keeps frames, registers, scheduler, cancellation, resource table, limits,
  dispatch, and external calls. JSON/YAML, regex, compression, encoding, hashes,
  date utilities, and collection algorithms move behind a versioned builtin
  registry or Core library runtime.
  - [ ] **V06.1 — Define the builtin registry contract.** Include `BuiltinId`,
    signature, determinism, cost, and version/digest information.
  - [ ] **V06.2 — Move pure library families incrementally.** Start with encoding
    and collection helpers, then JSON/YAML, regex, compression, hashes, and date
    utilities while preserving differential results.
  - [ ] **V06.3 — Reduce VM dependencies to execution primitives.** Verify VM
    Core no longer directly depends on library implementation crates.
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
  - [ ] **B04.1 — Declare independent compatibility constants.** Define container,
    language, ISA, Core library, Provider, analysis, and provenance versions.
  - [ ] **B04.2 — Validate each version at its owning boundary.** Verify container
    at decode, language/ISA at program verification, Core library at load, and
    Provider ABI at link.
  - [ ] **B04.3 — Add supported-range fixtures.** Cover accepted N/N-1 inputs and
    unknown major versions that must fail closed.
- [ ] **B05 — Preserve a versioned compatibility corpus.** Keep read-only v1
  fixtures, malformed v1/v2 inputs, N-1 schema fixtures, deterministic
  cross-platform bytes, and explicit unknown-version/section fail-closed tests.
  - [ ] **B05.1 — Check in read-only v1 bundles and expected reports.** Retain
    loaders after v2 becomes the writer.
  - [ ] **B05.2 — Add malformed and compatibility fixture suites.** Cover every
    section, table, opcode, version, and size boundary.
  - [ ] **B05.3 — Test deterministic bytes across supported platforms.** Compare
    bundle and analysis bytes from identical snapshots.

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
  - [ ] **P04.1 — Define `InterfaceDescriptorV1`.** Include canonical symbols,
    `WireType`, effects, retention, async shape, resources, and signature hashes.
  - [ ] **P04.2 — Emit descriptors from semantic checking.** Snapshot descriptor
    bytes and ensure source aliases cannot alter canonical ABI facts.
  - [ ] **P04.3 — Replace bindgen source parsing.** Bindgen accepts only the
    descriptor and rejects unsupported descriptor versions.
- [ ] **P05 — Generate typed Rust Provider APIs.** Generate sync/async traits,
  typed parameters/results, resource wrappers, descriptor/signature constants,
  registration glue, mocks, completeness checks, and conformance skeletons.
  `NativeValue` remains only in generated adapters.
  - [ ] **P05.1 — Generate scalar and aggregate Rust type mappings.** Cover unit,
    booleans, integers, floats, strings, bytes, lists, options, results, tuples,
    records, and variants.
  - [ ] **P05.2 — Generate sync and async Provider traits.** Method signatures
    reflect descriptor parameters, results, effects, and async shape.
  - [ ] **P05.3 — Generate resource wrappers and adapter glue.** Resource values
    use typed generation-safe handles; adapters isolate `NativeValue` conversion.
  - [ ] **P05.4 — Generate registration, mock, and completeness tests.** Provider
    implementations fail to compile or conformance-test when symbols drift.
- [ ] **P06 — Tighten the canonical wire value model.** Replace JSON, string
  type/field identity, and generic `Native { type_name, id }` escape hatches
  with typed records, variants, lists, resources, and generation-safe handles;
  JSON becomes an explicitly declared extension codec.
  - [ ] **P06.1 — Define typed wire records, variants, and resources.** Use type,
    field, variant, slot, and generation identity instead of free strings.
  - [ ] **P06.2 — Implement the compatibility adapter.** Convert legacy
    `NativeValue` at generated boundaries while Core contracts use wire values.
  - [ ] **P06.3 — Migrate official Providers and mocks.** Each migration keeps
    signature, error, resource, and payload-budget conformance fixtures green.
  - [ ] **P06.4 — Remove legacy escape variants from canonical APIs.** JSON stays
    only behind a named extension codec with explicit interface declaration.
- [ ] **P07 — Remove policy-shaped authority from Core ABI.** Rename it to a
  neutral host-call context or move authority scopes to runner/provider profiles;
  Core reports required symbols but does not interpret authorization policy.
- [ ] **P08 — Complete async/resource conformance.** Test cancellation during
  suspension, deadline expiry, blocking-lane enforcement, cleanup exactly once
  on success/error/cancel/deadline, reentrancy, panic containment, redaction, and
  request/response limits.
  - [ ] **P08.1 — Add async cancellation and deadline fixtures.** Assert Provider
    observation and VM/runner termination semantics while futures are pending.
  - [ ] **P08.2 — Add resource cleanup state-machine fixtures.** Cover success,
    script error, Provider error, cancellation, deadline, and drop failure.
  - [ ] **P08.3 — Add lane, reentrancy, and panic boundary fixtures.** Validate
    blocking policy and host failure containment.
  - [ ] **P08.4 — Add redaction and payload-limit fixtures.** Reports and traces
    remain bounded and do not expose sensitive Provider payloads by default.

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
  - [ ] **A03.1 — Inventory the existing SDK surface.** Classify each export as
    stable façade, compatibility-only, experimental, or internal.
  - [ ] **A03.2 — Create explicit façade modules.** Expose only compile,
    artifact, provider, runtime, report, diagnostics, and operation APIs.
  - [ ] **A03.3 — Move compatibility and experimental APIs behind opt-in modules.**
    JIT, AOT, review/risk, register VM, and opcode APIs disappear from defaults.
  - [ ] **A03.4 — Add public API snapshots.** CI rejects unreviewed stable-surface
    growth and validates feature combinations.
- [ ] **A04 — Remove invalid phase states and report-losing paths.** Public types
  do not use optional fields to represent incompatible phases; script,
  Provider, cancellation, deadline, and budget termination always return a full
  execution report. Only host/protocol/internal-invariant failures use outer
  errors.
  - [ ] **A04.1 — Audit all public phase types.** Replace optional phase fields
    and cross-phase enums with built/verified/linked/report-specific types.
  - [ ] **A04.2 — Audit every execution convenience API.** Script and Provider
    failures return `ExecutionReport`; only host/protocol failures return errors.
  - [ ] **A04.3 — Add compile-time and runtime phase tests.** Invalid transitions
    are unrepresentable through public constructors and report retention is
    tested for every termination reason.
- [x] **A05 — Make execution bounded by default.** Unbounded execution requires
  an explicitly named trusted-host constructor; per-run limits live on the
  execution request.
- [x] **A06 — Ship Artifact Bundle, `rss verify`, and neutral `rss diff`.** Both
  single-file and package builds produce analysis/provenance-bound bundles.
- [ ] **A07 — Complete semantic diff evidence.** Add read/mut/take, retention and
  escape, resource acquire/transfer/cleanup, structured-task fan-out and
  cancellation, call graph/recursion, Provider requirements, and diagnostic
  additions/removals while remaining policy-neutral.
  - [ ] **A07.1 — Diff ownership and call contracts.** Report effect, parameter,
    retention, escape, and external signature changes.
  - [ ] **A07.2 — Diff resources and concurrency.** Report acquire/transfer/close,
    task fan-out, await/select, cancellation, and cleanup-path changes.
  - [ ] **A07.3 — Diff graph and diagnostic facts.** Report call graph/recursion,
    Provider requirements, and diagnostic additions/removals.
  - [ ] **A07.4 — Version and fixture the neutral schema.** Add JSON/Markdown
    goldens and prove no policy verdict or risk score enters the output.
- [x] **A08 — Run scripts out of process by default.** `rss run` uses the
  versioned child protocol; trusted in-process execution is explicit.
- [ ] **A09 — Harden the reference Linux runner profile.** Add allowlisted
  Provider profiles, namespace/syscall/filesystem/network controls where
  available, parent-enforced kill-on-deadline, protocol/disconnect fuzzing, and
  tests separating runner termination from VM termination. Continue to state
  that this is defense in depth rather than a universal sandbox.
  - [ ] **A09.1 — Introduce explicit runner profiles.** Profiles preinstall
    allowlisted Providers and their host-owned roots/endpoints; requests cannot
    supply Provider code, library paths, credentials, or authorities.
  - [ ] **A09.2 — Add Linux isolation adapters.** Implement optional namespace,
    syscall, filesystem, network, and cgroup controls with capability detection
    and fail-closed profile requirements.
  - [ ] **A09.3 — Complete parent-side containment.** Cover process-tree kill,
    deadline, stdout/stderr/report limits, abnormal exits, and child disconnects.
  - [ ] **A09.4 — Fuzz protocol and runner failure paths.** Exercise framing,
    malformed messages, oversized inputs, incomplete I/O, and termination
    separation without calling it a universal sandbox.

### 7. Adoption, evidence, and maintenance

- [x] **E01 — Gate representative Core performance.** CI records check,
  compile, Artifact verify, VM, Provider boundary, cancellation, Artifact size,
  and deterministic usage metrics against the checked SLO fixture.
- [ ] **E02 — Add two complete product examples.** Keep the embedded Provider
  replacement pipeline and add a reviewable async/resource workflow; each must
  contain source, interfaces, generated Provider contract, memory and
  production-like Providers, Artifact identity, semantic-diff fixture, and
  success/failure reports for trusted and isolated execution.
  - [ ] **E02.1 — Upgrade the embedded report pipeline fixtures.** Add generated
    interface descriptor, semantic diff, and trusted/isolated report snapshots.
  - [ ] **E02.2 — Add an async/resource workflow example.** Demonstrate task
    groups, cancellation, cleanup, mock/production-like Providers, and failures.
- [ ] **E03 — Establish compatibility and conformance corpora.** Add source to
  diagnostic/HIR/MIR goldens, MIR to bytecode fixtures, old Artifact readers,
  cross-platform deterministic builds, Provider ABI compatibility, resource
  cleanup state machines, and interpreter/experimental-backend differential
  tests.
  - [ ] **E03.1 — Add source/semantic/MIR golden corpus.** Freeze diagnostics,
    normalized HIR/MIR, and lowering failures separately.
  - [ ] **E03.2 — Add Artifact/Provider compatibility corpus.** Cover old readers,
    ABI mismatch, replacement Providers, and deterministic bundles.
  - [ ] **E03.3 — Add execution state-machine corpus.** Cover budgets,
    cancellation, cleanup, Provider errors, and interpreter/experiment parity.
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
