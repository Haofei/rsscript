# RSScript SDK public API inventory

This file is the reviewed inventory for the `rsscript-sdk` crate. It is a
pre-1.0 compatibility guard: changing a listed category or adding a public
root export requires updating this document and the architecture test in the
same change.

## Stable façade

The stable façade is exposed through the explicit `compile`, `artifact`,
`provider_api`, `runtime`, `report`, `analysis`, and `operation` modules.
New embedding documentation and first-party applications use these modules;
the transitional root exports are removed only by A03.3 after compatibility
callers have migrated.

- Compilation and diagnostics: `Compiler`, `CompileError`, checked source and
  language-service query types.
- Artifact lifecycle: `BuiltArtifact`, `VerifiedArtifact`, `ArtifactBundle`,
  `ArtifactVerifier`, provenance, interface requirements, and neutral semantic
  diff data.
- Provider lifecycle: `ProviderRegistry`, provider descriptors, structured
  signatures, registration errors, and typed execution context contracts.
- Runtime lifecycle: `Runtime`, `LinkedArtifact`, `ExecutionRequest`, bounded
  `RunLimits`, `ExecutionReport`, termination reason, usage, and diagnostics.
- Shared operation control: cancellation tokens, monotonic deadlines, and
  operation contexts.

## Compatibility-only APIs

The `reg_vm_*` helpers and `RegVmExecutable` are retained only while the MIR
migration runs its old/new differential corpus. They are deliberately hidden
from rustdoc and must not be used as new embedding entry points.

## Feature-gated experimental APIs

Native JIT entry points and `NativeStats` exist only under the `native-jit`
feature. They must never be exported by the default or `execution` SDK feature
set. AOT, REIR, review/risk, opcode, register, and compiler-internal APIs are
not part of this inventory.

## Compatibility check

The Core architecture suite verifies this inventory and scans the explicit SDK
exports. CI runs that suite for the default product path and for `execution`;
the native JIT suite is maintained in the experiments workflow. Before a
public API promise is made, this inventory will be replaced by a semver
baseline generated from the completed explicit façade modules.
