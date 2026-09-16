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
are defined below. A binary release version does not
implicitly promote Experimental providers, JIT, plugins, or archived research
surfaces to the Core compatibility contract.

## Releases and SDK distribution

RSScript is pre-1.0. A manual run of the `Release` workflow is the required
dry-run: it performs the full locked validation gate, builds all supported
targets, verifies checksums, and creates build-provenance attestations, but does
not create a GitHub Release. A matching tag promotes those same artifacts.

### Binary alpha releases

Supported binary targets are:

| Target | Runner | Assets |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Ubuntu 24.04 | `rss` |
| `aarch64-apple-darwin` | macOS 14 | `rss` |
| `x86_64-pc-windows-msvc` | Windows 2025 | `rss.exe` |

Tags must exactly match the `rsscript-cli` Cargo version. Accepted
forms are `vX.Y.Z`, `vX.Y.Z-alpha.N`, `vX.Y.Z-beta.N`, and `vX.Y.Z-rc.N`.
Pre-release tags are marked as GitHub pre-releases and never become `latest`.

Every target publishes target-qualified binaries, `BUILD-INFO-<target>.txt`,
and `SHA256SUMS-<target>`. Consumers verify the checksum and then verify GitHub
build provenance; the bytecode checksum alone is not source authentication.

### Rust embedding SDK

`rsscript-sdk` is the supported embedding façade, but it is deliberately not
published to crates.io during the alpha architecture window. Its compiler-core
dependency is still repository-internal, so publishing only the façade would
produce an unusable package and a false stability promise. `publish = false`
makes this policy mechanical.

Alpha embedders pin the repository to a full reviewed commit:

```toml
[dependencies]
rsscript-sdk = { git = "https://github.com/Haofei/rsscript", rev = "<40-hex-commit>", features = ["execution"] }
```

The commit, `Cargo.lock`, language version, Artifact schema, runtime ABI, and
Provider signatures together identify the tested SDK/runtime combination.
Branch dependencies such as `branch = "main"` are unsupported for production
embedding.

Crates.io publication requires all non-optional public dependencies to be
publishable with explicit compatible versions, a successful
`cargo publish --dry-run`, generated package contents review, and an alpha tag
that passes the same multi-platform release workflow. Until then, Git revision
pinning is the only supported Rust distribution mechanism.

### Release sequence

1. Update the two binary package versions and compatibility documentation.
2. Run the `Release` workflow manually and inspect all three target artifacts.
3. Verify each `SHA256SUMS-<target>` file and provenance attestation.
4. Create the exact matching signed tag only after the dry-run succeeds.
5. Confirm the tagged workflow promotes target-identical artifact names and
   marks an alpha/beta/rc tag as a pre-release.

No release step publishes providers, JIT, native plugins, or archived research
crates as Core SDK contracts.
