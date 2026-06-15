# RSScript Package Manager Design — v0.6

Status: Implementation-aligned / review-graph ecosystem design
Version: 0.6
Based on: RSScript v0.6 language model, REIR v0.2 evidence specification
Audience: RSScript compiler implementers, package authors, native binding authors, registry implementers, CI/review-tool authors, AI coding-tool authors
Scope: package model, dependency resolution, Cargo integration, semantic dependency review, graph risk summary, native wrappers, review metadata, registry protocol direction, REIR capability-binding integration
Non-scope: RSScript core language semantics, full public registry product design, sandbox implementation, centralized trust policy, app-level specification DSL

### Changes from v0.5

```text
- Package review now tracks capability-binding chains via call-graph propagation.
- REIR adapter layer connects package review facts to evidence IR proofs.
- Capability bindings support unknown-reason tracking for incomplete analysis.
- S3 IAM scenarios validate the full review→evidence→reconciliation pipeline.
- Package review risk assessment integrates unknown-capability-binding count.
- Native risk detection targets structured adapter metadata (text scanning
  remains as fallback but is documented as heuristic, not semantic authority).
```

---

## 0. Reading Guide and Boundary Rule

RSScript package management is not a second language semantics layer. It consumes
RSScript `.rssi` contracts, compiler-normalized interface artifacts, review
metadata, and implementation/build metadata.

The package manager must not redefine or weaken these language-level rules:

```text
read / mut / take call-site checking
same-call conflict roots
constructor/variant call-like checking
freshness preservation
retention effects
managed closure capture retention
resource escape checks
ResourcePool factory contract
native / unsafe boundary declaration
frontend-first diagnostics and source mapping
```

If this document appears to conflict with the RSScript language specification,
the language semantic rule wins. This document specifies package artifacts,
dependency resolution, Cargo integration, package review metadata, native wrapper
checking, lockfiles, registry behavior, and review policy.

Implementation alignment note: the README's `Current CLI` section is the
authority for commands implemented by the current prototype. This design also
contains planned package-management commands and flags; those are labeled as
design targets or future extensions where they go beyond the implemented surface.
The canonical machine-readable-output flag for v0.6 documentation is `--json`.

### 0.1 Five package-management principles

The following five principles are normative for this design.

#### 1. Language / package-manager co-design

Package review is a language/package-manager co-design feature, not a
package-manager-only add-on.

The package manager can review dependency semantics only because RSScript makes
review-critical behavior part of the public `.rssi` contract:

```text
data effects: read / mut / take
return freshness
retention effects
resource ownership and ResourcePool contracts
native / unsafe boundaries
async review boundaries
runtime guarantees such as pure/no_panic/noalloc/no_block
public exported names, types, and call shapes
```

The compiler frontend parses and normalizes those contracts. The package manager
consumes the normalized effective interface to compute hashes, compare updates,
aggregate package risk, and enforce policy. It must not try to reconstruct
RSScript-facing semantics from Rust signatures, generated Rust, README files,
changelogs, or arbitrary source scans.

#### 2. Dependency update is a review event

A dependency update is not considered reviewable merely because version
resolution succeeds, semver permits it, the project still builds, or tests pass.

Before an update is accepted into the lockfile, tooling must be able to report:

```text
public contract delta
selected package feature delta
effective interface hash delta
package risk delta
native/build-time execution delta
implementation dependency delta
unknown fact delta
policy result
human-review reasons
```

Tests remain valuable, but they are not a substitute for knowing what changed.

#### 3. Summary-first, evidence-linked review

Package review must be summary-first and evidence-linked.

Tooling should not require reviewers to inspect all package source files before
understanding what changed. It must first report normalized public contract
deltas, direct dependency identities, selected feature deltas, package risk
deltas, native/build facts, implementation hash deltas, unknown facts, and
policy failures.

When source review is required, tooling should identify the reason and the
smallest relevant source artifact or region when possible.

Every review fact should record its evidence source where available:

```text
normalized .rssi
compiler review metadata
rsspkg.toml
rsspkg.lock
binding manifest
Cargo metadata
generated-adapter check
source scan best effort
audit evidence
registry metadata
not scanned / unknown
```

#### 4. Resolved graph does not imply reviewable graph

Dependency resolution success is not sufficient for package acceptance.

A package graph may resolve and build successfully while still being rejected as
unreviewable under project policy because of excessive transitive footprint,
unknown metadata, native/build-time execution, duplicate capabilities, generated
artifacts, or high-risk dependencies.

Package tooling must provide a graph-level risk summary that ranks installed
direct and transitive packages by review priority and explains the reasons and
evidence for each high, elevated, or unknown risk package.

#### 5. Machine-readable facts for CI and AI agents

Machine-readable review facts are first-class outputs.

Human-readable reports are views over structured facts. Commands that produce
review decisions, update plans, package risk summaries, or semantic diffs should
support stable machine-readable output for CI, registries, IDEs, and AI repair
agents.

AI agents should not be required to infer dependency changes from changelogs,
README files, arbitrary source diffs, or backend compiler errors when normalized
interface and review metadata are available.

### 0.2 Terminology

```text
interface content hash
    Hash of the compiler-normalized effective `.rssi` content. It excludes
    formatting, comments, private implementation files, tests, and review
    metadata.

effective interface
    The public `.rssi` surface after selected package features are applied.

effective interface hash
    Hash of the selected feature set, the package's interface content hash, and
    the public dependency interface identities that appear in the package's
    public surface. Used by lockfiles and registries to bind a dependency to a
    particular feature-shaped semantic surface, including exposed dependency
    contracts.

public dependency interface identity
    The normalized identity of a dependency interface that is exposed through a
    package's public surface: package name, selected feature set, effective
    interface hash, and the referenced public symbols. It is included in the
    consuming package's effective interface hash when dependency types or
    re-exports appear in public signatures.

fact acquisition mode
    How a review fact was obtained: manifest, normalized interface, lockfile,
    non-executing Cargo metadata, generated adapter check, source scan,
    author declaration, audit, sandbox observation, or unknown. Review-only
    commands may report unknown facts; they must not execute native build code
    to force a fact to become known.

package risk
    A package-level supply-chain/review tier computed from exports,
    implementation facts, native/build facts, and policy. It is not the same
    object as the language review-map classification of a function or file.

native conformance
    The degree to which a Rust native wrapper has been checked against its
    `.rssi` contract. Binding existence, adapter type-checking, semantic trust,
    and audit status are separate levels.

dependency update review
    A review of an update plan before build or run. It compares old and new
    effective interfaces, selected features, package risk, native/build facts,
    implementation hashes, lockfiles, and policy results.

audit surface
    The aggregate set of package APIs and dependency facts that require human or
    policy review: mutating APIs, retaining APIs, resources, async APIs, native
    APIs, unsafe APIs, build-time execution, native links, FFI, unknown facts,
    generated artifacts, and package features that enable any of those.

reviewable graph
    A dependency graph whose semantic contracts, implementation risk facts, and
    unknowns are sufficiently visible to satisfy project policy. A graph can be
    resolvable and buildable but not reviewable.

AI repair agent
    A tool that consumes machine-readable package review facts to plan source
    edits, dependency migrations, compatibility shims, targeted tests, or policy
    decisions. It is not trusted as an authority; it acts on facts produced by
    RSScript tooling.
```

---

## 1. Executive Summary

RSScript package management is not intended to replace Cargo.

The RSScript package manager is a semantic and review layer over:

```text
RSScript .rssi interfaces
RSScript .rss source packages
Rust native wrappers
Cargo-based implementation builds
```

The core model is:

```text
Package = reviewable semantic contract + implementation artifacts + computed risk metadata.
```

Cargo remains the build substrate for Rust code and Rust dependencies.

```text
Cargo owns:
  Rust crate dependency resolution
  crates.io integration for native Rust crates
  native wrapper compilation
  Cargo.lock
  target/platform handling
  workspace build mechanics
  build scripts and proc macros when an executing build command is run

RSScript package manager owns:
  .rssi semantic contracts
  RSScript package dependency resolution
  rsspkg.lock
  interface loading and compiler-owned normalization
  feature-conditioned effective interfaces
  semantic dependency diff
  computed package review metadata
  native boundary classification
  graph-level risk summary
  generated Cargo workspace glue
  source-map-aware diagnostic integration
```

Traditional package managers resolve dependency graphs. RSScript package
management reviews dependency semantics. A successful dependency resolution is
not, by itself, an acceptable review result.

A dependency update should answer:

```text
What public contracts changed?
Which APIs now mutate?
Which APIs now retain values or closure captures?
Which APIs now return or manage resources?
Which APIs now cross native or unsafe boundaries?
Which APIs expose review-visible async contracts?
Which package features introduced native, unsafe, build-script, proc-macro,
  FFI, native-link, resource, retention, or async risk?
Which native facts are checked, declared, scanned, audited, or unknown?
Which packages in the resulting graph deserve review first, and why?
Which dependency changes require human review before build or run?
```

The package manager must let a reviewer inspect a dependency graph before
executing that dependency's build-time code. Commands that run Cargo builds are
explicitly separated from commands that only parse manifests, load interfaces,
resolve metadata, compute risk, and generate review reports.

---

## 2. Design Thesis

RSScript exists because AI-era software shifts the bottleneck from writing code
to reviewing generated code. Package management must follow the same thesis.

Traditional package managers answer:

```text
Can this dependency graph be resolved?
Can this package be downloaded?
Can this code be built reproducibly?
```

RSScript package management must also answer:

```text
Can this dependency graph be reviewed?
Did an upgrade change semantic risk?
Did a package introduce hidden retention, mutation, resources, native code,
  unsafe code, async boundaries, or build-time execution?
Can the reviewer inspect the public contract without reading Rust implementation?
Can the reviewer tell which native facts are actually machine-checked?
Can an AI repair agent consume structured update facts before editing source?
```

This is a response to a common upgrade-by-hope failure mode in large package
ecosystems: a package update is accepted because version resolution succeeds and
tests pass, while runtime behavior, build-time execution, native dependencies,
transitive dependencies, or framework-specific contracts may have changed without
being visible as a package-level review event.

RSScript does not attempt to prove arbitrary behavioral equivalence. It makes the
review-relevant surface visible and comparable:

```text
normalized public contract
selected feature set
direct dependency identities
interface hashes
package risk
native/build facts
unknown facts
implementation hashes
lockfile changes
policy results
```

Therefore package management is part of the RSScript review story, not an
administrative layer outside it.

---

## 3. Goals and Non-goals

### 3.1 Primary goals

```text
provide a package format for RSScript libraries and applications
make .rssi files the public semantic contract of a package
support feature-conditioned effective interfaces
support pure RSScript packages
support Rust crate wrappers behind RSScript interfaces
reuse Cargo for Rust dependency resolution and native compilation
produce deterministic RSScript dependency locks
support semantic diff for package upgrades
support update review before build or run
generate machine-readable package review metadata
preserve source-map-aware diagnostics through generated Rust packages
make native/build/supply-chain risks visible before executing build code
summarize graph-level risk so reviewers know which packages deserve attention
separate computed risk from author-declared expectations
allow future registries to publish review summaries and semantic diff history
```

### 3.2 Secondary goals

```text
support local path dependencies before registry dependencies
support workspace development
support vendored/offline builds
support CI review policies and dependency budgets
support native risk classification for build scripts, proc macros, linked
  libraries, FFI, unsafe code, and transitive native facts
support optional native ABI adapter checking through generated Cargo adapters
make package publishing validate semantic interface consistency
support machine-readable facts for IDEs, CI, registries, and AI repair agents
```

### 3.3 Non-goals for MVP

```text
full Rust dependency resolver replacement
crates.io replacement in MVP
custom Rust build system
custom linker/toolchain management
native binary package manager
sandbox implementation for arbitrary native builds
package signing authority design
full registry moderation policy
full dependency quality/reputation scoring
automatic inference of RSScript effects from arbitrary Rust code
whole-program proof of Rust native wrapper semantic behavior
automatic proof that Rust code does not block, allocate, panic, spawn, or retain
app-level formal specification language
```

Important boundary:

```text
Rust implementation details may power a package.
Rust implementation details must not define the RSScript semantic contract.
```

---

## 4. Package Concepts

### 4.1 RSScript package

An RSScript package is a versioned unit that may contain:

```text
.rssi semantic interface files
.rss implementation files
Rust native wrapper code
rsspkg.toml
rsspkg.lock metadata
computed review metadata
tests
examples
generated artifacts with provenance
```

A package may be:

```text
library package
binary/tool package
interface-only package
native wrapper package
workspace package
```

Trust is not a package kind. Core/trusted status, if any, is a registry or
project policy result, not an author-declared package kind.

### 4.2 Semantic contract

The semantic contract of a package is its public `.rssi` surface after package
features are applied.

The `.rssi` contract declares:

```text
public roots and exported symbols
public types
public functions
parameter names
parameter types
read / mut / take data effects
return types
fresh returns
retention effects
resource APIs
native / unsafe effects
async signatures when review-visible
guarantees such as pure/no_panic/noalloc/no_block
```

The package manager treats the `.rssi` surface as the primary artifact for
review, compatibility, and dependency diff.

### 4.3 Effective interface

A package feature set produces an effective interface.

```text
effective interface = normalized public .rssi contract under selected package features
```

Rules:

