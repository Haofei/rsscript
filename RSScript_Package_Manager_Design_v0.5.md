# RSScript Package Manager Design — Revised Draft

Status: Draft / ecosystem design consolidation candidate  
Version: 0.5-sync-revised  
Based on: Package Manager Design v0.3-editorial and RSScript v0.5 language model  
Audience: RSScript compiler implementers, package authors, native binding authors, registry implementers, review-tool authors  
Scope: package model, dependency resolution, Cargo integration, semantic package review, native wrappers, registry protocol direction  
Non-scope: RSScript core language semantics, full registry product design, centralized service policy, sandbox implementation design

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

### 0.1 Normative package-management principles

Package tooling follows these package-specific principles:

```text
1. Cargo builds the Rust implementation; RSScript packages publish reviewable
   semantic contracts.
2. The public `.rssi` contract is authoritative for RSScript-facing semantics.
3. Review metadata is computed, not trusted because a package author wrote it.
4. Native implementation facts must distinguish machine-checked facts,
   declared facts, best-effort scanned facts, and audited facts.
5. Review commands must not execute untrusted native build code by default.
6. Feature selection produces an effective interface; hashes and diffs are over
   the effective interface for the selected feature set.
7. The RSScript compiler frontend owns `.rssi` parsing and semantic
   normalization; the package manager must not implement an independent
   semantic normalizer.
```

### 0.2 Terminology

```text
interface content hash
    Hash of the compiler-normalized effective `.rssi` content. It excludes
    formatting, comments, private implementation files, tests, and review
    metadata.

effective interface
    The public `.rssi` surface after selected package features are applied.

effective interface hash
    Hash of the selected feature set plus the interface content hash. Used by
    lockfiles and registries to bind a dependency to a particular feature-shaped
    semantic surface.

package risk
    A package-level supply-chain/review tier computed from exports,
    implementation facts, native/build facts, and policy. It is not the same
    object as the language review-map classification of a function or file.

native conformance
    The degree to which a Rust native wrapper has been checked against its
    `.rssi` contract. Binding existence, adapter type-checking, semantic trust,
    and audit status are separate levels.
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
  generated Cargo workspace glue
  source-map-aware diagnostic integration
```

A dependency update should answer:

```text
What public contracts changed?
Which APIs now mutate?
Which APIs now retain values?
Which APIs now retain closure captures?
Which APIs now return or manage resources?
Which APIs now cross native or unsafe boundaries?
Which package features introduced native, unsafe, build-script, proc-macro,
  FFI, or native-link boundaries?
Which native facts are checked, declared, scanned, audited, or unknown?
Which dependency changes require human review?
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
  unsafe code, or build-time execution?
Can the reviewer inspect the public contract without reading Rust implementation?
Can the reviewer tell which native facts are actually machine-checked?
```

Therefore package management is part of the RSScript review story.

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
generate machine-readable package review metadata
preserve source-map-aware diagnostics through generated Rust packages
make native/build/supply-chain risks visible before executing build code
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
language-level import syntax finalization
automatic inference of RSScript effects from arbitrary Rust code
whole-program proof of Rust native wrapper semantic behavior
automatic proof that Rust code does not block, allocate, panic, spawn, or retain
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
```

A package may be:

```text
library package
binary/tool package
interface-only package
native wrapper package
workspace member package
```

`core` and `trusted` are not package kinds. They are registry, distribution, or
project-policy trust tiers attached to a package version after review. Authors
must not be able to self-declare a package as trusted merely by choosing a
manifest kind.

### 4.2 Semantic contract

The semantic contract of a package is its public `.rssi` surface after package
features are applied.

The `.rssi` contract declares:

```text
public namespaces and exported roots
public types
public functions
parameter names
parameter types
read / mut / take effects
return types
fresh returns
retention effects
resource APIs
native / unsafe effects
review-relevant guarantees
```

The package manager treats the `.rssi` surface as the primary artifact for
review, compatibility, and dependency diff.

### 4.3 Effective interface

A package feature set produces an effective interface.

```text
effective interface = normalized public .rssi contract under selected features
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
   not hide a change to read/mut/take, retains, native, unsafe, resource, fresh,
   or guarantee semantics.
5. Any feature-conditioned change to a public contract is visible through the
   effective interface hash and semantic diff.
```

### 4.4 Implementation artifact

A package implementation may be written in:

```text
RSScript source
Rust native wrapper code
both RSScript and Rust
```

Implementation artifacts must conform to the public `.rssi` contract. For pure
RSScript implementation, conformance is checked by the RSScript compiler. For
native Rust implementation, conformance is split into explicit levels described
in Chapter 9.

### 4.5 Native wrapper

A native wrapper is Rust code that adapts one or more Rust crates to an RSScript
`.rssi` contract.

```text
serde_json crate
  -> rss-json native wrapper
  -> json.rssi
  -> RSScript Json.parse API
