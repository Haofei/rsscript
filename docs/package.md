# Packages and external providers

A package separates source implementations (`.rss`), interface declarations
(`.rssi`), and provider bindings. Interface declarations are ordinary bodyless
functions. Their implementation technology is not part of RSScript syntax.

Host services are explicit package dependencies. Binding files use
`rsscript.bindings.v1`, map a symbol to a provider and entry point, and may include
optional review metadata. Missing or duplicate bindings and ABI mismatches are
link errors.

The compiler captures a `WorkspaceSnapshot` once, then derives analysis and
bytecode from that immutable source/interface graph. It emits
`rsscript.package_analysis.v1`, containing the shared `snapshot_digest`, the
executable `module_digest` when built, diagnostics, exports, semantic summaries,
and external symbols. It does not contain permission grants. Package
build-selection features remain package metadata and are not language features.

`rss build --analysis-out <analysis.json> <package-directory>` writes an Artifact
Bundle and optionally extracts the neutral
analysis artifact, and `rss inspect analysis --json <package-directory>` prints
it. Optional review is derived by the separate REIR integration rather than by
the product CLI.

Provider loading uses the platform-neutral types in `rsscript-abi-model` and the
registry contract in `rsscript-provider-api`. Semantic signatures are hashed from
parameter names, `read`/`mut`/`take`, canonical structured `WireType` values,
retention, result type, and sync/async mode. Provider ABI or signature mismatches fail before a
callable can be resolved. Native plugins are adapted to this contract after their
generated shim is loaded; the provider API itself has no native-loader dependency.

Optional package review uses the distinct `rsscript.package_review.v1` schema.
Risk, provider selection, native implementation details, and deployment evidence
must not appear in package analysis. The authoritative schemas are checked in
under `schemas/`.

Package graph snapshots, hashes, bounded file reads, reduced build environments,
and atomic artifact writes remain integrity/correctness controls. They do not
grant authority.

`BytecodeVerifier::verify(bytes)` owns envelope validation and returns a
`VerifiedBytecode` phase value; it no longer accepts a caller-supplied callback
that could define arbitrary payload validity. The VM performs its typed
instruction decode only after this phase succeeds and exposes no public
half-verified module constructor.

The `rsscript.bytecode.v1` envelope is a canonical sectioned binary format.
Header, structural import signatures, and executable instructions use
deterministic CBOR; every section has an explicit length and SHA-256 digest.
Equivalent metadata or instructions therefore have one accepted byte encoding,
and non-canonical encodings fail verification before VM deserialization.
