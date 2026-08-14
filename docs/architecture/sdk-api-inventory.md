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

Filesystem/package capture is an explicit `project` feature and
`project::ProjectCompiler` adapter. It is a CLI/project-loader convenience,
not part of the reviewed in-memory `compile::Compiler` contract.
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
- Artifact lifecycle: `BuiltArtifact`, `VerifiedArtifact`, `ArtifactBundle`,
  `ArtifactVerifier`, typed `SourceAnalysisV1`,
  `AnalysisEnvelopeV1`/`AnalysisSchemaV1`, provenance,
  interface requirements, the versioned source and package analysis schema
  identifiers, and neutral semantic diff data,
  including structural external-call, public function ownership, call-graph,
  recursion, lexical resource-lifetime and explicit resource-transfer, and
  structured task-group contracts.
- Provider lifecycle: `ProviderRegistry`, provider descriptors, structured
  signatures, `WireInterpreterFn`/`AsyncWireInterpreterFn`/`WireValue` for the
  canonical scalar plus descriptor-scoped aggregate Provider path (`List<T>`,
  tuples, `Option<T>`, and `Result<T, E>`), registration errors, and typed
  execution context contracts.
  Legacy `NativeInterpreterFn`/`NativeValue` are not re-exported from this
  reviewed façade; compatibility adapters must opt into the SDK
  `compatibility` surface or depend directly on the low-level Provider crate.
- Runtime lifecycle: `Runtime`, `LinkedArtifact`, `ExecutionRequest`, bounded
  `RunLimits`, `ExecutionReport`, termination reason, usage, and diagnostics.
  The reviewed Rust report exposes the stable textual result only. Its retained
  v1 JSON `native_value` projection is private compatibility serialization, not
  a value type new embedders can read or construct.
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
SHA-256 snapshot of each reviewed façade module's normalized `pub use` surface;
CI rejects additions, removals, and reexports that are not accompanied by an
intentional inventory and snapshot update. CI runs that suite for the default
product path and for `execution`; the native JIT suite is maintained in the
experiments workflow. Before a public API promise is made, this source-level
snapshot will be complemented by a generated semver baseline.