```

The raw Rust crate API is not automatically exposed to RSScript. Only `.rssi`
APIs are visible.

### 4.6 Review metadata

Review metadata is generated from:

```text
compiler-normalized .rssi
RSScript source, if present
native package declarations
Cargo metadata, if native.rust is enabled
native binding manifests
policy configuration
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
semantic diff against previous version
native facts and their evidence source
native conformance level
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

Core packages still expose `.rssi` contracts. Being core does not remove the need
for semantic interfaces.

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
```

### 5.3 Generated build directory

The compiler/package manager may generate:

```text
target/rss/
  generated/
    Cargo.toml
    src/lib.rs
    src/main.rs
    rsscript-source-map.json

  workspace/
    Cargo.toml
    packages/
    native/
```

Generated Rust is an internal build artifact. It is not the RSScript package
contract.

### 5.4 Interface-only packages and providers

An interface-only package provides contracts but no implementation.

Rules:

```text
1. Interface-only packages may be used for type checking, semantic review, and
   mock/test contracts.
2. Executable builds require an implementation provider unless the dependency is
   explicitly marked compile_only, test_only, or platform_provided.
3. rsspkg.lock records provider resolution for executable builds.
4. A dependency on an interface-only package without an implementation provider
   is a diagnostic for executable build commands such as `rss run` and
   `rss verify-rust`. Pure frontend checks may use interface-only packages for
   type checking and review.
5. Interface-only packages may be published only if their manifest declares the
   provider expectation.
```

Example:

```toml
[package]
name = "platform-env"
version = "0.1.0"
edition = "2026"
kind = "interface-only"

[interfaces]
paths = ["interface"]
exports = ["Env"]

[provider]
mode = "platform_provided"   # platform_provided | compile_only | test_only | package
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

[interfaces]
paths = ["interface"]
exports = ["Json", "JsonValue", "JsonError"]

[sources]
paths = ["src"]

[dependencies]
rss-core = "0.5"

[dev-dependencies]
rss-test = "0.5"

[features]
default = []
streaming = []

[interfaces.features.streaming]
paths = ["interface/streaming"]
exports = ["JsonStream"]

[review.policy]
deny_unknown = false
deny_native = false
deny_unsafe_apis = true
max_public_params = 8
max_nested_type_depth = 4
native_api_risk = "elevated"       # elevated | high
build_execution_default = "forbid" # forbid | review | allow

[review.expect]
risk = "elevated"                  # optional expectation, never authoritative

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
cargo_features = []

[native.rust.feature_map]
streaming = ["streaming"]

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

Package names should be stable, lowercase, and registry-unique.

Optional package kind:

```toml
kind = "library"        # library | binary | interface-only | native-wrapper | tool
```

If omitted, the package kind is inferred from layout for local tooling, but
published packages should declare it explicitly.

### 6.3 `[interfaces]`

Declares where public `.rssi` files live.

```toml
[interfaces]
paths = ["interface"]
exports = ["Json", "JsonValue", "JsonError"]

[interfaces.features.streaming]
paths = ["interface/streaming"]
exports = ["JsonStream"]
```

`[interfaces]` describes the base public interface. Each
`[interfaces.features.<feature>]` table adds public interface roots only when
that package feature is selected. This is the MVP feature-gating mechanism for
package public contracts; a future compiler-defined conditional `.rssi` syntax
may be added only if it preserves a single normalized effective interface.

The package manager selects the relevant interface paths, then asks the compiler
frontend to parse and normalize them. The compiler frontend, not the package
manager, owns interface syntax and semantic normalization.

### 6.4 `[sources]`

Declares RSScript implementation source roots.

```toml
[sources]
paths = ["src"]
```

A native wrapper package may omit `[sources]` if all implementation is native
Rust behind `.rssi` contracts.

An interface-only package omits `[sources]` unless it contains test/mocking
source files outside the published interface.

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

Reserved future dependency source form:

```toml
my-git = { git = "https://example.org/my-git", rev = "abc123" }
```

The `git` form is syntax-reserved until a resolver exists. A prototype may parse
and reject it with a stable diagnostic rather than attempting partial support.

### 6.6 `[features]`

Package features select optional RSScript package APIs or implementation paths.

```toml
[features]
default = []
streaming = []
native-tls = []
```

Package features are not the same as RSScript file features such as
`features: local`.

Rules:

```text
1. Package features resolve deterministically.
2. Cargo-like additive feature unification is the default unless a package marks
   a feature as mutually exclusive through a future explicit mechanism.
3. A package feature must not silently introduce async, native, or unsafe
   boundaries.
4. If a package feature enables async APIs, native APIs, unsafe APIs, build
   scripts, proc macros, linked libraries, FFI, or additional resource/retention
   APIs, review metadata must report it.
5. A feature-conditioned public contract is expressed by selected interface
   paths such as `[interfaces.features.<feature>]` or a future compiler-owned
   conditional interface syntax, and it produces a different effective interface
   hash.
6. Package feature names are allowed to map to Cargo feature names for native
   wrapper builds, but Cargo feature selection does not define RSScript public
   semantics by itself.
```