```text
1. A package version may have multiple effective interfaces if package features
   expose optional APIs or optional boundary effects.
2. rsspkg.lock records the selected package feature set and the effective
   interface hash.
3. Feature resolution must not silently remove or weaken an already selected
   public API contract.
4. A feature may add APIs, add implementation choices, or add risk, but it must
   not hide a change to read/mut/take, retains, native, unsafe, resource, async,
   fresh, or guarantee semantics.
5. Any feature-conditioned change to a public contract is visible through the
   effective interface hash and semantic diff.
```

### 4.4 Implementation artifact

A package implementation may be written in:

```text
RSScript source
Rust native wrapper code
both RSScript and Rust
generated RSScript or adapter code with provenance
```

Implementation artifacts must conform to the public `.rssi` contract. For pure
RSScript implementation, conformance is checked by the RSScript compiler. For
native Rust implementation, conformance is split into explicit levels described
in Chapter 9.

### 4.5 Native wrapper

A native wrapper is Rust code that adapts one or more Rust crates or system
libraries to an RSScript `.rssi` contract.

```text
serde_json crate
  -> rss-json native wrapper
  -> json.rssi
  -> RSScript Json.parse API
```

The raw Rust crate API is not automatically exposed to RSScript. Only `.rssi`
APIs are visible.

v0.6 first-class native build integration is Rust/Cargo. Non-Rust libraries may
be wrapped through Rust native wrappers and must be surfaced through native/FFI,
native-link, build-script, system-library, or foreign-runtime facts when known.
The RSScript package manager does not resolve npm, pip, Go modules, system
package managers, or arbitrary foreign dependency graphs in v0.6.

### 4.6 Review metadata

Review metadata is generated from:

```text
compiler-normalized .rssi
RSScript source, if present
native package declarations
non-executing Cargo metadata, if native.rust is enabled and available
native binding manifests
policy configuration
generated artifact provenance
optional source scans or audits
```

It summarizes:

```text
file features
package features
public API count
mutating APIs
retaining APIs
closure-capture retaining APIs
resource APIs
fresh-returning APIs
async APIs
native APIs
unsafe RSScript APIs
unknown APIs
package risk level
graph-level risk contribution
semantic diff against previous version
native facts and their evidence source
native conformance level
generated artifact provenance
```

Review metadata is advisory and machine-readable. The authoritative contract is
still the `.rssi` surface plus package checksums and lockfile hashes. Review
metadata may be committed for convenience, but it is not trusted merely because
it appears in a package archive.

---

## 5. Package Types and Layout

### 5.1 Package types

Pure RSScript package:

```text
rss-math/
  rsspkg.toml
  interface/math.rssi
  src/math.rss
  tests/math_test.rss
```

Native wrapper package:

```text
rss-json/
  rsspkg.toml
  interface/json.rssi
  native/rust/Cargo.toml
  native/rust/src/lib.rs
  native/bindings.rssbind.toml
  tests/json_test.rss
```

Interface-only package:

```text
platform APIs
externally supplied runtime APIs
mock/test interfaces
cross-package contracts
```

Tool packages may contain binary entry points and stronger native/build risk
policies.

### 5.2 Recommended layout

```text
my-package/
  rsspkg.toml
  README.md
  LICENSE

  interface/
    lib.rssi

  src/
    lib.rss

  tests/
    smoke_test.rss

  examples/
    hello.rss

  review/
    package-review.json        # generated, optional to commit

  native/
    rust/
      Cargo.toml
      src/lib.rs
    bindings.rssbind.toml

  generated/                   # optional generated source artifacts
    README.md                  # provenance or generator notes
```

### 5.3 Generated build directory

The compiler/package manager may generate:

```text
target/packages/
  generated/
    Cargo.toml
    src/lib.rs
    src/main.rs
    rsscript-source-map.json
    package-review.json

  workspace/
    Cargo.toml
    packages/
    native/
```

Generated Rust is an internal build artifact. It is not the RSScript package
contract.

### 5.4 Interface-only packages and providers

An interface-only package provides contracts but no implementation. It is useful
for platform APIs, externally supplied runtime APIs, mock/test contracts, and
cross-package contracts.

Rules:

```text
1. Interface-only packages may be used for type checking, semantic review, and
   mock/test contracts.
2. Executable builds require an implementation provider unless the dependency is
   explicitly marked compile_only, test_only, or platform_provided.
3. Provider resolution is a package-resolution subproblem, not just a lockfile
   annotation.
4. rsspkg.lock records provider resolution for executable builds only after the
   selected provider has been matched against the selected interface contract.
5. A dependency on an interface-only package without a valid implementation
   provider is a diagnostic for rss run and rss verify-rust.
6. Interface-only packages publish the contract only. Executable consumers must
   either mark that dependency `platform_provided`, `compile_only`, or
   `test_only`, or select a package provider in `[providers]`.
```

Interface package example:

```toml
[package]
name = "platform-env"
version = "0.1.0"
edition = "2026"
kind = "interface-only"

[interfaces]
paths = ["interface"]
exports = ["Env"]
```

Provider package example:

```toml
[package]
name = "posix-env"
version = "0.1.0"
edition = "2026"
kind = "native-wrapper"

[implements."platform-env"]
version = "0.1"
interface_features = []
interface_effective_hash = "sha256:..."
```

Provider resolution rules:

```text
1. A provider package declares each interface package it implements under
   [implements."<interface-package-name>"].
2. The declaration names the interface version requirement, selected interface
   features, and the interface effective hash the provider was checked against.
3. During executable resolution, the resolver matches the requested
   interface-only package and selected feature set against available providers.
4. A provider is valid only if the provider's declared interface effective hash
   equals the requested interface effective hash, or the provider is rechecked
   locally and produces the same normalized interface contract.
5. If multiple valid providers are available and the root package or workspace
   policy does not choose one, provider resolution is ambiguous and diagnostic.
6. If a provider's own selected package features change the implementation
   risk, that risk is included in package review metadata and rsspkg.lock.
7. Provider implementation risk is not part of the interface package's effective
   interface hash. If one valid provider is substituted for another while the
   requested interface effective hash remains the same, the public RSScript
   contract is unchanged. The substitution is still a graph-risk and
   implementation review event and must be reported by update review.
8. Provider choice is executable-root scoped. The resolved executable graph has
   exactly one selected provider for each virtual/interface package identity.
   Dependency-local or consumer-local provider overrides are not defined in
   v0.6.
```

Root-level provider choice is expressed in the consuming package:

```toml
[dependencies]
platform-env = { path = "../platform-env" }
posix-env = { path = "../posix-env" }

[providers]
platform-env = { package = "posix-env", version = "0.1.0" }
```

Provider selection is intentionally not scoped to individual downstream
consumers. A reviewer should not need to ask which implementation a particular
dependency received: the executable graph has one provider answer per
virtual/interface package. Tests may use a different root manifest to select a
test provider, but that is a different executable graph, not an in-graph local
override.

If the target platform supplies the interface directly, the consumer marks that
dependency instead:

```toml
[dependencies]
platform-env = { path = "../platform-env", platform_provided = true }
```

---

## 6. Manifest: `rsspkg.toml`

### 6.1 Example

```toml
[package]
name = "rss-json"
version = "0.1.0"
edition = "2026"
description = "Reviewable JSON APIs for RSScript"
license = "MIT"
repository = "https://example.org/rss-json"
readme = "README.md"
kind = "native-wrapper"

[interfaces]
paths = ["interface"]
exports = ["Json"]

[interfaces.features.streaming]
paths = ["interface/streaming"]
exports = ["Json"]

[sources]
paths = ["src"]

[dependencies]
rss-core = "0.5"

[dev-dependencies]
rss-test = "0.5"

[features]
default = []
streaming = []

[review.policy]
deny_unknown = false
deny_native = false
deny_unsafe_apis = true
max_public_params = 8
max_nested_type_depth = 4
native_api_risk = "elevated"       # elevated | high
build_execution_default = "forbid" # forbid | review | allow

[review.expect]
risk = "elevated"                  # optional publish-time expectation, never authoritative

# Provider packages only:
# [implements."platform-env"]
# version = "0.1"
# interface_features = []
# interface_effective_hash = "sha256:..."

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
cargo_features = []

[native.rust.feature_map]
streaming = { cargo_features = ["streaming"] }

[native.rust.policy]
build_scripts = "forbid"             # forbid | review | allow
proc_macros = "forbid"               # forbid | review | allow
native_links = "review"              # forbid | review | allow
ffi = "review"                       # forbid | review | allow
rss_unsafe_apis = "forbid"           # forbid | review | allow
wrapper_unsafe_blocks = "review"     # forbid | review | allow
transitive_unsafe_blocks = "review"  # forbid | review | allow
```

### 6.2 `[package]`

Required:

```toml
[package]
name = "my-package"
version = "0.1.0"
edition = "2026"
```

Optional package kind:

```toml
kind = "library"        # library | binary | interface-only | native-wrapper | tool
```

If omitted, the package kind may be inferred from layout for local tooling, but
published packages should declare it explicitly.

### 6.3 `[interfaces]`

Declares where public `.rssi` files live. `exports` lists public roots
(for example `Json`), not every fully qualified symbol under that root.

```toml
[interfaces]
paths = ["interface"]
exports = ["Json"]
```

Feature-conditioned interfaces may be declared with:

```toml
[interfaces.features.streaming]
paths = ["interface/streaming"]
exports = ["Json"]
```

Rules:

```text
1. The package manager selects interface paths based on resolved package
   features.
2. The compiler frontend parses and normalizes all selected `.rssi` files.
3. Feature-conditioned public declarations are included in the effective
   interface hash.
4. The package manager must not implement an independent `.rssi` normalizer.
```

### 6.4 `[sources]`

Declares RSScript implementation source roots.

```toml
[sources]
paths = ["src"]
```

A native wrapper package may omit `[sources]` if all implementation is native
Rust behind `.rssi` contracts.

### 6.5 `[dependencies]` and `[dev-dependencies]`

Dependencies are RSScript packages, not arbitrary Rust crates.

```toml
[dependencies]
rss-core = "0.5"
rss-json = { version = "0.2", features = ["streaming"] }
my-local = { path = "../my-local" }
```

Rust crates belong in `native/rust/Cargo.toml`.

MVP dependency source forms:

```text
version requirement from registry or local index
local path dependency
```

Unsupported v0.6 dependency source forms:

```toml
my-git = { git = "https://example.org/my-git", rev = "abc123" }
```

Git dependencies are not part of the v0.6 accepted dependency-source grammar. If
such a key appears, tooling must reject it with a stable "unsupported dependency
source" diagnostic rather than attempting partial support or silently accepting a
future-looking manifest.

### 6.6 `[features]`

Package features select optional RSScript package APIs or implementation paths.

```toml
[features]
default = []
streaming = []
native-tls = []
```

Package features are not the same as RSScript file features such as
`features: local` or `features: native`.

Rules:

```text
1. Package features resolve deterministically.
2. Cargo-like additive feature unification is the default unless a package marks
   a feature as mutually exclusive through a future explicit mechanism.
3. The unified feature set is the effective feature set that gets normalized,
   hashed, reviewed, and locked.
4. A package feature must not silently introduce native or unsafe boundaries.
5. If a package feature enables native, unsafe, async, build scripts, proc
   macros, linked libraries, FFI, or additional resource/retention APIs, review
   metadata must report it.
6. A feature-conditioned public contract produces a different effective
   interface hash.
7. Consumer policy may reject a resolved graph because a forbidden feature was
   selected anywhere in the graph.
```

Consumer-side feature veto is intentionally separate from feature resolution.
The resolver may find a valid additive feature set, and policy may still reject
it as not reviewable:

```toml
[review.feature_policy]
deny = ["rss-http/native-tls", "*/unsafe-backend"]
```

A feature veto is not a request to silently remove the feature. If the graph
cannot be resolved without the denied feature, resolution is review-rejected and
must be changed by selecting another package, selected feature set, or provider.

No package override, feature-pinning, provider override, or patch mechanism is
defined in v0.6. Provider selection remains root-scoped for the whole executable
graph. A future override mechanism for package sources may be added only if it
preserves feature visibility, lockfile determinism, and review metadata for the
unified graph; consumer-local provider overrides are a non-goal.

### 6.7 `[review.policy]`

Package-level declared review policy.

```toml
[review.policy]
deny_unknown = true
deny_native = false
deny_unsafe_apis = true
max_public_params = 8
max_nested_type_depth = 4
native_api_risk = "high"           # elevated | high
build_execution_default = "forbid" # forbid | review | allow

[review.feature_policy]
deny = ["rss-http/native-tls", "*/unsafe-backend"]
```

`deny_unknown` controls whether unknown required review facts fail policy. Native
fact policy values such as `forbid` fail on known present facts; they do not by
themselves force review-only commands to execute native build code to prove a
fact absent.

This section is a policy, not a self-declared risk result. Computed metadata wins
over author expectations.

### 6.8 `[review.expect]`

Optional author expectation.

```toml
[review.expect]
risk = "low"        # low | elevated | high | unknown
```

`[review.expect]` is not authoritative and is not consumed as a safety fact by
dependent projects. It has exactly two uses:

```text
1. Local/publish feedback: package authors can detect that their package became
   riskier than intended.
2. Registry/publish policy: a registry may reject or warn on a package whose
   computed risk does not match its declared expectation.
```

Consumer policy must use computed local or registry-verified metadata, not the
author's expectation. A mismatch is reported as metadata context; it is not a
reason to trust the package and it is not itself a consumer-side package risk.

A registry must not display `[review.expect].risk` as the package's verified
risk.

### 6.9 `[native.rust]`

Declares Rust native wrapper integration.

```toml
[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
cargo_features = []
```

The package manager does not resolve Rust crates here. Cargo does.

