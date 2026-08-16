# RSScript compatibility policy

RSScript publishes four independent versioned contracts. Compatibility is
checked before Provider linking or execution.

The language specification filename is a document revision, not a fifth runtime
version. `RSScript_v0.7_Spec.md` is currently the normative text for the
`0.1.x` language-semantics line below; Artifact, runtime, and Provider versions
remain independent compatibility contracts.

| Contract | Current line | Compatibility rule |
| --- | --- | --- |
| Language | `0.1.x` | A compiler/runtime accepts only its own pre-1.0 minor line. |
| Artifact schema | `rsscript.bytecode.v1` | Unknown schema versions and unknown required sections are rejected. Unknown optional sections may be skipped. |
| Runtime ABI | `2` | Exact match is required. |
| Provider signature | structural signature + hash | Exact ABI version and structural signature match are required at link time. |

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