### 6.7 `[review.policy]`

Package-level declared review policy.

```toml
[review.policy]
deny_unknown = true
deny_native = true
deny_unsafe_apis = true
max_public_params = 8
max_nested_type_depth = 4
native_api_risk = "high"          # elevated | high
build_execution_default = "forbid" # forbid | review | allow
```

The canonical policy style is `deny_*`. `allow_*` aliases are not normative in
v0.5 because mixed allow/deny spelling makes policy precedence ambiguous. A
prototype may accept legacy aliases with warnings, but published package policy
should use the canonical keys above.

Canonical policy keys:

```text
deny_unknown       fail when computed package risk is unknown or required facts are unknown
deny_native        fail when selected public APIs or implementation facts require native boundaries
deny_unsafe_apis   fail when selected public APIs expose effects(unsafe)
native_api_risk    when native is not denied, map public native APIs to elevated or high
```

This section is a policy, not a self-declared risk result. Computed metadata
wins over author expectations.

### 6.8 `[review.expect]`

Optional author expectation.

```toml
[review.expect]
risk = "low"        # low | elevated | high | unknown
```

This is not authoritative. Tooling may use it to detect mismatches:

```text
declared expected risk: low
computed risk: elevated
result: warning or policy failure depending on project settings
```

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

[native.rust.feature_map]
streaming = ["streaming"]
native-tls = ["native-tls"]
```

`cargo_features` are always enabled for this wrapper. The optional
`[native.rust.feature_map]` table maps selected RSScript package features to
Cargo features. This mapping affects the Rust implementation graph, but it does
not by itself add or remove RSScript public contracts; public contract changes
must still be visible through selected `.rssi` interface paths and the effective
interface hash.

The package manager does not resolve Rust crates here. Cargo does.

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
forbid   fail if the fact is present; under strict mode, fail if the fact is
         unknown and must be known to enforce the policy.
review   allow, but classify and report the package as elevated or high risk.
allow    allow without policy error, still report in metadata.
```

`rss_unsafe_apis` refers to `.rssi` functions that expose `effects(unsafe)`.
`wrapper_unsafe_blocks` refers to unsafe Rust in the native wrapper crate itself.
`transitive_unsafe_blocks` refers to unsafe Rust in Rust dependencies.

These are intentionally separate because they have different review meanings.
A safe RSScript API may be implemented using Rust `unsafe` internally, but that
implementation fact must still be visible in package metadata when detected or
declared.

### 6.11 `[workspace]`

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

Provisional v0.5 package-interface syntax admits explicit namespaces and opaque
interface types. The compiler frontend owns the exact grammar and normalized
symbol form. Package tooling follows the frontend normalizer and must not accept
a second, package-manager-only interface syntax.

Canonical namespace form is namespace-relative: once inside `namespace Json`,
public declarations do not repeat the `Json.` prefix. The normalized exported
symbols are still `Json.parse` and `Json.field_string`. Method-like names such as
`HttpClient.get` are relative to the active namespace; authors write
`HttpClient.get`, not `Http.HttpClient.get`.

```rust
// interface/json.rssi

features: native

namespace Json

opaque struct JsonValue
opaque struct JsonError

native fn parse(
    text: read String,
) -> Result<fresh JsonValue, JsonError>
    effects(native)

native fn field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
    effects(native)
```

A Rust native wrapper API must be visible as a native boundary in `.rssi`;
otherwise review tools cannot distinguish pure RSScript implementation from
external Rust implementation.

### 7.2 Opaque interface types

An opaque interface type is a public RSScript type whose representation is not
specified by the `.rssi` contract.

```rust
opaque struct JsonValue
opaque struct JsonError
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

If a `.rssi` file writes a bodyless `struct` without `opaque`, the compiler must
interpret it according to the language specification. Package tooling must not
silently treat an ordinary bodyless `struct` as an opaque native contract type
unless the compiler frontend does so.

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

### 7.6 Interface content hash

The package manager computes a stable hash of compiler-normalized public `.rssi`
content.

Included in the interface content hash:

```text
exported namespaces and public roots
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

The interface content hash is useful for detecting that two package versions
have the same public contract.

### 7.7 Effective interface hash

The effective interface hash binds a selected feature set to an interface
content hash.

```text
effective_interface_hash = hash(
  normalized selected package feature set,
  interface_content_hash
)
```

The package name, version, source URL/path, and archive checksum are recorded
separately in `rsspkg.lock`. They are not folded into the interface content hash,
so a patch release with identical public contract can be recognized as
semantically unchanged.

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

### 8.2 Resolution order

