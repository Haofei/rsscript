# RSScript compatibility policy

RSScript publishes independent versioned execution contracts plus separately
versioned evidence and transport schemas. Compatibility is checked before
Provider linking or execution.

The language specification filename is a document revision, not a fifth runtime
version. `RSScript_v0.7_Spec.md` is currently the normative text for the
`0.1.x` language-semantics line below; Artifact, runtime, and Provider versions
remain independent compatibility contracts.

| Contract | Current line | Compatibility rule |
| --- | --- | --- |
| Language | `0.1.x` (current `0.1.0`) | A compiler/runtime accepts only its own pre-1.0 minor line. |
| Artifact schema | `rsscript.bytecode.v1` | Unknown schema versions and unknown required sections are rejected. Unknown optional sections may be skipped. |
| Runtime ABI | `2` | Exact match is required. |
| Core library ABI | `1` | Exact match is required by the bytecode verifier. |
| Provider signature | structural signature + hash | Exact ABI version and structural signature match are required at link time. |
| Artifact Bundle | `rsscript.artifact_bundle.v1` | Bundle readers reject malformed, tampered, or unsupported required content. |
| Package/source analysis | `rsscript.package_analysis.v1` / `rsscript.source_analysis.v1` | Typed evidence rejects unknown fields and is bound to the bundle digest. |
| Semantic diff | `rsscript.semantic_diff.v2` | Policy-neutral facts are versioned separately from Artifact bytes. |
| Execution report | `rsscript.execution_report.v2` | Consumers must reject unknown required report shape rather than infer a result. |
| Runner protocol | `rsscript.runner_request.v1` / `rsscript.runner_response.v1` | Request framing is bounded; a response must prove the host-selected profile identity. |

Patch releases may add diagnostics, optional Artifact sections, and compatible
Provider SDK helpers. They may not reinterpret an existing instruction,
structural wire type, ownership/resource operation, or Provider signature.

Before 1.0, a language minor release may intentionally break source or
Artifact compatibility. Such a release must increment the language minor,
publish a migration note, and retain fail-closed loading: an older runtime must
return `UnsupportedLanguageVersion` rather than attempt execution.

Artifact checksums detect corruption and bind sections together; they are not a
signature or proof of provenance. Hosts that need origin authentication must
verify an external signature before loading the Artifact.

Release binaries, pre-release tags, and the Git-revision-only alpha SDK policy
are defined in [releasing.md](releasing.md). A binary release version does not
implicitly promote Experimental providers, AOT, JIT, REIR, plugins, or research
surfaces to the Core compatibility contract.
