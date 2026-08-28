# RSScript SDK public API inventory

This file is the reviewed inventory for the `rsscript-sdk` crate. It is a
pre-1.0 compatibility guard: changing a listed category or adding a public
root export requires updating this document and the architecture test in the
same change.

## Stable façade

The stable façade is exposed through the explicit `compile`, `artifact`,
`provider_api`, `runtime`, `report`, `analysis`, and `operation` modules.
New embedding documentation and first-party applications use these modules.
The historical root-level compatibility feature has been retired. New embeds
use only the explicit reviewed modules, which are the complete default and
`execution` SDK surface.

Filesystem/package capture is an explicit `project` feature. The independent
`rsscript-project` crate owns the OS-captured workspace-to-frontend-snapshot
conversion; the SDK's `project::ProjectCompiler` only composes it with the
reviewed in-memory compiler. It is a CLI/project-loader convenience, not part
of the reviewed in-memory `compile::Compiler` contract.
`ProjectCompiler::capture_frontend_from` is the preferred explicit-base path
for a normal source/interface workspace: it returns a loader-owned logical
snapshot plus the immutable `FrontendInputSnapshot` accepted by `Compiler`.
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
  hosts use `OriginVerifiedAdmission` and `ArtifactOriginVerifier` (or their
  own admission policy) to bind detached origin evidence.
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
  The reviewed VM and façade use only `WireValue`; historical dynamic Provider
  adapters, where still needed by external migration tooling, live outside the
  VM execution closure.
- Runtime lifecycle: `Runtime`, `LinkedArtifact`, `ExecutionProfileV1`,
  `ExecutionRequest`, bounded `RunLimits`, `ExecutionReport`,
  `ExecutionOutcome`, termination reason,
  usage, and diagnostics. The reviewed Rust report exposes exactly one terminal
  outcome: a completed textual result or a structured failure. Provider and
  return values cross this boundary only as canonical `WireValue` data.
- Shared operation control: cancellation tokens, monotonic deadlines, and
  operation contexts.

## Optional native tier

The trusted in-process native tier is an explicit SDK feature. Hosts select it
through typed `experimental::native_jit::NativeJitOptions`; the experimental
namespace deliberately carries no Rust source-compatibility promise, while
selecting it never changes execution limits.
The report exposes only stable engine telemetry, not `NativeStats`, opcodes,
registers, OSR plans, or backend implementation state. AOT, REIR, review/risk,
and compiler-internal APIs remain outside this inventory.

## Compatibility check

The Core architecture suite verifies this inventory and scans the explicit SDK
exports. [`sdk-api-snapshot.v1.toml`](sdk-api-snapshot.v1.toml) records a
SHA-256 snapshot of each reviewed façade module's normalized `pub use` surface.
The normalization treats a façade export as a set: whitespace, declaration
order, and flat brace-group member order cannot turn a `rustfmt` run into a
spurious public-API change. CI rejects additions, removals, and reexports that
are not accompanied by an intentional inventory and snapshot update. CI runs that suite for the default
product path and for `execution`. `sdk-api-compatibility.yml` additionally runs pinned
`cargo-semver-checks` against the target-branch commit for both reviewed
feature closures. This branch baseline provides a generated API-diff gate
before a crates.io release exists.