```text
1. Read root rsspkg.toml.
2. Resolve RSScript package dependencies.
3. Fetch packages or use local paths.
4. Verify checksums if locked.
5. Resolve package features.
6. Load .rssi interfaces.
7. Ask the compiler frontend to normalize effective interfaces.
8. Compute interface content hashes and effective interface hashes.
9. Build effective interface environment.
10. Check RSScript sources.
11. Generate review metadata.
12. Generate build plan.
13. Delegate native graph to Cargo only when a command requires native build or
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

Future versions may support multiple major versions with explicit namespace
disambiguation.

### 8.4 Feature resolution

RSScript package features resolve deterministically.

Default MVP rule:

```text
Feature unification is additive, like Cargo features.
```

Review metadata must indicate when a feature changes risk.

Example:

```text
rss-http feature native-tls enabled by rss-client
  -> effective interface hash changed if public APIs or effects changed
  -> package risk elevated or high depending on native policy
  -> native dependency graph changed
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

### 8.6 Two lockfiles

RSScript and Cargo have different lock responsibilities.

```text
rsspkg.lock   RSScript semantic dependency lock
Cargo.lock    Rust implementation dependency lock
```

A single lockfile would mix semantic contract resolution with implementation
crate resolution.

### 8.7 `rsspkg.lock` authoritative fields

`rsspkg.lock` records authoritative dependency state:

```text
resolved RSScript package graph
package name and version
package source: registry/path/git-reserved/vendor
package archive checksum when applicable
selected package features
interface content hash
effective interface hash
native wrapper source hash, if native.rust is enabled
native binding manifest hash, if present
implementation source hash for published pure RSScript packages, when available
provider resolution for interface-only packages used in executable builds
```

`rsspkg.lock` may also record advisory/cache fields:

```text
review metadata hash
review metadata schema version
review tool version
native metadata summary hash
```

Advisory fields must be labeled as advisory. A review metadata hash change alone
must not be presented as a public contract change.

### 8.8 `Cargo.lock`

`Cargo.lock` records Rust crate resolution. Applications using native wrappers
should commit `Cargo.lock` when reproducibility matters.

A native wrapper update may change `Cargo.lock` even when `.rssi` is unchanged.
Package diff must report this as an implementation/native dependency change, not
as a public RSScript contract change.

### 8.9 Update behavior

On update, the package manager should report:

```text
RSScript package version changes
interface content hash changes
effective interface hash changes
selected feature changes
review metadata changes and whether they are schema/tool-only changes
native wrapper source changes
binding manifest changes
Cargo.lock changes
semantic diff summary
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
generated Rust package assembly
source-map-aware diagnostic integration
```

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

### 9.5 Binding manifest

A native wrapper package may provide:

```toml
# native/bindings.rssbind.toml

[bindings]
"Json.parse" = "rss_json_native::json_parse"
"Json.field_string" = "rss_json_native::json_field_string"

[types]
"JsonValue" = "rss_json_native::JsonValue"
"JsonError" = "rss_json_native::JsonError"
```

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
RSScript String       <-> Rust String / &str through adapter views
RSScript Bytes        <-> Vec<u8> / &[u8]
RSScript Buffer       <-> Vec<u8> / wrapper buffer
RSScript Result       <-> Rust Result through adapter mapping
RSScript Option       <-> Rust Option
RSScript resource     <-> Rust type implementing rss_rt::Resource
RSScript class/managed <-> rss_rt::Managed<T>
RSScript read/mut views <-> adapter-managed read/write views
```

The bridge is not a general FFI surface. It is generated for declared `.rssi`
contracts and native binding manifests.

### 9.7 Native conformance levels

Native wrapper conformance is reported in levels.

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

Level 3: semantic declarations recorded
  - native wrapper declares or metadata records whether it may retain, block,
    allocate, panic, spawn, use env, access filesystem/network, or call FFI.
  - these are trusted declarations or best-effort scans, not full proofs.

Level 4: audited conformance
  - external audit/test evidence is attached and hash-pinned.
  - registry may display audit status separately from computed metadata.
```

Package metadata must report which levels have been achieved. It must not imply
that arbitrary Rust semantic behavior was proven when only binding existence or
adapter type-checking occurred.

### 9.8 Adapter checking

Native ABI adapter checking is optional in the MVP but should be the standard
for publish readiness.

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
  "source": "cargo_metadata",
  "scope": "transitive"
}
```

Possible `value`:

```text
true
false
unknown
```

Possible `source`:

```text
manifest
cargo_metadata
binding_manifest
generated_adapter_check
source_scan_best_effort
author_declaration
audit
sandbox_observation
not_scanned
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

Recommended strict CI policy:

```toml
[native.rust.policy]
build_scripts = "forbid"
proc_macros = "forbid"
native_links = "forbid"
ffi = "forbid"
rss_unsafe_apis = "forbid"
wrapper_unsafe_blocks = "forbid"
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

The two are related but not identical. A public API is language-level
`must_review` because it is a public contract, but that baseline public-contract
fact does not automatically make the package elevated or high. At package level,
risk is aggregated from the kind of public facts that appear: native, unsafe,
resource, retention, mutation, async review boundaries, unknown facts, build-time
execution, implementation changes, and policy. For example, a native `.rssi`
function is a `must_review` export because it crosses a native boundary; at
package level, that native boundary may make the package `elevated` or `high`
depending on policy, build-time execution facts, unsafe facts, and audit status.

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
a native fact required by policy is unknown
an implementation provider is missing for an executable interface-only dependency
native binding targets cannot be resolved in review-only mode and policy requires
  them to be known
a registry checksum or lockfile hash is missing or mismatched
a semantic diff cannot be computed for an updated dependency
```

Unknown must not be treated as safe. If policy rejects unknown risk, the
operation fails with computed risk still displayed as `unknown`; policy failure
status is separate from the computed package risk tier.

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
any exported async API or selected feature exposes review-visible async contracts
any exported API uses local/resource/ResourcePool/retains/mut/take behavior that
  requires review but is bounded by visible contracts
native implementation changed while .rssi is unchanged
Cargo.lock changed for a native wrapper dependency
package features add async/native/build/proc-macro/resource/retention risk
managed closure capture retention appears in exported behavior
```

### 10.6 Low risk

A package risk may be `low` only if all of the following hold:

```text
all exported contracts parse and classify successfully
language-level must_review reasons are limited to baseline public-contract
  review facts accepted by policy
no async APIs unless project policy explicitly permits review-visible async
  signatures as low before executable async exists
no native APIs
no unsafe APIs
no ResourcePool/resource APIs
no retaining APIs
no mut/take public APIs unless policy explicitly permits them as low for the project
no unknown APIs or unknown required facts
no build scripts/proc macros/native links/FFI in the package graph under the selected features
```

A language-level `must_review` classification caused only by `public_api` does
not by itself block package-level `low`. Elevated or high package risk begins
when the public contract exposes additional review facts such as async, native,
unsafe, resource ownership, retention, mutation/take, unknown metadata, or
build-time native execution.

### 10.7 Computed risk and declared expectation

Computed risk is authoritative for tooling decisions.

```text
computed_risk != declared_expectation
```

is reported as a metadata mismatch, not treated as a reason to trust the declared
expectation.

---

## 11. Package Check and Build Workflow

### 11.1 `rss pkg check`

Canonical command namespace is `rss pkg`. v0.5 does not define command aliases.

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
Cargo metadata scan if native.rust enabled and metadata is available without build execution
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

### 11.2 Pure RSScript application

`rss check` is the compiler/frontend check command defined by the language spec.
For a package directory or source file inside a package, it may load
`rsspkg.toml` and dependency interfaces, but it does not run Cargo or execute
native build code.

```text
rss check
  -> load rsspkg.toml when present
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

### 11.3 Application with native wrappers

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

### 11.4 CI workflow

Recommended review-first CI:

```sh
rss pkg check
rss pkg tree
rss pkg metadata --verify
rss pkg diff --lockfile
rss check src/main.rss
rss review --map src/
```

Native ABI CI:

```sh
rss pkg check --native-abi
rss verify-rust src/main.rss
```

Strict dependency policy:

```sh
rss pkg check --deny-high-risk --deny-unknown --deny-unsafe-apis
rss pkg diff --deny-high-risk --deny-unknown --deny-unsafe-apis
```

---

## 12. Semantic Dependency Diff

### 12.1 Purpose

A package update should produce a semantic diff of public contracts and relevant
implementation/native facts.

Canonical command:

```sh
rss pkg diff
```

Examples:

```sh
rss pkg diff --lockfile old/rsspkg.lock --new-lockfile rsspkg.lock
rss pkg diff rss-http@0.3.1 rss-http@0.4.0
rss pkg diff --update-plan
```

### 12.2 Diff inputs

```text
old rsspkg.lock
new rsspkg.lock
old normalized effective .rssi contracts
new normalized effective .rssi contracts
old computed review metadata
new computed review metadata
old native binding/native source hashes
new native binding/native source hashes
Cargo.lock changes if native wrappers are present
```

### 12.3 Breaking or must-review changes

```text
public function removed
public type removed
public namespace/exported root removed
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
async signature added or changed
native effect added
unsafe effect added
resource return introduced
resource lifetime behavior changed
constructor/variant field or payload effect changed
handle/weak field marker changed
guarantee removed such as no_panic/noalloc/no_block/pure
opaque/public type kind changed
unknown classification introduced
```

### 12.4 Review-relevant but possibly compatible changes

```text
new public function added
new public type added
new namespace/exported root added
fresh guarantee added
retains effect removed
guarantee added
native implementation changed with unchanged .rssi
native binding manifest changed
Cargo.lock changed for native wrapper package
package risk increased from low to elevated/high/unknown
new package feature changes async/native/build/proc-macro/resource/retention risk
review metadata changed because risk algorithm/schema changed
```