Package features may map to Cargo features:

```toml
[native.rust.feature_map]
native-tls = { cargo_features = ["native-tls"] }
streaming = { cargo_features = ["streaming"] }
```

A Cargo feature map is implementation metadata. If it changes the RSScript-facing
public contract, the selected `.rssi` interface must also change through the
feature-conditioned interface mechanism.

### 6.10 `[native.rust.policy]`

Declares policy for native Rust facts.

```toml
[native.rust.policy]
build_scripts = "forbid"             # forbid | review | allow
proc_macros = "forbid"               # forbid | review | allow
native_links = "review"              # forbid | review | allow
ffi = "review"                       # forbid | review | allow
rss_unsafe_apis = "forbid"           # forbid | review | allow
wrapper_unsafe_blocks = "review"     # forbid | review | allow
transitive_unsafe_blocks = "review"  # forbid | review | allow
```

The policy values mean:

```text
forbid   fail if the fact is known present. If the fact is unknown, report the
         unknown fact; fail only when deny_unknown=true or another policy marks
         that fact as required-known.
review   allow, but classify and report the package as elevated or high risk
         when the fact is known present; report unknown facts separately.
allow    allow without policy error, still report known and unknown metadata.
```

This avoids forcing review-only commands into contradiction. A review-only
command must not execute native build code just because a policy would prefer to
know a fact. It reports `unknown` with an acquisition reason; project policy then
chooses whether that unknown is acceptable.

`rss_unsafe_apis` refers to `.rssi` functions that expose `effects(unsafe)`.
`wrapper_unsafe_blocks` refers to unsafe Rust in the native wrapper crate itself.
`transitive_unsafe_blocks` refers to unsafe Rust in Rust dependencies.

### 6.11 `[implements]`

Provider packages declare interface-only contracts they implement with
`[implements."<interface-package>"]`.

```toml
[implements."platform-env"]
version = "0.1"
interface_features = []
interface_effective_hash = "sha256:..."
```

Rules:

```text
1. The implemented package name identifies the interface-only package.
2. `version` is a version requirement over the interface package.
3. `interface_features` is the selected feature set of the interface contract the
   provider was checked against.
4. `interface_effective_hash` binds the provider to one exact normalized
   interface contract. It is verified during provider resolution.
5. If the declared hash is stale or does not match the requested interface hash,
   registry-only matching must reject the provider for that request. Local
   tooling may recheck the provider against the requested interface contract and
   accept it only if the recheck succeeds.
6. v0.6 does not define a way for a provider to declare "any
   contract-compatible hash". Compatibility ranges for provider declarations are
   future work.
7. A provider may implement multiple interface packages, but each implementation
   declaration is checked independently.
```

### 6.12 Optional graph budgets

A project may define graph budgets in root policy. This is optional for MVP but
part of the reviewable-graph model.

```toml
[dependency.budget]
max_direct_dependencies = 40
max_total_packages = 300
max_new_transitive_per_add = 25  # advisory transition budget
max_native_packages = 5
max_high_risk_packages = 0
max_unknown_packages = 0
max_build_execution_packages = 0
```

Budgets do not replace package risk. They express whether the whole resolved
graph remains reviewable under project policy.

Final-graph budgets such as `max_total_packages`, `max_native_packages`, and
`max_unknown_packages` are stable properties of the resolved graph. Transition
budgets such as `max_new_transitive_per_add` are evaluated against the proposed
change from the current lockfile and are inherently order-dependent; they are
workflow guards, not publish-time compatibility claims.

### 6.13 `[workspace]`

A workspace root may declare members:

```toml
[workspace]
members = [
  "packages/rss-json",
  "packages/rss-http",
  "tools/rss-review-ci",
]
```

Workspace support should be implemented after local path dependencies.

---

## 7. `.rssi` Contract, Interface Syntax, and Hashing

### 7.1 Public contract

Every RSScript-facing public API must be declared in `.rssi`.

v0.6 package-interface syntax uses fully qualified public symbols and opaque
interface types. There is no package-level `namespace` shorthand: package tooling
follows the compiler frontend's normalizer and must reject shorthand forms rather
than normalizing them.

A selected `.rssi` file is already a public semantic-contract artifact, so
package tooling treats declarations in it as contract entries. Matching source
implementations in `.rss` files must still be explicit `pub` declarations; a
private source type or function must not satisfy a public interface contract
unless the interface function is fulfilled by a declared native binding.

```rust
// interface/json.rssi

features: native

opaque struct Json.JsonValue
opaque struct Json.JsonError

native fn Json.parse(
    text: read String,
) -> Result<fresh Json.JsonValue, Json.JsonError>
    effects(native)

native fn Json.field_string(
    value: read Json.JsonValue,
    name: read String,
) -> Result<String, Json.JsonError>
    effects(native)
```

The normalized symbols are `Json.parse`, `Json.field_string`,
`Json.JsonValue`, and `Json.JsonError`. Package metadata and binding manifests
use these normalized symbols.

A Rust native wrapper API must be visible as a native boundary in `.rssi`;
otherwise review tools cannot distinguish pure RSScript implementation from
external Rust implementation.

### 7.2 Opaque interface types

An opaque interface type is a public RSScript type whose representation is not
specified by the `.rssi` contract.

```rust
opaque struct Json.JsonValue
opaque struct Json.JsonError
```

Rules:

```text
1. An opaque type may appear in public function signatures.
2. Its fields are not visible to dependents.
3. It is not equivalent to an empty struct.
4. A native wrapper or package implementation provides the representation.
5. Opaque resource types must be declared with resource syntax, not opaque struct.
6. Opaque types still obey ordinary RSScript kind rules: class, struct, resource,
   managed capability, freshness eligibility, and resource rules are determined
   by the declared kind.
```

### 7.3 Contract is semantic, not only type-level

```rust
pub fn Cache.put(
    cache: mut Cache,
    key: read String,
    value: read Image,
) -> Unit
    effects(retains(key), retains(value))
```

This tells the reviewer that `cache` is mutated and both `key` and `value` may be
retained.

### 7.4 Implementation must conform

If the implementation is Rust, the wrapper must conform to the `.rssi` contract.

Bad model:

```text
Rust function type determines RSScript effect semantics.
```

Correct model:

```text
.rssi declares RSScript semantics.
Rust wrapper is adapted and checked against that contract only to the level that
package tooling explicitly reports.
```

The package manager must not infer RSScript effects from arbitrary Rust code.

### 7.5 Compiler-owned normalization

The RSScript compiler frontend owns `.rssi` parsing and canonical semantic
normalization.

```text
rssc normalize-interface <package>
```

or an equivalent internal library API produces the canonical normalized
interface used for hashing, review metadata, and semantic diff.

Package tooling must not implement a separate normalizer that can drift from the
compiler. The package manager may cache normalized outputs, but the compiler
frontend is the authority.

### 7.6 Canonical interface form and hash algorithm

The compiler frontend emits a canonical normalized interface IR for the selected
`.rssi` files. Package tooling hashes that canonical IR; it must not hash source
text directly.

Normative hashing rules for v0.6:

```text
algorithm: SHA-256
encoding: UTF-8 canonical JSON emitted by the compiler frontend
schema tag: included as the first field, e.g. "rss.interface.v0.6"
object keys: sorted lexicographically
sets/maps: serialized as arrays sorted by canonical key
feature sets: sorted lexicographically by feature name
generic bounds: normalized by the compiler frontend and serialized in canonical order
qualified names: fully qualified and normalized by the compiler frontend
hash display: lowercase hex with `sha256:` prefix
```

A future binary canonical format may replace canonical JSON only with a schema
version bump. Cross-implementation hash stability is a conformance requirement.

### 7.7 Interface content hash

The interface content hash is the SHA-256 hash of the compiler-normalized public
`.rssi` content for the selected feature set.

Included in the interface content hash:

```text
exported public roots
type names and kinds
opaque markers
field names, field modes, weak/handle markers
public function names
parameter names
parameter types
data effects
return types
fresh markers
effects clauses including retains/native/unsafe/guarantees
async signature markers
generic bounds
resource declarations
constructor/variant contract shape where public
feature-gated public declarations included in the effective interface
```

Not included:

```text
comments
formatting
private implementation files
non-public test interfaces
review metadata
package archive checksum
Cargo.lock
native source hash
```

The interface content hash is useful for detecting that two package versions have
the same direct public contract text after compiler normalization.

### 7.8 Public dependency interface identity

A package public surface may expose dependency contracts by:

```text
using dependency types in public signatures
returning dependency opaque/resource/class/struct types
re-exporting dependency public roots
aliasing dependency types in exported declarations
using dependency generic/resource bounds in public contracts
```

When a dependency interface appears in the public surface, the package's
semantic contract depends on that dependency's selected effective interface. The
compiler frontend must therefore report a `public_dependency_interfaces` list as
part of normalized interface metadata.

Each entry contains:

```text
dependency package name
dependency package version or package identity selected by resolution
selected dependency feature set
dependency effective interface hash
referenced public symbols or roots
```

Private implementation dependencies are not included in this list unless their
contracts appear in the public surface. They are still reported as ordinary
implementation or dependency changes.

The list contains only dependency interfaces directly referenced by this
package's public surface. It does not recursively expand the full transitive
closure. A directly referenced dependency's effective interface hash already
covers any deeper dependency contracts that it exposes publicly. This keeps the
hash composable and avoids redundant churn.

### 7.9 Effective interface hash

The effective interface hash binds the selected package feature set, the package
interface content hash, and every public dependency interface identity exposed by
the public surface.

```text
effective_interface_hash = sha256(canonical_json({
  schema: "rss.effective_interface.v0.6",
  package: package_name,
  selected_features: sorted_feature_set,
  interface_content_hash: interface_content_hash,
  public_dependency_interfaces: sorted_public_dependency_interface_identities
}))
```

The package version, source URL/path, archive checksum, private implementation
source hash, and Cargo.lock are recorded separately in `rsspkg.lock`. They are
not folded into the direct interface content hash. Public dependency interface
identities are folded into the effective interface hash because they are part of
the exposed semantic contract.

Provider package identity, provider selected features, provider implementation
risk, native source hashes, and Cargo.lock state are not folded into the
interface effective hash. They are implementation and graph-risk facts recorded
in `rsspkg.lock` and review metadata. A provider substitution can therefore be a
required review event without being a public contract change.

If a dependency interface changes but none of its types or roots appear in this
package's public surface, this package's effective interface hash need not
change. The update is still reportable as a private dependency or implementation
change if the package depends on it.

---

## 8. Dependency Graphs, Resolution, and Lockfiles

### 8.1 Graph layers

RSScript package management has three dependency graphs:

```text
1. Semantic package graph
   RSScript packages and effective .rssi contracts

2. RSScript implementation graph
   .rss source packages and interface imports

3. Native Rust graph
   Cargo crates used by native wrappers
```

The RSScript package manager resolves graphs 1 and 2. Cargo resolves graph 3.
The package manager still summarizes graph 3 as implementation risk when native
wrappers are present.

### 8.2 Resolution order

```text
1. Read root rsspkg.toml.
2. Resolve RSScript package dependencies.
3. Fetch packages or use local paths.
4. Verify checksums if locked.
5. Resolve package features.
6. Load selected .rssi interfaces.
7. Ask the compiler frontend to normalize effective interfaces.
8. Compute interface content hashes and effective interface hashes.
9. Build effective interface environment.
10. Check RSScript sources.
11. Generate review metadata.
12. Compute graph-level risk summary.
13. Generate build plan.
14. Delegate native graph to Cargo only when a command requires native build or
    native metadata inspection.
```

### 8.3 Version requirements

MVP version requirement forms:

```toml
rss-core = "0.5"
rss-json = "^0.1"
rss-http = { version = ">=0.2, <0.4" }
local-lib = { path = "../local-lib" }
```

Recommended MVP rule:

```text
A package graph should resolve to one version per package name.
```

Future versions may support multiple major versions with explicit qualified-root
disambiguation.

### 8.4 Feature resolution

RSScript package features resolve deterministically.

Default MVP rule:

```text
Feature unification is additive, like Cargo features.
```

The unified feature set is the only effective feature set used for normalization,
hashing, review metadata, and lockfile recording. If two dependency paths select
different feature sets of the same package, the package is normalized and hashed
with the union. That unified interface may be larger or riskier than either
importer individually requested; the update/add plan must report which paths
caused each selected feature.

Consumer-side feature policy may reject the graph after resolution:

```toml
[review.feature_policy]
deny = ["rss-http/native-tls", "*/unsafe-backend"]
```

A denied feature produces a policy failure. Tooling must not silently remove the
feature from the unified set, because doing so could weaken another dependency's
assumptions.

Review metadata must indicate when a feature changes risk.

Example:

```text
rss-http feature native-tls enabled by rss-client
  -> unified effective interface includes native-tls-selected public contracts
  -> effective interface hash changes if public APIs or public effects change
  -> package risk elevated or high depending on native policy
  -> native dependency graph may change
  -> graph policy may reject rss-http/native-tls
```

### 8.5 Interface environment

The checker receives an interface environment assembled from:

```text
bundled core interfaces
root package interfaces
resolved dependency effective interfaces
explicit user-supplied interfaces
```

Duplicate exported symbols, incompatible exports, and ambiguous package roots are
diagnostics.

### 8.6 Add/update plan before lockfile acceptance

Adding or updating a dependency is a review event. Tooling should compute an add
or update plan before accepting the change into `rsspkg.lock`.

An add/update plan reports:

