# Release and SDK distribution

RSScript is pre-1.0. A manual run of the `Release` workflow is the required
dry-run: it performs the full locked validation gate, builds all supported
targets, verifies checksums, and creates build-provenance attestations, but does
not create a GitHub Release. A matching tag promotes those same artifacts.

## Binary alpha releases

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

## Rust embedding SDK

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

## Release sequence

1. Update the two binary package versions and compatibility documentation.
2. Run the `Release` workflow manually and inspect all three target artifacts.
3. Verify each `SHA256SUMS-<target>` file and provenance attestation.
4. Create the exact matching signed tag only after the dry-run succeeds.
5. Confirm the tagged workflow promotes target-identical artifact names and
   marks an alpha/beta/rc tag as a pre-release.

No release step publishes providers, JIT, AOT, REIR libraries, native plugins,
or research crates as Core SDK contracts.