### 12.5 No public contract delta / low semantic-contract risk changes

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

### 12.6 Example output

```text
Dependency review: rss-http 0.3.1 -> 0.4.0

PACKAGE RISK
  elevated -> high

MUST REVIEW
  HttpClient.send
    + effects(retains(request))
    + effects(native)

  Response.body_stream
    new API returns resource BodyStream

  ResourcePool<Connection>.lazy_new
    factory retains create closure

NATIVE CHANGES
  Cargo.lock changed
    reqwest 0.12.4 -> 0.12.8
    rustls 0.23.12 -> 0.23.18

  build_scripts: false -> true via transitive dependency
    source: cargo_metadata
    policy: review
```

### 12.7 Semantic version check

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

Command:

```sh
rss pkg semver-check --since 0.3.1
```

---

## 13. Package Review Metadata

### 13.1 Metadata file

Generated file:

```text
review/package-review.json
```

Schema example:

```json
{
  "schema": "rss.review.package.v2",
  "tool": {
    "name": "rsspkg",
    "version": "0.5.0-alpha"
  },
  "package": {
    "name": "rss-json",
    "version": "0.1.0"
  },
  "interface": {
    "selected_features": [],
    "interface_content_hash": "sha256:...",
    "effective_interface_hash": "sha256:...",
    "normalizer": "rssc 0.5.0-alpha"
  },
  "risk": {
    "computed": "elevated",
    "declared_expectation": "elevated",
    "reasons": ["native_boundary", "result_return", "fresh_return"]
  },
  "summary": {
    "public_types": 2,
    "public_functions": 2,
    "mutating_apis": 0,
    "retaining_apis": 0,
    "closure_capture_retaining_apis": 0,
    "resource_apis": 0,
    "fresh_returning_apis": 1,
    "async_apis": 0,
    "native_apis": 2,
    "unsafe_apis": 0,
    "unknown_apis": 0
  },
  "native": {
    "rust": true,
    "conformance": {
      "native_boundary_declared": "checked",
      "binding_existence": "checked",
      "adapter_typechecked": "not_run",
      "semantic_effects": "trusted_declaration",
      "audit": "none"
    },
    "facts": [
      {
        "name": "build_scripts",
        "value": false,
        "source": "cargo_metadata",
        "scope": "package"
      },
      {
        "name": "proc_macros",
        "value": false,
        "source": "cargo_metadata",
        "scope": "transitive"
      },
      {
        "name": "wrapper_unsafe_blocks",
        "value": false,
        "source": "source_scan_best_effort",
        "scope": "package"
      },
      {
        "name": "transitive_unsafe_blocks",
        "value": "unknown",
        "source": "not_scanned",
        "scope": "transitive"
      }
    ]
  },
  "exports": [
    {
      "name": "Json.parse",
      "kind": "function",
      "classification": "must_review",
      "reasons": ["native_boundary", "returns_fresh", "returns_result"]
    }
  ]
}
```

### 13.2 Metadata generation

Command:

```sh
rss pkg metadata
```

or:

```sh
rss pkg review --emit-metadata
```

Inputs:

```text
compiler-normalized .rssi interfaces
.rss source if present
native wrapper declaration
binding manifest
Cargo metadata if native wrapper exists
review policy
optional source scan/audit inputs
```

If a `.rssi` contract has frontend errors, those diagnostics are reported as
unknown contract exports and counted as unknown APIs, because the public semantic
contract cannot be trusted.

If a package has no `.rssi` surface, local prototype tooling may fall back to
public source declarations for counts and exports, but publishing public packages
requires `.rssi`.

### 13.3 Metadata trust

Registry-provided metadata is useful for search and preview. Consumers should
verify metadata by checking package hashes and optionally regenerating metadata
locally.

Rule:

```text
Metadata is cacheable.
.rssi contract hash is authoritative.
Computed local metadata wins over registry metadata for policy decisions.
```

### 13.4 Metadata-only changes

A review metadata hash may change because:

```text
tool version changed
schema version changed
risk aggregation rules changed
Cargo metadata changed
package contents changed
```

Tooling must distinguish these cases when possible. A metadata-only change is
not a public contract delta unless the normalized effective interface changed.

---

## 14. Registry, Publishing, and Security

### 14.1 Registry model

A registry is not required by the language core.

The package model must work with:

```text
local path dependencies
vendored dependencies
private registries
future public registry
syntax-reserved git dependencies
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
version compatibility data
deprecation/advisory metadata
audit evidence references
```

### 14.2 Registry index entry