```text
direct dependency changes
new or removed transitive RSScript packages
selected feature changes
public contract hash changes
package risk changes
native/build fact changes
unknown fact changes
graph-level risk summary changes
policy result
```

A command may support explicit apply mode:

```sh
rss pkg add rss-http
rss pkg add rss-http --apply
rss pkg diff --update-plan
```

The exact UX is tool-defined, but policy failures must be visible before the
lockfile is accepted.

### 8.7 Resolved versus reviewable

A graph can be:

```text
resolved      all package versions and features have been selected
locked        rsspkg.lock records the selected graph and hashes
buildable     Cargo/Rust implementation build can run or has run successfully
reviewable    semantic contracts and risk facts satisfy project policy
```

`resolved` does not imply `reviewable`.

Examples of policy reasons to reject a resolved graph:

```text
unknown package risk
high-risk dependency exceeds budget
new build-time execution
new native or FFI dependency
excessive transitive footprint
duplicate capability beyond policy
missing review metadata for selected features
```

### 8.8 Two lockfiles

RSScript and Cargo have different lock responsibilities.

```text
rsspkg.lock   RSScript semantic dependency lock
Cargo.lock    Rust implementation dependency lock
```

A single lockfile would mix semantic contract resolution with implementation
crate resolution.

### 8.9 `rsspkg.lock` authoritative fields

`rsspkg.lock` records authoritative dependency state:

```text
resolved RSScript package graph
package name and version
package source: registry/path/vendor
package archive checksum when applicable
selected package features
interface content hash
effective interface hash
public dependency interface identities exposed by the package surface
native wrapper source hash, if native.rust is enabled
native binding manifest hash, if present
implementation source hash for published pure RSScript packages, when available
generated artifact hashes, when generated artifacts are part of the package
provider resolution for interface-only packages used in executable builds:
  interface package, interface selected features, interface effective hash,
  provider package, provider selected features, provider effective hash,
  provider implementation risk summary
```

`rsspkg.lock` may also record advisory/cache fields:

```text
review metadata hash
review metadata schema version
review tool version
native metadata summary hash
graph risk summary hash
```

Advisory fields must be labeled as advisory. A review metadata hash change alone
must not be presented as a public contract change.

### 8.10 `Cargo.lock`

`Cargo.lock` records Rust crate resolution. Applications using native wrappers
should commit `Cargo.lock` when reproducibility matters.

A native wrapper update may change `Cargo.lock` even when `.rssi` is unchanged.
Package diff must report this as an implementation/native dependency change, not
as a public RSScript contract change.

### 8.11 Update behavior

On update, the package manager should report:

```text
RSScript package version changes
interface content hash changes
effective interface hash changes
selected feature changes
review metadata changes and whether they are schema/tool-only changes
native wrapper source changes
binding manifest changes
generated artifact changes
provider substitutions for interface-only packages
Cargo.lock changes
semantic diff summary
graph risk summary delta
```

Provider substitution rule:

```text
If an interface-only dependency keeps the same interface effective hash but its
selected provider changes, the update has no public contract delta. It must still
be reported as a provider/implementation change with provider risk delta,
selected provider features, evidence sources, and policy result.
```

Example:

```text
rss-json 0.1.0 -> 0.1.1
  .rssi unchanged
  interface content hash unchanged
  effective interface hash unchanged
  native/rust changed
  Cargo.lock changed: serde_json 1.0.120 -> 1.0.125
  semantic contract change: none
  review required: native implementation update
```

---

## 9. Cargo and Native Wrapper Integration

### 9.1 Core rule

```text
Cargo is the Rust implementation substrate.
RSScript package management is the semantic/review layer.
```

Cargo owns:

```text
Rust crate dependency resolution
crates.io dependency fetching
Cargo.lock
Rust feature unification
workspace compilation
build scripts
proc macros
target triples
platform cfg
native linking
incremental compilation
```

RSScript package manager owns:

```text
RSScript package dependency resolution
.rssi interface loading and normalization through the compiler frontend
RSScript semantic lockfile
semantic package diff
review metadata generation
native boundary classification
graph-level risk summary
generated Rust package assembly
source-map-aware diagnostic integration
```

Cargo graph buildability is not RSScript graph reviewability. A Cargo graph may
compile successfully while still increasing review risk through native wrappers,
build scripts, proc macros, FFI, native links, unsafe implementation,
feature-induced transitive dependencies, generated adapter code, or excessive
dependency footprint.

### 9.2 Build pipeline

```text
rsspkg.toml
  -> resolve RSScript package graph
  -> fetch/load .rssi interfaces
  -> normalize effective interfaces with compiler frontend
  -> check RSScript source against interfaces
  -> lower RSScript to Rust source
  -> generate Cargo package/workspace glue
  -> include native/rust wrapper crates as path dependencies
  -> invoke cargo check/build/run when the command requires it
  -> remap rustc diagnostics through RSScript source maps
```

### 9.3 Review-without-execution rule

Package review must be possible without executing untrusted native build code.

Commands that must not execute build scripts, proc macros, or native build code
by default:

```text
rss pkg review
rss pkg metadata
rss pkg diff
rss pkg tree
rss pkg audit-surface
rss pkg lock --check
```

Commands that may execute the Cargo build pipeline:

```text
rss run
rss verify-rust
rss pkg check --native-abi
rss pkg publish --dry-run --native-abi
```

Before a command executes native build code, it must apply native risk policy and
surface the facts that are known without execution.

Review-only commands may use only non-executing fact sources:

```text
rsspkg.toml and dependency manifests
selected .rssi contracts and compiler-normalized interfaces
rsspkg.lock and existing Cargo.lock files
registry metadata and package checksums
binding manifests
non-executing Cargo metadata, if the Cargo invocation is guaranteed not to run
  build scripts, proc macros, or native build code
previously committed review metadata, labeled as cache/advisory unless verified
```

If a fact is knowable only by running a build, running a build script, expanding
a proc macro, compiling generated adapters, probing a system library, or scanning
source that is unavailable, a review-only command reports that fact as `unknown`
with an acquisition reason. It must not silently execute native build code to
turn the unknown into a known value.

Examples:

```text
Cargo.lock delta
  known if old/new Cargo.lock files are supplied or a non-executing Cargo
  resolution is performed; otherwise unknown.

build_scripts/proc_macros/native_links
  often knowable from Cargo metadata; if metadata cannot be obtained without
  execution, report unknown.

transitive_unsafe_blocks
  known only if a source scan or trusted audit was run for the exact selected
  graph; otherwise unknown.

adapter_typechecked
  known only after rss pkg check --native-abi or equivalent build execution;
  otherwise not_run.
```

Review-only unknowns are not safe. They are structured facts that policy can
allow, warn on, or reject.

### 9.4 Native wrapper Cargo.toml

A native wrapper package owns a normal Rust crate:

```text
native/rust/Cargo.toml
native/rust/src/lib.rs
```

Example:

```toml
[package]
name = "rss_json_native"
version = "0.1.0"
edition = "2024"

[dependencies]
serde_json = "1"
```

The `edition = "2026"` in `rsspkg.toml` is the RSScript package/language edition.
The `edition = "2024"` in `native/rust/Cargo.toml` is the Rust crate edition.
They are intentionally separate.

### 9.5 Binding manifest

A native wrapper package may provide:

```toml
# native/bindings.rssbind.toml

[bindings]
"Json.parse" = "rss_json_native::json_parse"
"Json.field_string" = "rss_json_native::json_field_string"

[types]
"Json.JsonValue" = "rss_json_native::JsonValue"
"Json.JsonError" = "rss_json_native::JsonError"
```

A whole boundary that binds many functions of one namespace to a single Rust
wrapper crate can be declared compactly with an `[adapter.<Namespace>]` section
instead of one `[bindings]` line per function:

```toml
# native/bindings.rssbind.toml

[adapter.Json]
crate = "rss_json_native"
functions = ["parse", "field_string"]

# Per-method overrides when the Rust name differs from the RSScript method:
[adapter.Json.rename]
parse = "json_parse"
field_string = "json_field_string"
```

This expands at load time to exactly the `[bindings]` entries above
(`Json.parse -> rss_json_native::json_parse`, …), so lowering, the VM shim, and
all binding checks see the identical flat map — there is no separate adapter code
path. Every bound method is still listed by name, keeping the boundary
review-visible; only the repeated `Namespace.` prefix and `crate::` path are
factored out. A symbol defined by both an adapter and an explicit `[bindings]`
entry (or by two adapters) is rejected as a duplicate.

Package checks must reject:

```text
bindings whose RSScript symbol is not declared by package .rssi
bindings to non-native .rssi functions unless explicitly allowed by contract metadata
bindings whose Rust target does not live under configured [native.rust].crate
missing bindings for bodyless native functions needed by the package
missing type bindings for opaque/native-backed types needed by native functions
```

The binding manifest is part of native review metadata and native hashing.

### 9.6 Type bridge

Common bridge types:

```text
RSScript String        <-> Rust String / &str through adapter views
RSScript Bytes         <-> Vec<u8> / &[u8]
RSScript Buffer        <-> Vec<u8> / wrapper buffer
RSScript Result        <-> Rust Result through adapter mapping
RSScript Option        <-> Rust Option
RSScript resource      <-> Rust type implementing rss_rt::Resource
RSScript class/managed <-> rss_rt::Managed<T>
RSScript read/mut views <-> adapter-managed read/write views
```

The bridge is not a general FFI surface. It is generated for declared `.rssi`
contracts and native binding manifests.

Native implementation features are not RSScript language features. If a wrapper
maps a package or target choice to Cargo features, package metadata records the
selected native Cargo features, target, and risk reason separately from
RSScript `features:` declarations. For example, a Rayon wrapper may expose an
RSScript package feature or target profile that selects the native Cargo feature
`rayon/web_spin_lock` for a wasm browser build, but RSScript source must not
observe `web_spin_lock` as a language capability.

### 9.7 Native conformance levels

Native wrapper conformance is reported as independent facets. A higher facet
must not imply that arbitrary Rust semantic behavior has been proven.

```text
Level 0: native boundary declared
  - .rssi marks native functions with effects(native).

Level 1: binding existence checked
  - every required native .rssi function has a binding.
  - every required opaque/native-backed type has a binding.
  - binding targets are inside the configured native crate.

Level 2: adapter type-checked
  - package tooling generates bridge adapters from .rssi to Rust.
  - cargo check succeeds for the generated adapter and native crate.
  - this may execute build scripts/proc macros and is not run by default in
    review-only commands.

Level 3D: semantic declarations recorded
  - native wrapper author declarations record whether the implementation may
    retain, block, allocate, panic, spawn, execute worker-thread parallelism, use
    env, access filesystem/network, or call FFI.
  - these are declarations, not machine proof.

Level 3S: semantic source scan recorded
  - best-effort scans record facts such as unsafe blocks, obvious FFI usage,
    build script presence, native parallel backends such as Rayon, or suspicious
    IO calls.
  - scans are tool-specific, may be incomplete, and must be labeled with source,
    tool, version, and selected graph.

Level 4: audited conformance
  - external audit/test evidence is attached and hash-pinned.
  - registry may display audit status separately from computed metadata.
```

Package metadata must display declaration, scan, generated-adapter, and audit
sources separately. A UI must not collapse `author_declaration` and
`source_scan_best_effort` into the same trust claim merely because both are
Level 3 evidence.

### 9.8 Adapter checking

Native ABI adapter checking is optional in the MVP but should be the standard for
publish readiness.

Conceptual generated adapter:

```rust
fn __rss_bind_Json_parse(
    text: rss_rt::Read<String>,
) -> rss_rt::Result<rss_rt::Fresh<JsonValue>, JsonError> {
    rss_json_native::json_parse(text.as_str()).map(/* bridge */)
}
```

Command:

```sh
rss pkg check --native-abi
```

This command may run Cargo and therefore may execute build scripts or proc macros
according to Cargo's normal behavior. Policy must be applied first.

### 9.9 Native risk facts

Native wrapper metadata classifies facts, but every fact must carry an evidence
source.

Structural facts commonly available from Cargo metadata:

```text
build.rs present
proc-macro crates present
links key present
transitive Rust dependency graph
Cargo features selected
```

Facts usually not provable from Cargo metadata alone:

```text
uses unsafe
performs blocking IO
spawns threads
uses environment variables
uses filesystem/network during build
performs FFI through generated or macro-expanded code
retains managed handles correctly or incorrectly
preserves noalloc/no_block/pure/no_panic guarantees
```

Native risk fact schema:

```json
{
  "name": "build_scripts",
  "value": true,
  "source": "cargo_metadata_nonexecuting",
  "scope": "transitive"
}
```

Possible `value`:

```text
true
false
unknown
not_run
```

Possible `source`:

```text
manifest
normalized_interface
rsspkg_lock
cargo_metadata_nonexecuting
binding_manifest
generated_adapter_check
source_scan_best_effort
author_declaration
audit
sandbox_observation
not_scanned
build_required
tool_unsupported
metadata_unavailable
```

Unknown facts should include an acquisition reason:

```json
{
  "name": "transitive_unsafe_blocks",
  "value": "unknown",
  "source": "not_scanned",
  "scope": "transitive",
  "acquisition": "source_scan_not_run_for_selected_cargo_graph"
}
```

### 9.10 Native risk categories

Native wrapper metadata should classify:

```text
RSScript APIs with effects(native)
RSScript APIs with effects(unsafe)
Rust wrapper unsafe blocks
transitive Rust unsafe blocks
FFI usage
build.rs
proc macros
links native library
system libraries
dynamic loading
foreign runtime
blocking IO
thread spawning
async runtime usage
environment variable access
filesystem/network access during build
```

