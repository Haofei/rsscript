# RSScript SDK public API inventory

This file is the reviewed inventory for the `rsscript-sdk` crate. It is a
pre-1.0 compatibility guard: changing a listed category or adding a public
root export requires updating this document and the architecture test in the
same change.

## Stable façade

The stable façade is exposed through the explicit `compile`, `artifact`,
`provider_api`, `runtime`, `report`, `analysis`, and `operation` modules.
New embedding documentation and first-party applications use these modules.
The transitional root exports are available only through the explicit
`compatibility` feature while the MIR differential corpus migrates; they are
not part of the default or `execution` SDK surface.

Filesystem/package capture is an explicit `project` feature. The independent
`rsscript-project` crate owns the OS-captured workspace-to-frontend-snapshot
conversion; the SDK's `project::ProjectCompiler` only composes it with the
reviewed in-memory compiler. It is a CLI/project-loader convenience, not part
of the reviewed in-memory `compile::Compiler` contract.
`ProjectCompiler::capture_frontend_from` is the preferred explicit-base path
for a normal source/interface workspace: it returns a loader-owned logical
snapshot plus the immutable `FrontendInputSnapshot` accepted by `Compiler`.
Legacy package review/native authorization remains on the separate package
snapshot path during migration.

- Compilation and diagnostics: `Compiler`, `CompileError`, immutable
  `FrontendInputSnapshot`, snapshot-based check/build entry points, checked
  source, and language-service query types. The frontend language module also
  exposes the operation-aware multi-source/interface analysis query used by
  the shared workspace diagnostics path; it remains frontend-only and has no
  runtime or Provider dependency.
- Artifact lifecycle: `BuiltArtifact`, `VerifiedArtifact`,
  `AdmittedArtifact`, `ArtifactAdmissionPolicy`, `ArtifactBundle`,
  `ArtifactVerifier`, typed `SourceAnalysisV1`,
  `PackageAnalysisV1`, `AnalysisEnvelopeV1`/`AnalysisSchemaV1`, provenance,
  interface requirements, the versioned source and package analysis schema
  identifiers, and neutral semantic diff data,
  `SourceAnalysisV1` includes checked function ownership/retention contracts,
  the direct call graph, and external-call facts bound to ordinary source
  builds. `BuiltArtifact::analysis_envelope` is the schema-discriminated access point;
  `source_analysis` and `package_analysis` expose the corresponding typed
  payload only when its schema matches. This keeps raw JSON outside the
  reviewed embedding contract. The analysis facts include structural external-call, public function ownership, call-graph,
  recursion, lexical resource-lifetime and explicit resource-transfer, and
  structured task-group contracts. Verification proves format and semantic
  integrity; a host must then explicitly admit the verified Artifact before
  Provider linking. `TrustedInputAdmission` is an intentionally named shortcut
  for a host-controlled input channel, while runners and provenance-aware
  hosts implement their own admission policy.
- Provider lifecycle: `ProviderRegistry`, provider descriptors, structured
  signatures, their constructor support types (`DataEffect`,
  `ParameterSignature`, `ProviderErrorMapping`, `ResourceCleanupContract`, and
  `RUNTIME_ABI_VERSION`, plus `ProviderResource` for run-owned cleanup),
  `WireInterpreterFn`/`AsyncWireInterpreterFn` and
  `WireMutationInterpreterFn`/`AsyncWireMutationInterpreterFn` with
  `WireMutationResult`, plus
  `WireValue` for the canonical scalar plus descriptor-scoped aggregate Provider
  path (`List<T>`, tuples, `Option<T>`, and `Result<T, E>`), registration
  errors, and typed execution context contracts. This permits a Provider author
  to construct and register a complete descriptor solely through the reviewed
  SDK façade. Opt-in replay contracts, tapes, entries, modes, and synchronous/
  asynchronous wire-callable wrappers provide in-memory deterministic
  test/diagnostic replay only; they neither persist values nor establish an
  authorization or security decision.
  Legacy `NativeInterpreterFn`/`NativeValue` are not re-exported from this
  reviewed façade; compatibility adapters must opt into the SDK
  `compatibility` surface or depend directly on the low-level Provider crate.
- Runtime lifecycle: `Runtime`, `LinkedArtifact`, `ExecutionRequest`, bounded
  `RunLimits`, `ExecutionReport`, `ExecutionOutcome`, termination reason,
  usage, and diagnostics. The reviewed Rust report exposes exactly one terminal
  outcome: a completed textual result or a structured failure. Its retained v1
  JSON `native_value` projection is private compatibility serialization, not a
  value type new embedders can read or construct.
- Shared operation control: cancellation tokens, monotonic deadlines, and
  operation contexts.

## Compatibility-only APIs

The `reg_vm_*` helpers, `RegVmExecutable`, legacy `NativeInterpreterFn` and
`NativeValue`, package review/risk types, and raw bytecode helpers are retained
only behind `compatibility` while the MIR
migration runs its old/new differential corpus. They are deliberately hidden
from the reviewed default surface and must not be used as new embedding entry
points.

## Feature-gated experimental APIs

Native JIT entry points and `NativeStats` exist only under the `native-jit`
feature. They must never be exported by the default or `execution` SDK feature
set. AOT, REIR, review/risk, opcode, register, and compiler-internal APIs are
not part of this inventory.

## Compatibility check

The Core architecture suite verifies this inventory and scans the explicit SDK
exports. [`sdk-api-snapshot.v1.toml`](sdk-api-snapshot.v1.toml) records a
SHA-256 snapshot of each reviewed façade module's normalized `pub use` surface.
The normalization treats a façade export as a set: whitespace, declaration
order, and flat brace-group member order cannot turn a `rustfmt` run into a
spurious public-API change. CI rejects additions, removals, and reexports that
are not accompanied by an intentional inventory and snapshot update. CI runs that suite for the default
product path and for `execution`; the native JIT suite is maintained in the
experiments workflow. `sdk-api-compatibility.yml` additionally runs pinned
`cargo-semver-checks` against the target-branch commit for both reviewed
feature closures. This branch baseline provides a generated API-diff gate
before a crates.io release exists; compatibility and native-JIT features are
intentionally excluded.