```json
{
  "name": "rss-json",
  "version": "0.1.0",
  "checksum": "sha256:...",
  "interface_content_hash": "sha256:...",
  "effective_interface_hash_default": "sha256:...",
  "review_hash": "sha256:...",
  "review_schema": "rss.review.package.v2",
  "risk": "elevated",
  "native": true,
  "unsafe_apis": false,
  "dependencies": {
    "rss-core": "^0.5"
  },
  "features": {
    "default": [],
    "streaming": []
  }
}
```

A registry index entry is a preview and resolution aid. Lockfile verification
uses package checksums and normalized interface hashes.

### 14.3 Review-oriented registry UI

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
Cargo native dependency summary
native conformance level
fact evidence sources: cargo_metadata, source_scan, declaration, audit, unknown
```

### 14.4 Publish validation

`rss pkg publish --dry-run` should validate:

```text
manifest valid
interfaces parse
public APIs explicit
effective interface hashes computed
implementation checks
native metadata generated
semantic version check
package review metadata generated
package archive reproducible
unknown package review risk blocks publish readiness unless explicitly allowed
```

`rss pkg publish --dry-run --native-abi` additionally runs generated native
adapter type-checking and may execute native build code after policy approval.

Yanking should not break existing lockfile builds, but new resolution should
avoid yanked versions unless explicitly allowed.

### 14.5 Security and supply chain

RSScript packages may contain:

```text
RSScript source
Rust native source
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

### 14.6 Build-time execution policy

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

## 15. CLI Design

Canonical command namespace for the v0.5 prototype:

```sh
rss pkg check
rss pkg check --native-abi
rss pkg tree
rss pkg review
rss pkg metadata
rss pkg diff
rss pkg lock
rss pkg publish
rss pkg vendor
rss pkg semver-check
```

No `rss package ...` command is defined for v0.5 tooling. No `rss review deps`
alias is normative in v0.5; dependency review belongs to `rss pkg diff` and
`rss pkg review`.

### 15.1 `rss pkg check`

Runs manifest, interface, source, lockfile, and non-executing native checks.

```sh
rss pkg check
rss pkg check --deny-unknown
rss pkg check --deny-high-risk
rss pkg check --native-abi
```

### 15.2 `rss pkg tree`

Shows dependency graph with risk:

```text
my-app
├── rss-core 0.5.0 [low]
├── rss-json 0.1.0 [elevated, native]
└── rss-http 0.4.0 [high, native, build.rs, resource]
```

### 15.3 `rss pkg review`

Generates package-level review report for the current package or workspace.

```sh
rss pkg review
rss pkg review --emit-metadata
rss pkg review --all-features
```

It must not execute native build code by default.

### 15.4 `rss pkg metadata`

Emits machine-readable metadata.

```sh
rss pkg metadata --format json
rss pkg metadata --verify
```

`--verify` recomputes metadata locally and compares against committed or registry
metadata.

### 15.5 `rss pkg diff`

Compares package versions, lockfiles, or update plans.

```sh
rss pkg diff rss-http@0.3.1 rss-http@0.4.0
rss pkg diff --lockfile old/rsspkg.lock --new-lockfile rsspkg.lock
rss pkg diff --update-plan
```

### 15.6 `rss pkg lock`

Updates or checks `rsspkg.lock`.

```sh
rss pkg lock
rss pkg lock --check
```

### 15.7 `rss pkg vendor`

Vendors dependencies locally for offline/reproducible builds.

```sh
rss pkg vendor
```

For the prototype, local path dependencies can be copied into:

```text
vendor/<name>-<version>/
vendor/rss-vendor.json
```

Registry support depends on resolver availability. Git dependencies remain
syntax-reserved until their resolver exists.

### 15.8 Future commands

Future package-management commands may extend the same `rss pkg` namespace, but
they are not part of the current executable surface until implemented and tested:

```sh
rss pkg init
rss pkg add <package>
rss pkg remove <package>
rss pkg update [package]
rss pkg clean
```

---

## 16. Review Policies and Budgets

A project may define dependency review policy.

```toml
[review.policy]
deny_unknown = true
deny_native = false
deny_unsafe_apis = true
max_high_risk_dependencies = 0
max_native_dependencies = 5
require_lockfile = true
require_review_metadata = true
native_api_risk = "high"
build_execution_default = "forbid"

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
error: package rss-http is high risk, max_high_risk_dependencies=0
warning: package rss-image uses build.rs; policy requires review
error: package rss-json has unknown native wrapper unsafe status, but deny_unknown=true
```

Budget dimensions:

```text
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

---

## 17. Diagnostics

Package manager diagnostics use stable `PKGxxxx` codes.

Diagnostic classes:

```text
manifest error
dependency resolution error
interface normalization error
interface conflict
feature-conditioned interface conflict
semantic version mismatch
native wrapper missing binding
native binding target mismatch
native adapter type-check failure
native risk policy violation
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
PKG04xx  semantic diff and semver
PKG05xx  review metadata and risk policy
PKG06xx  native bindings and native conformance
PKG07xx  Cargo integration
PKG08xx  registry/publish
PKG09xx  provider/interface-only package resolution
```

Current package-manager diagnostic allocations:

```text
PKG0501  review policy violation
PKG0601  native binding metadata or conformance mismatch
```

Boundary with language diagnostics:

```text
RSxxxx diagnostics are compiler/frontend diagnostics over RSScript source and
.rssi semantic contracts.
PKGxxxx diagnostics are package-manager diagnostics over manifests, dependency
resolution, selected package features, lockfiles, registries, native binding
metadata, native conformance, and Cargo integration.
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

## 18. Examples

### 18.1 Wrapping `serde_json`

RSScript interface:

```rust
// interface/json.rssi

features: native

namespace Json

opaque struct JsonValue
opaque struct JsonError

native fn parse(
    text: read String,
) -> Result<fresh JsonValue, JsonError>
    effects(native)

native fn field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
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

Binding manifest:

```toml
# native/bindings.rssbind.toml

[bindings]
"Json.parse" = "rss_json_native::json_parse"
"Json.field_string" = "rss_json_native::json_field_string"

[types]
"JsonValue" = "rss_json_native::JsonValue"
"JsonError" = "rss_json_native::JsonError"
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

The reviewer sees `.rssi` native boundaries and semantic contracts, not
`serde_json` internals.

Review metadata summary:

```text
risk: elevated
reason: native_boundary
native APIs: 2
unsafe RSScript APIs: 0
build.rs: false, source=cargo_metadata
proc macros: false, source=cargo_metadata
wrapper unsafe blocks: false, source=source_scan_best_effort
adapter typechecked: not_run unless rss pkg check --native-abi is used
```

### 18.2 HTTP wrapper risk

```rust
features: native

namespace Http

opaque struct HttpClient
opaque struct Response
opaque struct HttpError
opaque struct Url

native fn HttpClient.get(
    client: read HttpClient,
    url: read Url,
) -> Result<Response, HttpError>
    effects(native)

native fn Response.body_text(
    response: read Response,
) -> Result<String, HttpError>
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
  build.rs: yes via dependency graph, source=cargo_metadata
  public APIs: 8
  network APIs: 3, source=author_declaration
```

### 18.3 Interface-only provider

Interface package:

```rust
// interface/env.rssi

namespace Env

opaque struct EnvError

native fn get(name: read String) -> Result<String, EnvError>
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
exports = ["Env", "EnvError"]

[provider]
mode = "platform_provided"
```

Executable builds must either target a platform that provides this interface or
resolve a package provider.

---

## 19. Workspace Model

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

Development overrides may later use:

```toml
[patch]
rss-json = { path = "../rss-json" }
```

Patch syntax can be deferred until after local path dependencies work.

Workspace-level review policy may override package-local default policy for CI.
Package-local policy is still useful for publish readiness and expected risk.

---

## 20. MVP Plan

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
  computed package risk summary
  API classification
  native risk summary with evidence sources
  CI policy checks

MVP 4: Cargo native wrapper metadata integration
  native/rust package discovery
  binding manifest checks
  Cargo metadata scan without build execution
  build.rs/proc-macro/native-link detection where available

MVP 5: Optional native ABI adapter check
  generated bridge adapters
  cargo check under --native-abi
  native conformance level reporting
  source-map-aware native boundary diagnostics

MVP 6: Semantic dependency diff
  rss pkg diff
  compare old/new effective .rssi
  classify semantic changes
  report native source/binding/Cargo.lock changes
  produce human and JSON reports

MVP 7: Registry protocol
  package archive format
  index format
  checksums
  publish dry-run
  local/private registry support

MVP 8: Public registry
  package search
  package page with review metadata
  semantic diff between versions
  advisories
  yanking
  signing policy
```

---

## 21. Open Questions

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
```

Not open in v0.5:

```text
- .rssi normalization belongs to the compiler frontend, not an independent
  package-manager normalizer.
- package risk is computed, not author-declared.
- review-only commands must not execute native build code by default.
- package metadata must use low_semantic_risk and must not emit legacy category
  names such as the old skip-safety label.
```

---

## 22. Summary

The RSScript package manager should be designed around one sentence:

```text
Cargo builds the implementation; RSScript packages publish reviewable semantic contracts.
```

The distinctive value of RSScript package management is that package dependencies
become semantically reviewable:

```text
.rssi defines public contract
effective interface hash captures selected-feature public semantics
rsspkg.lock locks semantic dependency graph
Cargo.lock locks Rust implementation graph
review metadata summarizes package risk
semantic diff explains dependency upgrades
native wrappers expose Rust crates through reviewable APIs
native facts report their evidence source
unknown risk is classified as unknown, not safe
review can happen before build-time native code executes
```

This gives RSScript a package ecosystem aligned with its language philosophy:

```text
Script-like source.
System-level boundaries.
Reviewable dependencies.
Rust-powered implementation.
```