Build scripts and proc macros are native execution boundaries.

Recommended MVP default:

```toml
[native.rust.policy]
build_scripts = "review"
proc_macros = "review"
native_links = "review"
ffi = "review"
rss_unsafe_apis = "forbid"
wrapper_unsafe_blocks = "review"
transitive_unsafe_blocks = "review"
```

---

## 10. Package Risk Aggregation

### 10.1 Relationship to language review classifications

The RSScript language review map classifies code regions as:

```text
unknown
must_review
review_if_changed
low_semantic_risk
```

Package risk is a separate aggregate tier:

```text
unknown
high
elevated
low
```

The two are related but not identical. For example, a public API may be
language-level `must_review` because it is a public contract, while the containing
package may still be package-level `low` if no elevated/high/unknown package-risk
facts are present under policy.

### 10.2 Risk precedence

Package risk is single-valued and chosen by precedence:

```text
1. unknown
2. high
3. elevated
4. low
```

Reasons are a list and never collapse. A package may be classified `unknown` with
reasons such as `native_unsafe_not_scanned` and `public_native_api`.

### 10.3 Unknown risk

A package risk is `unknown` if:

```text
any exported public contract cannot be parsed or classified
any dependency's required metadata is missing under policy
a fact marked required-known by policy is unknown
an implementation provider is missing for an executable interface-only dependency
native binding targets cannot be resolved in review-only mode and policy requires
  them to be known
a registry checksum or lockfile hash is missing or mismatched
a semantic diff cannot be computed for an updated dependency
```

Unknown must not be treated as safe. Policy may fail an operation because risk is
unknown, but the displayed risk remains `unknown`.

### 10.4 High risk

A package risk is at least `high` if:

```text
any exported RSScript API has effects(unsafe)
policy maps native APIs to high and the package exports native APIs
build scripts, proc macros, FFI, or native links are present under a policy that
  treats them as high-risk
wrapper unsafe blocks are detected under high-risk policy
critical guarantees are removed from public APIs in an update
```

### 10.5 Elevated risk

A package risk is at least `elevated` if:

```text
any exported API has effects(native) and policy does not map it to high
any exported API exposes review-visible async signatures
any exported API uses local/resource/ResourcePool/retains/mut/take behavior that
  requires review but is bounded by visible contracts
native implementation changed while .rssi is unchanged
Cargo.lock changed for a native wrapper dependency
package features add native/build/proc-macro/resource/retention/async risk
managed closure capture retention appears in exported behavior
```

### 10.6 Low risk

A package risk may be `low` only if all of the following hold:

```text
all exported APIs are parseable, classifiable, and non-unknown
public API must-review reasons are limited to baseline public-contract review
  accepted by policy
no native APIs
no unsafe APIs
no async APIs
no ResourcePool/resource APIs
no retaining APIs
no mut/take public APIs unless policy explicitly permits them as low for the project
no unknown APIs or unknown required facts
no build scripts/proc macros/native links/FFI in the selected package graph
```

This rule intentionally distinguishes language-level public API review from
package-level risk rollup.

### 10.7 Computed risk and declared expectation

Computed risk is authoritative for tooling decisions.

```text
computed_risk != declared_expectation
```

is reported as a metadata mismatch, not treated as a reason to trust the declared
expectation.

---

## 11. Graph Risk Summary and Audit Surface

### 11.1 Purpose

A resolved graph is itself a review artifact. Tooling must help reviewers answer:

```text
Out of all installed direct and transitive packages, which ones deserve attention first, and why?
```

This does not require a full graph-governance system in the MVP. At minimum,
package tooling should summarize the highest-risk packages in the current graph
and explain the reasons and evidence.

Planned canonical command once graph-audit output is implemented:

```sh
rss pkg audit-surface
rss pkg audit-surface --json
```

### 11.2 Minimum risk summary

A graph risk summary should report:

```text
total package count
direct dependency count
transitive dependency count
risk distribution: unknown/high/elevated/low
highest-risk direct and transitive packages
reason list for each high/elevated/unknown package
dependency path that introduced each package
evidence source for each reason when available
policy result
```

At minimum, it should identify packages with:

```text
unknown risk
high risk
native or FFI boundaries
unsafe RSScript APIs
build-time execution such as build scripts or proc macros
resource APIs
retaining APIs or retained closure captures
review-visible async APIs
changed native implementation with unchanged public contract
missing or stale review metadata
```

### 11.3 Example output

```text
Project dependency risk summary

total packages: 184
direct dependencies: 23
transitive dependencies: 161

risk distribution:
  unknown: 2
  high: 3
  elevated: 18
  low: 161

highest-risk packages:

1. rss-http 0.4.0 [high]
   path: my-app -> rss-http
   reasons:
     - native APIs
     - resource API: Response.body_stream
     - build-time execution in native Cargo graph
   evidence:
     - .rssi
     - cargo_metadata_nonexecuting

2. rss-sqlite 0.2.1 [high]
   path: my-app -> rss-db -> rss-sqlite
   reasons:
     - FFI/native link: sqlite3
     - native wrapper unsafe blocks detected
     - resource API: Sqlite.Connection
   evidence:
     - .rssi
     - source_scan_best_effort
     - cargo_metadata_nonexecuting

3. rss-legacy-xml 0.1.0 [unknown]
   path: my-app -> rss-importer -> rss-legacy-xml
   reasons:
     - missing normalized interface metadata
     - native facts not scanned
   evidence:
     - not_scanned

policy: fail
  unknown packages: 2, policy maximum: 0
  high-risk packages: 3, policy maximum: 0
```

### 11.4 Relationship to budgets

Budgets are optional policy. A graph can fail policy because its risk summary
contains forbidden or excessive risk even when every package resolves.

Examples:

```text
too many high-risk dependencies
unknown packages present
new build-time execution introduced
native package count exceeds policy
transitive footprint exceeds policy
duplicate capability exceeds policy
```

The exact graph-cost formula is intentionally not standardized in v0.6. Tools
may experiment, but they must report raw facts separately from any score.

---

## 12. Package Check and Build Workflow

### 12.1 `rss pkg check`

Canonical command namespace is `rss pkg`. v0.6 does not define command aliases.

Default `rss pkg check` runs:

```text
manifest validation
interface parse/check through compiler frontend
compiler-owned interface normalization
interface hash computation
RSScript source check, if sources exist
pure RSScript implementation-vs-interface conformance check
native binding declaration check
review metadata generation
graph risk summary generation
non-executing Cargo metadata scan if native.rust enabled and metadata is available without build execution
rsspkg.lock consistency check
policy checks that do not require native build execution
```

Default `rss pkg check` must not execute native build scripts or proc macros.

To run generated native adapter type-checking:

```sh
rss pkg check --native-abi
```

This command may invoke Cargo and therefore may execute build-time Rust code.
Policy must be applied before execution.

When `[review.policy].deny_unknown = true`, any package review result with
unknown risk makes package check fail even if the lock is current and there are
no frontend errors.

### 12.2 Pure RSScript application

```text
rss check
  -> load rsspkg.toml
  -> load dependency .rssi
  -> normalize effective interfaces
  -> check .rss source
```

```text
rss run
  -> check
  -> lower to Rust
  -> generate Cargo package
  -> cargo run
  -> remap diagnostics
```

### 12.3 Application with native wrappers

```text
rss run
  -> resolve RSScript package graph
  -> load .rssi contracts
  -> normalize effective interfaces
  -> check RSScript source
  -> generate Rust package
  -> include native/rust crates as Cargo path dependencies
  -> apply native build policy
  -> cargo run
  -> remap rustc diagnostics
```

### 12.4 CI workflow

Recommended review-first CI:

```sh
rss pkg check
rss pkg tree
rss pkg review --json
rss pkg metadata --dry-run --json
rss pkg diff old-package-dir new-package-dir
rss check src/main.rss
rss review --map src/
```

Native ABI CI:

```sh
# planned native ABI extension:
# rss pkg check --native-abi
rss verify-rust src/main.rss
```

Strict dependency policy:

```sh
# planned policy-flag extension; today use manifest policy plus --json output:
rss pkg check --json
rss pkg review --json
rss pkg diff --json old-package-dir new-package-dir
```

---

## 13. Semantic Dependency Diff and Update Review

### 13.1 Purpose

A package update should produce a semantic diff of public contracts and relevant
implementation/native facts.

Canonical command:

```sh
rss pkg diff <old-package-directory> <new-package-directory>
```

Examples:

```sh
rss pkg diff --json old-package-dir new-package-dir
rss pkg diff --reir old-package-dir new-package-dir
rss pkg review update --json --from old/rsspkg.lock --to rsspkg.lock
rss pkg reir diff --json --fail-on-change --from review/reir-baseline.json --to review/reir/rsscript.json
```

Registry-coordinate diffs, `--lockfile`/`--new-lockfile`, and update-plan diff
UX are design targets, not part of the implemented v0.6 prototype surface.

### 13.2 Diff inputs

```text
old rsspkg.lock
new rsspkg.lock
old normalized effective .rssi contracts
new normalized effective .rssi contracts
old computed review metadata
new computed review metadata
old native binding/native source hashes
new native binding/native source hashes
old provider resolution, if interface-only packages are executable dependencies
new provider resolution, if interface-only packages are executable dependencies
old graph risk summary
new graph risk summary
Cargo.lock changes if native wrappers are present
```

### 13.3 Breaking or must-review changes

```text
public function removed
public type removed
public root/exported symbol removed
parameter added
parameter removed
parameter renamed
parameter type changed
parameter effect read -> mut
parameter effect read/mut -> take
return type changed
fresh guarantee removed
retains effect added
managed closure capture retention introduced
ResourcePool factory contract changed
ResourcePool factory changed from eager/noescape to retained/lazy
native effect added
unsafe effect added
async API added or async boundary changed
resource return introduced
resource lifetime behavior changed
constructor/variant field or payload effect changed
handle/weak field marker changed
guarantee removed such as no_panic/noalloc/no_block/pure
opaque/public type kind changed
unknown classification introduced
```

### 13.4 Review-relevant but possibly compatible changes

```text
new public function added
new public type added
new public root/exported symbol added
fresh guarantee added
retains effect removed
guarantee added
native implementation changed with unchanged .rssi
native binding manifest changed
Cargo.lock changed for native wrapper package
package risk increased from low to elevated/high/unknown
new package feature changes native/build/proc-macro/resource/retention/async risk
review metadata changed because risk algorithm/schema changed
provider substituted while interface effective hash is unchanged
provider implementation risk changed while interface effective hash is unchanged
graph risk summary changed even if the changed package's .rssi is unchanged
```

### 13.5 No public contract delta / low semantic-contract risk changes

The following changes have no public RSScript contract delta:

```text
comments changed
formatting changed
private implementation changed with unchanged interface and no native change
new tests/examples added
review metadata regenerated with no semantic delta
review metadata changed only because tool/schema version changed
```

This does not prove behavioral safety or implementation correctness. It means
only that the public RSScript semantic contract did not change. Projects may
still require source review, tests, audits, or native implementation review.

### 13.6 Update review result

An update review report should contain:

```text
package version changes
public contract delta
selected feature delta
effective interface hash delta
package risk delta
native/build-time execution delta
implementation/native dependency delta
unknown fact delta
provider resolution delta
provider risk delta
graph risk summary delta
policy result
human-review reasons
```

Example:

```text
Dependency review: rss-http 0.3.1 -> 0.4.0

PACKAGE RISK
  elevated -> high

PUBLIC CONTRACT
  HttpClient.send
    + effects(retains(request))
    + effects(native)

  Response.body_stream
    new API returns resource BodyStream

NATIVE CHANGES
  Cargo.lock changed
    reqwest 0.12.4 -> 0.12.8

  build_scripts: false -> true via transitive dependency
    source: cargo_metadata_nonexecuting
    policy: review

GRAPH RISK
  high-risk packages: 0 -> 1
  build-time execution packages: 0 -> 1

REVIEW REQUIRED
  - request may now be retained by HttpClient.send
  - new resource BodyStream must be consumed by with
  - build-time execution introduced before native build
```

Provider substitution example:

```text
Provider review: platform-env provider posix-env -> native-env

PUBLIC CONTRACT
  platform-env interface effective hash: unchanged
  semantic contract change: none

PROVIDER / IMPLEMENTATION
  provider package changed: posix-env 0.1.0 -> native-env 0.2.0
  provider risk: low -> high
  reasons:
    + native APIs
    + build-time execution in native Cargo graph

REVIEW REQUIRED
  - provider implementation risk increased while the RSScript-facing interface
    contract stayed unchanged

POLICY
  reject if project policy forbids new high-risk providers, has
  max_high_risk_packages = 0, or denies introduced build-time execution.

  otherwise review_required, because provider risk increased without a public
  contract change.
```

### 13.7 Semantic version check

Publishing should verify that version changes match semantic contract changes.

```text
major bump required:
  breaking public contract change

minor bump allowed:
  new API
  stronger guarantees
  compatible review-visible addition

patch allowed:
  implementation-only change
  documentation/test change
  compatible native bugfix with same .rssi
```

Planned command:

```sh
rss pkg semver-check --since 0.3.1
```

Semver remains a convention. The normalized interface diff is the review source
of truth.

---

## 14. Package Replacement Review

Replacement review compares one package candidate against another by RSScript-facing
contract and risk compatibility.

Planned canonical command once replacement review is implemented:

```sh
rss pkg compare rss-json rss-fast-json
rss pkg compare rss-json rss-fast-json --json
```

It should report:

```text
matching exported roots and symbols
missing required APIs
extra APIs
parameter/return/effect differences
feature differences
resource/retention/native/unsafe/async differences
package risk differences
policy result
compatibility shim opportunities
```

Replacement review does not prove behavioral equivalence. It reports RSScript-facing
contract compatibility and review-risk differences.

Example:

```text
Compatibility: rss-json -> rss-fast-json

compatible:
  Json.parse(text: read String) -> Result<fresh Json.JsonValue, Json.JsonError>

missing:
  Json.pretty_print

different:
  Json.parse
    rss-json:      effects(native)
    rss-fast-json: effects(native, unsafe)

risk:
  rss-json:      elevated
  rss-fast-json: high

result:
  not drop-in compatible under current policy
```

This command is not required for the earliest MVP but follows directly from the
same normalized interface and review metadata used by `rss pkg diff`.

---

## 15. Package Review Metadata

### 15.1 Metadata file

Generated file:

```text
review/package-review.json
```

Implemented v1 schema shape:

```json
{
  "schema": "rss.review.package.v1",
  "package": {
    "name": "rss-json",
    "version": "0.1.0",
    "edition": "2026"
  },
  "risk": "elevated",
  "reasons": ["native Rust wrapper enabled", "returns Result"],
  "badges": ["risk:elevated", "native"],
  "features": [],
  "implements": [],
  "dependencies": [],
  "summary": {
    "interface_files": 1,
    "source_files": 1,
    "dependencies": 0,
    "package_features": 0,
    "public_types": 2,
    "public_sum_types": 0,
    "public_type_aliases": 0,
    "public_consts": 0,
    "public_functions": 2,
    "public_apis": 4,
    "mutating_apis": 0,
    "retaining_apis": 0,
    "resource_apis": 0,
    "fresh_returning_apis": 1,
    "guarantee_apis": 0,
    "native_guarantee_apis": 0,
    "async_apis": 0,
    "await_sites": 0,
    "parallel_apis": 0,
    "native_apis": 2,
    "unsafe_apis": 0,
    "unknown_apis": 0,
    "diagnostics": 0,
    "errors": 0
  },
  "files": [],
  "exports": [
    {
      "name": "Json.parse",
      "kind": "function",
      "function_kind": "sync",
      "classification": "must_review",
      "reasons": ["native_boundary", "returns_fresh", "returns_result"],
      "span": {
        "file": "interface/json.rssi",
        "line": 4,
        "column": 1,
        "length": 9
      },
      "signature": "native fn Json.parse(text: read String) -> Result<fresh JsonValue, JsonError>",
      "normalized_effects": ["native"]
    }
  ],
  "await_sites": [
    {
      "function": "Api.run",
      "callee": "Timer.sleep",
      "boundary": "runtime_pending",
      "live_across_await": ["client"],
      "span": {
        "file": "src/main.rss",
        "line": 4,
        "column": 5,
        "length": 5
      }
    }
  ],
  "native_rust": null,
  "review_map": {
    "schema": "rss.review.v0.6",
    "source": "package",
    "findings": []
  },
  "diagnostics": []
}
```

`badges` is a compact, machine-readable set of review-risk badges derived from
`risk` and the capability `summary` (`risk:<level>`, plus `native`, `unsafe`,
`async`, `parallel`, `unknown-capability`, `has-errors` as applicable). They
restate signals already in the review — never new analysis — so a registry can
render per-package badges without re-deriving them. The registry index entry
(`rss.registry.index.v1`) carries the subset derivable from its own authoritative
fields (`risk:<level>`, `native`, `unsafe`), kept consistent with the entry's
`risk`/`native`/`unsafe_apis`.

`exports` records the normalized public contract surface, not only callable
functions. Current package review metadata uses `kind` values such as
`type`, `sum_type`, `type_alias`, `const`, `function`, `protocol`, and
`protocol_impl` so typed-error, public data-model, protocol declaration, and
explicit protocol implementation changes are visible to metadata and REIR diff
consumers. `protocol` exports include both method names and normalized
effect-carrying method contract strings in `reasons`; a method effect change is
therefore a package contract change, not only a source diff.

For async exports, `function_kind` is `"async"` and `normalized_effects` includes
`"suspends"` even though `suspends` is not a user-authored RSS effect.
Future package metadata schemas may add nested `tool`, `interface`, and richer
native conformance summaries, but those fields are not part of the implemented
`rss.review.package.v1` artifact and must not be required by v0.6 registry or CI
consumers.

The implemented CLI also provides a direct REIR projection:

```sh
rss pkg review --reir <package-directory>
rss pkg reir diff --from <baseline-reir.json> --to <current-reir.json>
reir collect --producer rsscript --package-review review/package-review.json --package-check review/package-check.json --out review/reir/rsscript-ci.json
```

This command emits a `reir.bundle.v0.1` JSON bundle derived from the package
review and embedded language review map. It preserves package risk, native
capability facts, native boundary facts, source `module` / `use` organization
facts, and native crossing edges so REIR tools can consume package-manager
evidence without an extra conversion step. The
stdlib capability projection includes known file/data façades such as
`File.open`, `File.read_all_string`, `File.write_buffer`, `Json.parse_file`, and
`Toml.parse_file`; the general `File.open` constructor is reported as both
`filesystem.read` and `filesystem.write` because its contract does not constrain
the returned handle to one direction. The bundle includes derived review slices
such as `package_risk_slice` and
`native_unsafe_slice` so CI and registry views can start from review-focused
subsets without recomputing them. `rss pkg reir diff` compares two already
generated `reir.bundle.v0.1` artifacts and emits a `reir.diff.v0.1` result,
letting CI compare a locked baseline artifact against the current
`review/reir/rsscript.json` without re-running package review for the baseline.
By default this reports differences without failing; `--fail-on-change` returns
non-zero when any REIR diff item is present.
When package REIR artifacts are merged with other producer bundles, `reir merge`
dedupes stable ids, rebuilds the subject index, and recomputes derived slices so
registry and CI views do not rely on stale per-package slices.
Bundle-mode `reir reconcile <bundle.json> --out <reconciled.json>` then records
required/granted capability reconciliation back into the merged bundle and
recomputes slices for the review UI.
For CI systems that already store package-manager JSON, `reir collect --producer
rsscript` accepts `--package-check`, `--package-lock`, `--lock-update`,
`--package-tree`, `--package-publish`, `--package-metadata`, and
`--package-vendor` in addition to `--review-map` and `--package-review`, then
merges those producer views into one deduped bundle. For `--package-lock`, the
collector preserves the input artifact path as lockfile-entry evidence when the
JSON does not already include a concrete `lockfile_path`.

`rss pkg metadata` writes both review artifacts for CI and registry ingestion:

```text
review/package-review.json
review/reir/rsscript.json
```

`review/package-review.json` and generated REIR files under `review/reir/` are
excluded from package archive hashing, including CI-only artifacts such as
`review/reir/rsscript-check.json` or `review/reir/rsscript-metadata-verify.json`.
Regenerating or adding review artifacts must not change the package content
checksum.
`rss pkg metadata --verify` recomputes both artifacts and compares them with the
files on disk; it reports missing or stale artifacts as metadata mismatches and
exits unsuccessfully so CI can enforce committed review metadata freshness.
Each mismatch records the artifact kind (`package_review` or `reir_bundle`), the
path, mismatch kind (`missing`, `stale`, or `unreadable`), an expected SHA-256
digest, and an actual SHA-256 digest when the stale artifact could be read. This
lets CI and registry tooling compare artifacts without parsing human messages or
inferring artifact type from paths.

### 15.2 Graph summary metadata

A workspace or root package may emit a graph-level review summary:

```json
{
  "schema": "rss.review.graph.v1",
  "root": "my-app",
  "counts": {
    "total_packages": 184,
    "direct_dependencies": 23,
    "transitive_dependencies": 161
  },
  "risk_distribution": {
    "unknown": 2,
    "high": 3,
    "elevated": 18,
    "low": 161
  },
  "highest_risk": [
    {
      "package": "rss-http",
      "version": "0.4.0",
      "risk": "high",
      "path": ["my-app", "rss-http"],
      "reasons": ["native_boundary", "resource_api", "build_time_execution"],
      "evidence": ["normalized_rssi", "cargo_metadata_nonexecuting"]
    }
  ],
  "policy": {
    "status": "fail",
    "errors": ["unknown_packages_present"]
  }
}
```

### 15.3 Metadata generation

Command:

```sh
rss pkg metadata
rss pkg metadata --json
```

Design extensions may add:

```sh
rss pkg review --emit-metadata
```

Inputs:

```text
compiler-normalized .rssi interfaces
.rss source if present
native wrapper declaration
binding manifest
non-executing Cargo metadata if native wrapper exists and is available
review policy
generated artifact provenance
optional source scan/audit inputs
```

If a `.rssi` contract has frontend errors, those diagnostics are reported as
unknown contract exports and counted as unknown APIs, because the public semantic
contract cannot be trusted.

If a package has no `.rssi` surface, local prototype tooling may fall back to
public source declarations for counts and exports, but publishing public packages
requires `.rssi`.

### 15.4 Metadata trust

Registry-provided metadata is useful for search and preview. Consumers should
verify metadata by checking package hashes and optionally regenerating metadata
locally.

Rule:

```text
Metadata is cacheable.
.rssi contract hash is authoritative.
Computed local metadata wins over registry metadata for policy decisions.
```

### 15.5 Machine-readable facts

Human-readable reports are views over structured facts. The implemented package
commands with stable machine-readable output are:

```text
rss pkg check --json
rss pkg metadata --json
rss pkg diff --json
rss review --map --json
rss review --diff --json
```

Planned package-management commands and flags should use the same stable
machine-readable style once implemented and tested:

```text
rss pkg diff --update-plan --json
rss pkg audit-surface --json
rss pkg compare --json
```

Machine-readable formats must distinguish:

```text
authoritative facts
computed local facts
registry preview facts
advisory/cache facts
unknown facts
policy decisions
human-review obligations
```

### 15.6 Metadata-only changes

A review metadata hash may change because:

```text
tool version changed
schema version changed
risk aggregation rules changed
Cargo/native metadata changed
package contents changed
```

Tooling must distinguish these cases when possible. A metadata-only change is
not a public contract delta unless the normalized effective interface changed.

---

## 16. Registry, Publishing, and Security

### 16.1 Registry model

A registry is not required by the language core.

The package model must work with:

```text
local path dependencies
vendored dependencies
private registries
future public registry
future git dependencies after resolver support
```

A registry may provide:

```text
package index
package tarballs
checksum database
.rssi interface preview
review metadata preview
semantic diff history
native risk summary
graph footprint preview
version compatibility data
deprecation/advisory metadata
audit evidence references
```

### 16.2 Registry index entry

```json
{
  "schema": "rss.registry.index.v1",
  "name": "rss-json",
  "version": "0.1.0",
  "checksum": "sha256:...",
  "interface_hash": "sha256:...",
  "effective_interface_hash_default": "sha256:...",
  "review_hash": "sha256:...",
  "review_schema": "rss.review.package.v1",
  "risk": "elevated",
  "native": true,
  "unsafe_apis": false,
  "dependencies": {
    "rss-core": "^0.5"
  },
  "features": {
    "default": [],
    "streaming": []
  },
  "footprint_default": {
    "direct_dependencies": 1,
    "total_packages": 2,
    "path_dependencies": 0,
    "unresolved_dependencies": 0,
    "native": true,
    "native_packages": 1,
    "build_time_execution": false,
    "build_execution_packages": 0,
    "high_risk_packages": 0,
    "unknown_facts": 0
  }
}
```

A registry index entry is a preview and resolution aid. The implemented dry-run
index writes both `interface_hash` and `effective_interface_hash_default` with
the same default effective-interface value. Current REIR adapters prefer
`effective_interface_hash_default` and fall back to `interface_hash` for older
cached preview artifacts. Lockfile verification uses package checksums and
normalized interface hashes.

### 16.3 Review-oriented registry UI

A package page should show:

```text
public API summary
mutating APIs
retaining APIs
closure-capture retention APIs
resource APIs
async APIs
native APIs
unsafe APIs
fresh-returning APIs
semantic changes between versions
risk trend
feature risk matrix
default dependency footprint
Cargo native dependency summary
native conformance level
fact evidence sources: cargo_metadata_nonexecuting, source_scan, declaration, audit, unknown
```

The registry should not present download count or popularity as a substitute for
reviewability.

### 16.4 Publish validation

`rss pkg publish --dry-run` should validate:

```text
manifest valid
interfaces parse
public APIs explicit
effective interface hashes computed
implementation checks
native metadata generated
graph footprint summarized
semantic version check
package review metadata generated
package archive reproducible
unknown package review risk blocks publish readiness unless explicitly allowed
```

In the implemented prototype, `rss pkg publish --dry-run --reir` converts the
publish preview into REIR `supply_chain` facts for archive checksum, registry
checksum, effective-interface hash, review metadata hash, and native wrapper
hash when present. It also emits publish readiness/check facts with
`registry_metadata` evidence and maps the registry preview's `native` and
`unsafe_apis` signals into REIR boundary facts so registries and CI can diff and
slice publish previews without accepting the package. When `--registry` is
provided, REIR registry-index facts carry the planned index path as their
evidence file and archive checksum facts carry the planned archive-manifest path,
while publish readiness and per-check facts carry the target registry directory.
CI can therefore link to dry-run artifacts or their target context without
implying the package was published. If a publish preview input is missing an
expected checksum, effective-interface hash, or review metadata hash, the
corresponding REIR `supply_chain` fact is `unknown`; the adapter must not turn a
missing hash into verified supply-chain evidence.

Planned native ABI validation, once implemented and policy-gated, may add
`rss pkg publish --dry-run --native-abi` to run generated native adapter
type-checking and native build code. The current prototype rejects
`--native-abi` rather than silently executing build-time native code.

Yanking should not break existing lockfile builds, but new resolution should
avoid yanked versions unless explicitly allowed.

### 16.5 Security and supply chain

RSScript packages may contain:

```text
RSScript source
Rust native source
generated source artifacts
build scripts
proc macros
native library links
network/file access during build
```

The package manager must make these visible.

All registry packages should be checksum-verified. `rsspkg.lock` records package
archive checksums and interface hashes.

Sandboxing is future work. MVP should at least surface build-time native
execution risk and avoid executing it during review-only commands.

### 16.6 Build-time execution policy

Public registries may choose stricter defaults than local development.

Recommended public registry publish defaults:

```text
build_scripts: review or forbid
proc_macros: review or forbid
native_links: review
ffi: review
rss_unsafe_apis: forbid unless package is explicitly unsafe-capability package
```

A package with build-time execution risk may still be publishable, but the risk
must be displayed and included in semantic dependency diff.

---

## 17. CLI Design

Canonical package-management commands live under `rss pkg`, with `--json` for
package-manager JSON and `--reir` for REIR bundle or diff JSON where listed. The
subcommands wired into the CLI **today** are `rss pkg [dir]` (the default check),
`rss pkg review`, `rss pkg diff`, `rss pkg ci`, `rss pkg publish --dry-run`,
`rss pkg lock`, `rss pkg tree`, `rss pkg metadata`, and `rss pkg vendor`. The two
remaining forms below — `rss pkg review update` and `rss pkg reir diff` — are part
of the canonical design and backed by library functions, but are not yet exposed
as their own CLI subcommands. The full canonical surface:

```sh
rss pkg check    [--json|--reir] [package-directory]
rss pkg review   [--json|--reir] [package-directory]
rss pkg review update [--json|--reir] --from <old-rsspkg.lock> --to <new-rsspkg.lock>
rss pkg lock     [--json|--reir] <package-directory>
rss pkg tree     [--json|--reir] [package-directory]
rss pkg publish  --dry-run [--json|--reir] [--registry <directory>] [package-directory]
rss pkg vendor   [--dry-run] [--json|--reir] [package-directory]
rss pkg metadata [--dry-run|--verify] [--json|--reir] [package-directory]
rss pkg diff     [--json|--reir] <old-package-directory> <new-package-directory>
rss pkg reir diff [--json] [--fail-on-change] --from <baseline-reir.json> --to <current-reir.json>
```

No `rss package ...` command is defined for v0.6 tooling. No `rss review deps`
alias is normative in v0.6. New dependency-review workflows should stay under
`rss pkg diff`, `rss pkg review`, `rss pkg review update`, or future tested
`rss pkg` subcommands; they should not introduce parallel command namespaces.

Design-target commands such as `rss pkg audit-surface`, `rss pkg semver-check`,
`rss pkg compare`, and `rss pkg check --native-abi` remain planned until their
underlying graph-risk, semver, replacement-review, or adapter-check facts are
implemented and tested.

### 17.1 `rss pkg check`

Runs manifest, interface, source, lockfile, graph summary, and non-executing
native checks.

```sh
rss pkg check [--json|--reir] [package-directory]
```

In the implemented prototype, `--reir` converts the package check result into a
REIR bundle for CI gates. It emits the overall check status, graph/lock/native
policy results, stale lock package-change facts and their changed lock fields,
native unsafe/build-time facts, provider implementation declarations from
`[implements]`, and diagnostics. This makes `rss pkg check` failures mergeable
with package review, tree, lock, metadata, vendor, and publish evidence. Overall
status and graph policy facts use the package directory as their evidence file.
The lock policy fact uses `lockfile_entry` evidence at the reported semantic lock
path. The native policy fact and native unsafe/build-time facts use the
`native_rust.path` directory as their evidence file, keeping the review boundary
directly navigable in CI output. Provider implementation facts from
`[implements]` use `rsspkg.toml` as their evidence file.
Diagnostic facts keep their source-span line and column; relative diagnostic
paths are resolved under the checked package directory so CI output links to the
actual manifest, source, interface, native metadata, or policy file.

Planned policy/native-ABI extensions, once implemented and tested:

```sh
rss pkg check --deny-unknown
rss pkg check --deny-high-risk
rss pkg check --native-abi
```

### 17.2 `rss pkg tree`

Shows dependency graph with risk. In the implemented prototype, `--reir`
converts the resolved tree into dependency-risk facts, effective-interface hash
`supply_chain` facts, and `depends_on` edges with `dependency_path` evidence, so
CI and registry tooling can merge graph facts with package-review and lockfile
facts. The REIR evidence source is `rsscript_tree`, keeping graph observations
separate from package-review output. For resolved `path+` dependencies, the
evidence file is the resolved package directory; unresolved registry/git
dependencies and missing path dependencies remain graph observations with empty
`evidence.file`, not a synthetic local artifact path:

```text
my-app
├── rss-core 0.5.0 [low]
├── rss-json 0.1.0 [elevated, native]
└── rss-http 0.4.0 [high, native, build.rs, resource]
```

### 17.3 `rss pkg audit-surface` (planned)

Summarizes the current graph as a review surface. Until this subcommand is implemented, the same facts should be surfaced through `rss pkg review`, `rss pkg tree`, `rss pkg check`, and metadata output where available.

```sh
rss pkg audit-surface
rss pkg audit-surface --json
```

It reports risk distribution, highest-risk packages, dependency paths, reasons,
evidence, and policy result. It must not execute native build code by default.

### 17.4 `rss pkg review`

Generates package-level review report for the current package or workspace.

```sh
rss pkg review [--json|--reir] [package-directory]
```

In the implemented prototype, `--reir` converts the package review report into a
REIR bundle with package risk, public contract, protocol, dependency, native
boundary, capability, diagnostic, and async-boundary facts. This is the primary
package-local producer for `reir show`, `reir merge`, `reir slice`, and
bundle-mode `reir reconcile`.

Design extensions may add:

```sh
rss pkg review --emit-metadata
rss pkg review --all-features
```

It must not execute native build code by default.

### 17.5 `rss pkg metadata`

Emits machine-readable metadata.

```sh
rss pkg metadata [--dry-run|--verify] [--json|--reir] [package-directory]
```

`--verify` recomputes metadata locally and compares against committed
`review/package-review.json` and `review/reir/rsscript.json` artifacts. Registry
metadata verification remains a design extension layered on top of the same
local artifact comparison.

`--reir` converts the metadata command result itself into a REIR bundle. The
bundle records metadata status, the package-review and REIR artifact paths as
artifact `supply_chain` facts, and any stale/missing/unreadable artifact
mismatches as `policy_result` facts with `package_metadata` evidence. The
top-level metadata status fact uses the package directory as its evidence file
and `/ok` as its JSON pointer. It is important that mismatch `evidence.file`
remains the artifact path while expected/actual SHA-256 details live in
`evidence.value`, `evidence.reason`, and `unknown_reason`. This is intended for
CI artifact freshness gates and complements, rather than replaces, the
package-review REIR bundle written to `review/reir/rsscript.json`.

### 17.6 `rss pkg diff`

Compares package versions, lockfiles, or update plans.

```sh
rss pkg diff [--json|--reir] <old-package-directory> <new-package-directory>
rss pkg reir diff [--json] [--fail-on-change] --from <baseline-reir.json> --to <current-reir.json>
rss pkg review update [--json|--reir] --from <old-rsspkg.lock> --to <new-rsspkg.lock>
```

`--reir` emits a `reir.diff.v0.1` JSON diff over the REIR bundles derived from
each package review. This is the package-manager convenience path for CI jobs
that want REIR-native review diffs without separately invoking `reir diff`.
`rss pkg reir diff` uses already-written REIR bundles instead of package
directories, which is the baseline-artifact path for registries and CI caches.
`--fail-on-change` makes the artifact diff suitable as a CI gate while leaving
plain diff usable for local inspection.
`rss pkg review update --reir` emits a REIR bundle from the semantic lock update
itself: update-risk facts, package-risk facts, and changed-field facts with
`lockfile_entry` evidence. This complements `rss pkg lock --reir`, which emits
the accepted lock state rather than the update decision. The top-level
update-risk fact uses the `/risk` JSON pointer. Evidence for added or changed
lock entries points at the new lockfile, while evidence for removed packages or
removed fields points at the old lockfile.

Design extensions may add:

```sh
rss pkg diff rss-http@0.3.1 rss-http@0.4.0
rss pkg diff --lockfile old/rsspkg.lock --new-lockfile rsspkg.lock
rss pkg diff --update-plan
rss pkg diff --update-plan --json
```

### 17.7 `rss pkg lock`

Updates or checks `rsspkg.lock`. In the implemented prototype, `--reir`
converts the generated semantic lock into a REIR bundle with lockfile-backed
`supply_chain` facts for package checksum, effective interface hash, review
metadata hash, and native wrapper hash when present. The REIR evidence file is
the generated semantic lock path (`<package-directory>/rsspkg.lock`) so CI and
registry tools can link each hash fact to the lockfile that would be written or
checked. Missing checksum, effective-interface hash, review hash, or native hash
values become `unknown` REIR facts rather than verified supply-chain evidence.

```sh
rss pkg lock [--json|--reir] <package-directory>
```

Design extensions may add:

```sh
rss pkg lock --check
```

### 17.8 `rss pkg vendor`

Vendors dependencies locally for offline/reproducible builds.

```sh
rss pkg vendor [--dry-run] [--json|--reir] [package-directory]
```

For the prototype, local path dependencies can be copied into:

```text
vendor/<name>-<version>/
vendor/rss-vendor.json
```

Registry support depends on resolver availability. Git dependencies are unsupported in v0.6 and must be rejected with a stable
unsupported dependency-source diagnostic if encountered.

In the implemented prototype, `--reir` converts the vendor report into a REIR
bundle. Vendored local dependencies become checksum `supply_chain` facts, and
registry/git/unresolved dependencies remain unknown `dependency_risk` facts with
`package_metadata` evidence. The top-level vendor status fact points at the
vendor directory with `/ok` evidence. Checksum facts for vendored entries point
their evidence file at the specific `vendor/<name>-<version>/` path from the
vendor report, while unresolved dependency facts point at the vendor directory.
This lets offline-build preparation participate in the same package-risk and
supply-chain review slices as lock, tree, metadata, and publish evidence.

### 17.9 Future commands

Future package-management commands may extend the same `rss pkg` namespace, but
they are not part of the current executable surface until implemented and tested:

```sh
rss pkg init
rss pkg add <package>
rss pkg add <package> --apply
rss pkg remove <package>
rss pkg update [package]
rss pkg audit-surface
rss pkg semver-check --since <version>
rss pkg compare <old-package> <new-package>
rss pkg explain <package>
rss pkg why <package>
rss pkg clean
```

---

## 18. Review Policies and Budgets

A project may define dependency review policy.

```toml
[review.policy]
deny_unknown = true
deny_unsafe_apis = true
max_high_risk_packages = 0
max_native_dependencies = 5
require_lockfile = true
require_review_metadata = true
native_api_risk = "high"
build_execution_default = "forbid"

[dependency.budget]
max_direct_dependencies = 40
max_total_packages = 300
max_new_transitive_per_add = 25  # advisory transition budget
max_native_packages = 5
max_high_risk_packages = 0
max_unknown_packages = 0
max_build_execution_packages = 0

[native.rust.policy]
build_scripts = "forbid"
proc_macros = "forbid"
native_links = "review"
ffi = "review"
rss_unsafe_apis = "forbid"
wrapper_unsafe_blocks = "review"
transitive_unsafe_blocks = "review"
```

Policy checks should fail CI if violated:

```text
error: package rss-crypto exports effects(unsafe), but deny_unsafe_apis=true
error: package rss-http is high risk, max_high_risk_packages=0
warning: package rss-image uses build.rs; policy requires review
error: package rss-json has unknown native wrapper unsafe status, but deny_unknown=true
error: dependency graph has 2 unknown packages, max_unknown_packages=0
```

Budget dimensions:

```text
number of direct dependencies
number of total packages
number of newly added transitive packages per add/update
number of high-risk dependencies
number of native dependencies
number of retaining APIs imported
number of closure-capture-retaining APIs imported
number of resource APIs imported
number of async APIs imported
number of unsafe APIs imported
number of unknown APIs
number of dependencies with build-time execution risk
number of native dependencies lacking adapter type-checking
number of dependencies with unknown native facts required by policy
```

Budgets are policy choices, not language semantics. Tools must report raw facts
so projects can choose their own thresholds.

---

## 19. Diagnostics

Package manager diagnostics should use stable codes eventually.

Diagnostic classes:

```text
manifest error
dependency resolution error
feature resolution error
interface normalization error
interface conflict
feature-conditioned interface conflict
semantic version mismatch
native wrapper missing binding
native binding target mismatch
native adapter type-check failure
native risk policy violation
graph risk policy violation
lockfile mismatch
registry checksum mismatch
review metadata mismatch
provider resolution failure for interface-only packages
Cargo metadata failure
Cargo integration failure
unmappable backend diagnostic
```

Recommended code ranges:

```text
PKG00xx  manifest and package layout
PKG01xx  dependency resolution and feature resolution
PKG02xx  interface loading, normalization, hashing
PKG03xx  lockfile/checksum/vendor
PKG04xx  semantic diff, update review, semver
PKG05xx  review metadata and risk policy
PKG06xx  native bindings and native conformance
PKG07xx  Cargo integration
PKG08xx  registry/publish
PKG09xx  provider/interface-only package resolution
PKG10xx  graph risk summary and dependency budgets
```

Boundary with RSScript diagnostics:

```text
RSxxxx diagnostics are compiler/frontend diagnostics over RSScript source and
.rssi semantic contracts.

PKGxxxx diagnostics are package-manager diagnostics over manifests, dependency
resolution, feature resolution, lockfiles, registries, native binding metadata,
Cargo integration, graph summaries, and package policy.

When package tooling invokes the compiler frontend and receives an RS diagnostic,
it surfaces that RS diagnostic rather than translating it into PKG.
```

Example:

```text
error[PKG0401]: dependency update adds retaining API
  package: rss-cache 0.2.0 -> 0.3.0
  function: Cache.put
  change: +effects(retains(value))

This update requires review because values passed to Cache.put may now be retained.
```

Graph policy example:

```text
error[PKG1001]: dependency graph contains unknown-risk packages
  unknown packages: 2
  policy maximum: 0

Run `rss pkg review --json` or `rss pkg tree --json` to see which packages are
unknown and why. A dedicated `rss pkg audit-surface` command is a design target.
```

Native wrapper compile errors may not map to RSScript source. Diagnostics should
identify the package/native wrapper boundary clearly:

```text
error[PKG0702]: native wrapper `rss-json` failed to compile
  package: rss-json 0.1.0
  native crate: native/rust
  command: cargo check for generated adapter

This is a native implementation error, not an RSScript source error.
```

---

## 20. Examples

### 20.1 Wrapping `serde_json`

RSScript interface:

```rust
// interface/json.rssi

features: native

opaque struct Json.JsonValue
opaque struct Json.JsonError

native fn Json.parse(
    text: read String,
) -> Result<fresh Json.JsonValue, Json.JsonError>
    effects(native)

native fn Json.field_string(
    value: read Json.JsonValue,
    name: read String,
) -> Result<String, Json.JsonError>
    effects(native)
```

Rust wrapper manifest:

```toml
# native/rust/Cargo.toml

[package]
name = "rss_json_native"
version = "0.1.0"
edition = "2024"

[dependencies]
serde_json = "1"
```

The `edition = "2026"` in `rsspkg.toml` is the RSScript package/language edition.
The `edition = "2024"` in `native/rust/Cargo.toml` is the Rust crate edition.
They are intentionally separate.

Binding manifest:

```toml
# native/bindings.rssbind.toml

[bindings]
"Json.parse" = "rss_json_native::json_parse"
"Json.field_string" = "rss_json_native::json_field_string"

[types]
"Json.JsonValue" = "rss_json_native::JsonValue"
"Json.JsonError" = "rss_json_native::JsonError"
```

Rust wrapper implementation:

```rust
pub struct JsonValue {
    inner: serde_json::Value,
}

#[derive(Debug)]
pub struct JsonError {
    message: String,
}

pub fn json_parse(text: &str) -> Result<JsonValue, JsonError> {
    serde_json::from_str(text)
        .map(|inner| JsonValue { inner })
        .map_err(|error| JsonError {
            message: error.to_string(),
        })
}

pub fn json_field_string(value: &JsonValue, name: &str) -> Result<String, JsonError> {
    value
        .inner
        .get(name)
        .and_then(|field| field.as_str())
        .map(str::to_string)
        .ok_or_else(|| JsonError {
            message: format!("missing or non-string JSON field `{name}`"),
        })
}
```

RSScript usage:

```rust
let value = Json.parse(text: read body)?
let name = Json.field_string(value: read value, name: read "name")?
```

Review metadata summary:

```text
risk: elevated
reason: native_boundary
native APIs: 2
unsafe RSScript APIs: 0
build.rs: false, source=cargo_metadata_nonexecuting
proc macros: false, source=cargo_metadata_nonexecuting
wrapper unsafe blocks: false, source=source_scan_best_effort
adapter typechecked: not_run unless rss pkg check --native-abi is used
```

### 20.2 HTTP wrapper risk

```rust
features: native

struct HttpResponse
struct HttpError

pub native fn Http.get(
    url: read Url,
) -> Result<fresh HttpResponse, HttpError>
    effects(native)

pub native fn Http.post_json(
    url: read Url,
    body: read String,
) -> Result<fresh HttpResponse, HttpError>
    effects(native)

pub native fn HttpResponse.text(
    response: read HttpResponse,
) -> fresh String
    effects(native)
```

Review metadata:

```text
risk: high under strict native policy, elevated under bounded-native policy
native APIs: 2
blocking/network APIs: 2, source=author_declaration or audit
resource APIs: 0
retaining APIs: 0
```

Registry summary:

```text
rss-http 0.4.0
  risk: high
  native: yes
  unsafe RSScript APIs: no
  build.rs: yes via dependency graph, source=cargo_metadata_nonexecuting
  public APIs: 8
  network APIs: 3, source=author_declaration
```

### 20.3 Update review

```text
rss pkg diff --update-plan

Update review

rss-json 0.2.1 -> 0.2.2
  public contract: unchanged
  effective interface hash: unchanged
  risk: elevated unchanged
  native implementation: changed
  review: native implementation update only

rss-http 0.3.0 -> 0.4.0
  public contract: changed
  risk: elevated -> high

  changed APIs:
    HttpClient.send
      + effects(retains(request))

  added APIs:
    Response.body_stream() -> BodyStream
      returns resource

  native/build changes:
    build_scripts: false -> true
    native_links: false -> true

  graph risk:
    high-risk packages: 0 -> 1
    build-time execution packages: 0 -> 1

  review required:
    - request may now be retained
    - new resource return must be consumed by with
    - native build-time execution introduced
```

### 20.4 Interface-only provider

Interface package:

```rust
// interface/env.rssi

features: native

pub native fn Env.get(name: read String) -> Option<fresh String>
    effects(native)

pub native fn Env.get_or_default(
    name: read String,
    default: read String,
) -> fresh String
    effects(native)
```

Manifest:

```toml
[package]
name = "platform-env"
version = "0.1.0"
edition = "2026"
kind = "interface-only"

[interfaces]
paths = ["interface"]
exports = ["Env"]
```

Executable builds must either mark the interface dependency as platform-provided
in the consumer manifest:

```toml
[dependencies]
platform-env = { path = "../platform-env", platform_provided = true }
```

or resolve a package provider with a reviewed implementation declaration:

```toml
[dependencies]
platform-env = { path = "../platform-env" }
posix-env = { path = "../posix-env" }

[providers]
platform-env = { package = "posix-env", version = "0.1.0" }
```

Provider packages bind themselves to the reviewed interface contract with
`[implements."<interface-package>"]` and an `interface_effective_hash`.

---

## 21. Workspace Model

Workspace root:

```text
repo/
  rsspkg.toml
  packages/
    rss-json/
    rss-http/
  apps/
    image-service/
```

Root manifest:

```toml
[workspace]
members = [
  "packages/rss-json",
  "packages/rss-http",
  "apps/image-service",
]
```

A workspace should share one `rsspkg.lock` by default. Native Cargo builds may
share one generated Cargo workspace.

Development package-source overrides are not part of v0.6. Local path
dependencies cover the initial development workflow; provider selection still
stays root-scoped for the executable graph.

Workspace-level review policy may override package-local default policy for CI.
Package-local policy is still useful for publish readiness and expected risk.

### 21.1 Self-hosted package-review module boundary

As the RSScript package manager moves from Rust implementation support toward a
self-hosted RSScript implementation, package-review code should use explicit
module and use declarations rather than an unstructured single-file namespace:

```rust
module rss.package.review

use rss.package.contract.PackageContract
use rss.review.ReviewMap
```

`rss.package.review` owns package-level review aggregation: it consumes
compiler-owned package contracts and language review maps, then produces package
risk summaries and metadata. It must not reimplement language semantics, infer
effects from Rust code, or treat `use` as implicit method lookup. The imported
`PackageContract` and `ReviewMap` are semantic inputs, not trait-style extension
points.

---

## 22. MVP Plan

```text
MVP 0: Local package format and review-without-execution
  rsspkg.toml
  local path dependencies
  interface path loading
  safe default rss pkg check
  rss pkg metadata without native build execution

MVP 1: Compiler-owned interface normalization
  compiler frontend parses .rssi
  normalize .rssi and compute interface content hash
  selected-feature effective interface hash
  detect duplicate exported symbols

MVP 2: Interface dependency graph and lockfile
  resolve local dependency interfaces
  check source against dependency .rssi
  rsspkg.lock authoritative fields
  advisory review metadata fields separated from semantic hashes

MVP 3: Package review metadata and risk policy
  review/package-review.json
  review/reir/rsscript.json
  computed package risk summary
  API classification
  native risk summary with evidence sources
  CI policy checks

MVP 4: Graph risk summary
  rss pkg audit-surface
  risk distribution
  highest-risk package list
  dependency paths
  evidence-linked reasons
  unknown/high/elevated/low graph summary

MVP 5: Cargo native wrapper metadata integration
  native/rust package discovery
  binding manifest checks
  Non-executing Cargo metadata scan
  build.rs/proc-macro/native-link detection where available

MVP 6: Optional native ABI adapter check
  generated bridge adapters
  cargo check under --native-abi
  native conformance level reporting
  source-map-aware native boundary diagnostics

MVP 7: Semantic dependency diff and update review
  rss pkg diff
  rss pkg diff --update-plan
  compare old/new effective .rssi
  classify semantic changes
  report native source/binding/Cargo.lock changes
  report graph risk delta
  produce human and JSON reports

MVP 8: Registry protocol
  package archive format
  index format
  checksums
  publish dry-run
  review metadata preview
  local/private registry support

MVP 9: Public registry and expanded governance
  package search
  package page with review metadata
  semantic diff between versions
  feature risk and footprint matrix
  advisories
  yanking
  signing policy
```

---

## 23. Open Questions

The following questions remain open for post-MVP design or implementation policy:

```text
1. Should package imports be explicit in RSScript source, or manifest-driven initially?
2. Should the package manager allow multiple major versions of the same package in one graph?
3. How strict should native ABI adapter checking be by default for public registry publishing?
4. Should package features remain purely additive, or should a future explicit
   mutually-exclusive feature group exist?
5. How much Cargo metadata should be surfaced in default review output?
6. Should build scripts be denied by default for public registry packages?
7. Should registry metadata be signed independently from package archives?
8. Should review metadata be mandatory for publishing, or only for public registry indexing?
9. How should async runtime dependencies be represented without leaking Rust runtime details?
10. Should future retained ResourcePool factory signatures use a first-class
    retained-closure syntax or remain expressed through effects(retains(create))?
11. What audit evidence format is sufficient for native conformance level 4?
12. Should source_scan_best_effort be standardized, or should it remain an
    implementation-specific metadata source?
13. What graph-budget defaults, if any, should a public registry recommend?
14. Should capability metadata be declared by package authors, inferred from
    exported public roots, curated by registries, or a combination?
15. Should a future provider declaration support contract-compatible interface
    ranges rather than an exact `interface_effective_hash`, and how would a
    registry validate freshness without local rechecking?
16. Should a post-v0.6 package-source patch mechanism exist for development
    workflows where local path dependencies are insufficient, and how can it
    remain deterministic and review-visible without introducing consumer-local
    provider overrides?
17. Should registry metadata optionally cache a non-authoritative transitive
    exposed-interface closure for search and UX, while the normative effective
    interface hash keeps only directly referenced public dependency identities?
    Any such registry cache must not be folded into authoritative hashes,
    lockfile contract identity, or package acceptance decisions.
```

Not open in v0.6:

```text
- .rssi normalization belongs to the compiler frontend, not an independent
  package-manager normalizer.
- package risk is computed, not author-declared.
- review-only commands must not execute native build code by default.
- package metadata must use low_semantic_risk and must not emit legacy category
  names such as the old skip-safety label.
- a resolved dependency graph is not automatically accepted as reviewable.
- `public_dependency_interfaces` contains only directly referenced dependency
  interfaces; deeper exposed contracts are covered transitively by those
  dependencies' effective interface hashes.
- provider implementation risk is not part of an interface effective hash;
  provider substitution is reported as graph-risk / implementation delta, not as
  a public contract delta.
- v0.6 defines no package override, patch, or feature-pinning mechanism.
```

---

## 24. Summary

The RSScript package manager should be designed around two sentences:

```text
Cargo builds the implementation; RSScript packages publish reviewable semantic contracts.
A dependency graph is not just an installation artifact; it is a review artifact.
```

The distinctive value of RSScript package management is that package dependencies
become semantically reviewable:

```text
.rssi defines public contract
effective interface hash captures selected-feature public semantics
rsspkg.lock locks semantic dependency graph
Cargo.lock locks Rust implementation graph
review metadata summarizes package risk
rss pkg review/tree summarize graph risk; audit-surface is a design target
semantic diff explains dependency upgrades
native wrappers expose Rust crates through reviewable APIs
native facts report their evidence source
unknown risk is classified as unknown, not safe
review can happen before build-time native code executes
machine-readable facts support CI, registries, IDEs, and AI repair agents
```

This gives RSScript a package ecosystem aligned with its language philosophy:

```text
Script-like source.
System-level boundaries.
Reviewable dependencies.
Rust-powered implementation.
```
