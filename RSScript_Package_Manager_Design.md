# RSScript Package Manager Design

Status: Draft / Ecosystem Design Candidate  
Version: 0.2  
Audience: RSScript compiler implementers, package authors, native binding authors, registry implementers, review-tool authors  
Scope: package model, dependency resolution, Cargo integration, semantic package review, native wrappers, registry protocol direction  
Non-scope: RSScript language syntax, core language semantics, full registry product design, centralized service policy

---

## 0. Executive Summary

RSScript package management is not intended to replace Cargo.

The RSScript package manager is a **semantic and review layer** over RSScript interfaces, RSScript source packages, Rust native wrappers, and Cargo-based implementation builds.

The core model is:

```text
Package = reviewable semantic contract + implementation artifacts.
```

For RSScript, a package is not just code. A package publishes a public `.rssi` interface surface that describes review-critical behavior:

```text
read / mut / take
fresh returns
retention effects
resource boundaries
native / unsafe boundaries
file features
review risk metadata
```

Cargo remains the build substrate for Rust code and Rust dependencies:

```text
Cargo owns:
  Rust crate dependency resolution
  crates.io integration
  native wrapper compilation
  Cargo.lock
  target/platform handling
  workspace build mechanics

RSScript package manager owns:
  .rssi semantic contracts
  RSScript dependency resolution
  RSScript package lockfile
  interface loading
  semantic dependency diff
  review metadata
  native boundary classification
  generated Cargo workspace glue
```

The long-term goal is not merely `crates.io for RSScript`. The goal is a package ecosystem where dependency changes are reviewable at the semantic boundary.

A dependency update should answer:

```text
What public contracts changed?
Which APIs now mutate?
Which APIs now retain values?
Which APIs now return resources?
Which packages introduced native or unsafe boundaries?
Which dependency changes require human review?
```

---

## 1. Design Thesis

RSScript exists because AI-era software shifts the bottleneck from writing code to reviewing generated code.

The package manager must follow the same thesis.

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
Did a package introduce hidden retention, mutation, resources, native code, or unsafe code?
Can the reviewer inspect the public contract without reading the Rust implementation?
```

Therefore, package management is part of the RSScript review story.

---

## 2. Goals

### 2.1 Primary goals

1. Provide a package format for RSScript libraries and applications.
2. Make `.rssi` files the public semantic contract of a package.
3. Support pure RSScript packages.
4. Support Rust crate wrappers behind RSScript interfaces.
5. Reuse Cargo for Rust dependency resolution and native compilation.
6. Produce deterministic RSScript dependency locks.
7. Support semantic diff for package upgrades.
8. Generate machine-readable package review metadata.
9. Preserve source-map-aware diagnostics through generated Rust packages.
10. Allow future registries to publish review summaries and semantic diff history.

### 2.2 Secondary goals

1. Support local path dependencies before registry dependencies.
2. Support workspace development.
3. Support vendored/offline builds.
4. Support CI review policies.
5. Support native risk classification for build scripts, proc macros, linked libraries, FFI, and unsafe code.
6. Make package publishing validate semantic interface consistency.

---

## 3. Non-goals

The RSScript package manager should not attempt to replace Cargo in the MVP.

Non-goals:

```text
full Rust dependency resolver replacement
crates.io replacement in the MVP
custom Rust build system
custom linker/toolchain management
native binary package manager
sandbox design for arbitrary native builds
package signing authority design
full registry moderation policy
language-level import syntax finalization
automatic inference of RSScript effects from Rust code
```

Important boundary:

```text
Rust implementation details may power a package.
Rust implementation details must not define the RSScript semantic contract.
```

---

## 4. Core Concepts

### 4.1 RSScript package

An RSScript package is a versioned unit that may contain:

```text
.rssi semantic interface files
.rss implementation files
Rust native wrapper code
package manifest
package lock metadata
review metadata
tests
examples
```

A package may be:

```text
library package
binary/tool package
interface-only package
native wrapper package
core/trusted package
workspace package
```

### 4.2 Semantic contract

The semantic contract of a package is its public `.rssi` surface.

The `.rssi` contract declares:

```text
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

The package manager treats the `.rssi` surface as the primary artifact for review, compatibility, and dependency diff.

### 4.3 Implementation artifact

A package implementation may be written in:

```text
RSScript source
Rust native wrapper code
both RSScript and Rust
```

Implementation artifacts must conform to the public `.rssi` contract.

### 4.4 Native wrapper

A native wrapper is Rust code that adapts one or more Rust crates to an RSScript `.rssi` contract.

Example:

```text
serde_json crate
  -> rss-json native wrapper
  -> json.rssi
  -> RSScript Json.parse API
```

The raw Rust crate API is not automatically exposed to RSScript. Only `.rssi` APIs are visible.

### 4.5 Review metadata

Review metadata is generated from `.rssi`, RSScript source, and native package declarations.

It summarizes:

```text
file features
public API count
mutating APIs
retaining APIs
resource APIs
fresh-returning APIs
native APIs
unsafe APIs
package risk level
semantic diff against previous version
```

Review metadata is advisory and machine-readable. The authoritative contract is still the `.rssi` surface plus package checksums.

---

## 5. Design Principles

### 5.1 Interface-first

Public package behavior is defined by `.rssi` before implementation details.

```text
.rssi is the package contract.
.rss or Rust is the implementation.
```

### 5.2 Cargo-first for Rust implementation

Cargo remains the implementation build system for Rust wrappers and Rust dependencies.

RSScript package management should generate Cargo integration, not duplicate Cargo.

### 5.3 Review metadata is first-class

Dependency management must support review operations, not only install/build operations.

### 5.4 Raw Rust APIs do not leak by default

Rust crates are used behind wrapper packages.

RSScript users should not be required to understand Rust lifetimes, trait bounds, proc macros, `Pin`, `Cow`, `Arc`, `Mutex`, or crate-specific generic APIs in order to review RSScript code.

### 5.5 Semantic diffs are package diffs

A package update is not fully described by source diff or version diff.

RSScript package updates must be able to report semantic contract changes.

### 5.6 Unknown risk must not be hidden

If package review cannot classify an API or native boundary, it must mark it as `unknown`, not safe.

### 5.7 Over-report early, never under-report risk

Early implementations may be conservative. They should not classify risky or unknown APIs as safe.

---

## 6. Package Types

### 6.1 Pure RSScript package

Contains `.rssi` and `.rss` files.

```text
rss-math/
  rsspkg.toml
  interface/math.rssi
  src/math.rss
  tests/math_test.rss
```

No native Rust wrapper is required.

### 6.2 Native wrapper package

Contains `.rssi` plus Rust code that wraps external crates.

```text
rss-json/
  rsspkg.toml
  interface/json.rssi
  native/rust/Cargo.toml
  native/rust/src/lib.rs
  tests/json_test.rss
```

The Rust wrapper may depend on `serde_json`, but RSScript users only see `json.rssi`.

### 6.3 Interface-only package

Contains `.rssi` contracts but no implementation.

Used for:

```text
platform APIs
externally supplied runtime APIs
mock/test interfaces
cross-package contracts
```

### 6.4 Core package

A trusted package shipped with the compiler/runtime distribution.

Examples:

```text
rss-core
rss-fs
rss-json
rss-test
rss-resource
```

Core packages still expose `.rssi` contracts. Being core does not remove the need for semantic interfaces.

### 6.5 Tool package

A package that provides executable commands.

Examples:

```text
rss-fmt-extra
rss-review-ci
rss-bindgen
```

Tool packages may contain binary entry points and may have stronger native/build risk policies.

---

## 7. Package Layout

### 7.1 Recommended package layout

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
```

### 7.2 Minimal pure package

```text
my-utils/
  rsspkg.toml
  interface/lib.rssi
  src/lib.rss
```

### 7.3 Minimal native wrapper package

```text
rss-regex/
  rsspkg.toml
  interface/regex.rssi
  native/rust/Cargo.toml
  native/rust/src/lib.rs
```

### 7.4 Generated build directory

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

Generated Rust is an internal build artifact. It is not the RSScript package contract.

---

## 8. Manifest: `rsspkg.toml`

### 8.1 Example

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

[review]
risk = "low"
allow_native = true
allow_unsafe = false
unknown_is_error = false

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
cargo_features = []
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
links = []
```

### 8.2 `[package]`

Required:

```toml
[package]
name = "my-package"
version = "0.1.0"
edition = "2026"
```

Recommended:

```toml
description = "..."
license = "MIT"
repository = "..."
readme = "README.md"
keywords = ["json", "parser"]
categories = ["data"]
```

Package names should be stable, lowercase, and registry-unique.

### 8.3 `[interfaces]`

Declares where public `.rssi` files live.

```toml
[interfaces]
paths = ["interface"]
exports = ["Json", "JsonValue", "JsonError"]
```

The package manager loads these interfaces for dependents.

### 8.4 `[sources]`

Declares RSScript implementation source roots.

```toml
[sources]
paths = ["src"]
```

A native wrapper package may omit `[sources]` if all implementation is native Rust.

### 8.5 `[dependencies]`

Declares RSScript package dependencies.

```toml
[dependencies]
rss-core = "0.5"
rss-json = { version = "0.2", features = ["streaming"] }
my-local = { path = "../my-local" }
my-git = { git = "https://example.org/my-git", rev = "abc123" }
```

Dependencies are RSScript packages, not arbitrary Rust crates.

Rust crates belong in `native/rust/Cargo.toml`.

### 8.6 `[dev-dependencies]`

Used only for tests/examples/tools.

```toml
[dev-dependencies]
rss-test = "0.5"
```

### 8.7 `[features]`

Package features select optional RSScript package APIs or implementation paths.

```toml
[features]
default = []
streaming = []
native-tls = []
```

Package features are not the same as RSScript file features such as `features: local`.

Rules:

```text
Package features must not silently introduce native or unsafe boundaries.
If a package feature enables native, unsafe, build scripts, or proc macros, review metadata must report it.
```

### 8.8 `[review]`

Package-level declared review policy and risk hint.

```toml
[review]
risk = "elevated"          # low | elevated | high | unknown
allow_native = true
allow_unsafe = false
unknown_is_error = false
max_public_params = 8
max_nested_type_depth = 4
```

The compiler may compute a risk different from the declared hint. Computed metadata wins.

### 8.9 `[native.rust]`

Declares Rust native wrapper integration.

```toml
[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
cargo_features = []
build_scripts = "forbid"   # forbid | review | allow
proc_macros = "forbid"     # forbid | review | allow
unsafe = "forbid"          # forbid | review | allow
links = []
```

The package manager does not resolve Rust crates here. Cargo does.

### 8.10 `[workspace]`

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

## 9. Cargo Integration Model

### 9.1 Core rule

RSScript package management does not replace Cargo.

```text
Cargo is the Rust implementation substrate.
RSScript package management is the semantic/review layer.
```

### 9.2 Responsibilities

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
.rssi interface loading
RSScript semantic lockfile
semantic package diff
review metadata generation
native boundary classification
generated Rust package assembly
source-map-aware diagnostic integration
```

### 9.3 Build pipeline

```text
rsspkg.toml
  -> resolve RSScript package graph
  -> fetch/load .rssi interfaces
  -> check RSScript source against interfaces
  -> lower RSScript to Rust source
  -> generate Cargo package/workspace glue
  -> include native/rust wrapper crates as path dependencies
  -> invoke cargo check/build/run
  -> remap rustc diagnostics through RSScript source maps
```

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

This Cargo manifest is passed to Cargo. RSScript does not reimplement its resolver.

### 9.5 Generated Cargo workspace

For an application depending on multiple RSScript packages, the package manager may generate a temporary Cargo workspace:

```text
target/rss/workspace/
  Cargo.toml

  app/
    Cargo.toml
    src/lib.rs
    src/main.rs
    rsscript-source-map.json

  native/
    rss_json_native/       # path to or copy of native wrapper crate
    rss_http_native/
```

Generated workspace responsibilities:

```text
link generated RSScript Rust code to rsscript-runtime
link native wrapper crates
preserve source map files
call cargo check/build/run
```

### 9.6 Cargo features

Cargo features stay inside native wrapper crates by default.

RSScript package features may choose wrapper behavior, but should not expose arbitrary Cargo features directly unless the package explicitly declares them.

Bad default:

```text
rss pkg add rss-http --cargo-feature danger-native-tls
```

Preferred:

```toml
[features]
native-tls = []
```

The package metadata explains whether `native-tls` introduces native or platform risk.

### 9.7 Build scripts and proc macros

Cargo build scripts and proc macros are powerful native execution boundaries.

They must be classified in review metadata.

Policy values:

```text
forbid   fail if present
review   allow, but mark package high/elevated risk
allow    allow without error, still report in metadata
```

Recommended default for MVP:

```toml
build_scripts = "review"
proc_macros = "review"
unsafe = "review"
```

Recommended strict CI policy:

```toml
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
```

---

## 10. Lockfiles

### 10.1 Two lockfiles

RSScript and Cargo have different lock responsibilities.

```text
rsspkg.lock   RSScript semantic dependency lock
Cargo.lock    Rust implementation dependency lock
```

Both may be present.

### 10.2 `rsspkg.lock`

`rsspkg.lock` records the resolved RSScript package graph and semantic contract hashes.

Example:

```toml
version = 1

[[package]]
name = "rss-core"
version = "0.5.0"
source = "registry+https://registry.rsscript.org"
checksum = "sha256:..."
interface_hash = "sha256:..."
review_hash = "sha256:..."
features = []

[[package]]
name = "rss-json"
version = "0.1.0"
source = "registry+https://registry.rsscript.org"
checksum = "sha256:..."
interface_hash = "sha256:..."
review_hash = "sha256:..."
native_hash = "sha256:..."
features = []

[metadata]
rss_version = "0.5.0"
created_by = "rss pkg 0.1.0"
```

### 10.3 `Cargo.lock`

Cargo owns the native Rust dependency lock.

Generated Cargo workspaces should either:

```text
reuse the project Cargo.lock when stable
or generate a Cargo.lock under target/rss for temporary builds
```

For applications, committing a Cargo.lock is recommended if native wrappers are used.

### 10.4 Why not one lockfile?

A single lockfile would mix semantic package contracts with implementation crate resolution.

The separation is intentional:

```text
rsspkg.lock answers: did the RSScript contract graph change?
Cargo.lock answers: did the Rust implementation graph change?
```

A package can have unchanged `.rssi` but changed Rust dependencies. That is still review-relevant, but it is not the same as a semantic contract change.

### 10.5 Update behavior

On update, the package manager should report:

```text
RSScript package version changes
.rssi interface hash changes
review metadata changes
native wrapper source changes
Cargo.lock changes
semantic diff summary
```

Example:

```text
rss-json 0.1.0 -> 0.1.1
  .rssi unchanged
  native/rust changed
  Cargo.lock changed: serde_json 1.0.120 -> 1.0.125
  semantic contract change: none
  review required: native implementation update
```

---

## 11. Dependency Resolution

### 11.1 Graph layers

RSScript package management has three dependency graphs:

```text
1. Semantic package graph
   RSScript packages and .rssi contracts

2. RSScript implementation graph
   .rss source packages and interface imports

3. Native Rust graph
   Cargo crates used by native wrappers
```

The RSScript package manager resolves graph 1 and graph 2.
Cargo resolves graph 3.

### 11.2 Resolution order

```text
1. Read root rsspkg.toml
2. Resolve RSScript package dependencies
3. Fetch packages or use local paths
4. Verify checksums if locked
5. Load .rssi interfaces
6. Check feature compatibility
7. Build effective interface environment
8. Check RSScript sources
9. Generate build plan
10. Delegate native graph to Cargo
```

### 11.3 Version requirements

MVP version requirement forms:

```toml
rss-core = "0.5"
rss-json = "^0.1"
rss-http = { version = ">=0.2, <0.4" }
local-lib = { path = "../local-lib" }
```

MVP should avoid complex multi-version package graphs unless necessary.

Recommended MVP rule:

```text
A package graph should resolve to one version per package name.
```

Future versions may support multiple major versions with explicit namespace disambiguation.

### 11.4 Feature resolution

RSScript package features should be resolved deterministically.

Feature unification may follow Cargo-like additive rules, but review metadata must indicate when a feature changes risk.

Example:

```text
rss-http feature native-tls enabled by rss-client
  -> package risk elevated
  -> native dependency graph changed
```

### 11.5 Interface environment

The checker receives an interface environment assembled from:

```text
bundled core interfaces
root package interfaces
resolved dependency interfaces
explicit user-supplied interfaces
```

Conflicts must be diagnostics.

---

## 12. `.rssi` as Package Contract

### 12.1 Public contract

Every RSScript-facing public API must be declared in `.rssi`.

```rust
struct JsonValue
struct JsonError

pub fn Json.parse(
    text: read String,
) -> Result<fresh JsonValue, JsonError>

pub fn Json.field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
```

### 12.2 Contract is semantic, not just type-level

The interface must declare review-relevant behavior:

```rust
pub fn Cache.put(
    cache: mut Cache,
    key: read String,
    value: read Image,
) -> Unit
    effects(retains(key), retains(value))
```

This tells the reviewer:

```text
cache is mutated
key may be retained
value may be retained
```

### 12.3 Native implementation must conform

If the implementation is Rust, the wrapper must conform to the `.rssi` contract.

The compiler should not infer RSScript semantics from Rust signatures.

Bad model:

```text
Rust function type determines RSScript effect semantics.
```

Correct model:

```text
.rssi declares RSScript semantics.
Rust wrapper is checked/adapted against that contract where possible.
```

### 12.4 Interface hash

The package manager computes a stable hash of normalized public `.rssi` content.

The hash should ignore formatting but include semantic declarations.

Included in hash:

```text
type names and kinds
public function names
parameter names
data effects
parameter types
return types
fresh markers
effects clauses
generic bounds
native/unsafe markers
resource declarations
```

Not included:

```text
comments
formatting
private implementation files
non-public test interfaces
```

---

## 13. Native Rust Wrapper Model

### 13.1 Standard pattern

```text
Rust crate ecosystem
  -> native/rust wrapper
  -> .rssi contract
  -> RSScript code
```

Example:

```text
serde_json
  -> rss_json_native::json_parse
  -> Json.parse in json.rssi
  -> Json.parse(text: read body)
```

### 13.2 Wrapper responsibilities

A wrapper must:

```text
adapt Rust crate APIs to RSScript-friendly types
hide Rust lifetimes and trait-bound complexity
translate Rust errors into RSScript errors
preserve RSScript source span hooks where applicable
respect .rssi read / mut / take contracts
respect .rssi fresh / retains contracts
respect resource cleanup semantics
classify native/unsafe behavior
```

### 13.3 Binding manifest

A native wrapper package may provide a binding manifest:

```toml
[bindings]
"Json.parse" = "rss_json_native::json_parse"
"Json.field_string" = "rss_json_native::json_field_string"

[types]
"JsonValue" = "rss_json_native::JsonValue"
"JsonError" = "rss_json_native::JsonError"
```

Possible file:

```text
native/bindings.rssbind.toml
```

The exact format can be deferred. The key requirement is that generated Rust lowering can map RSScript callable symbols to wrapper functions.

### 13.4 Type bridge

Common bridge types:

```text
RSScript String  <-> Rust String / &str
RSScript Bytes   <-> Vec<u8> / &[u8]
RSScript Buffer  <-> Vec<u8> / wrapper buffer
RSScript Result  <-> Rust Result
RSScript Option  <-> Rust Option
RSScript resource <-> Rust type implementing rsscript_runtime::Resource
RSScript class/managed <-> runtime Gc/handle
```

### 13.5 No direct raw crate exposure

RSScript should not expose arbitrary Rust crate APIs directly.

Instead of exposing:

```text
reqwest::ClientBuilder
serde::Deserialize<'de>
tokio::spawn
hyper::Body
Pin<Box<dyn Future<...>>>
```

Expose reviewable RSScript APIs:

```rust
pub fn HttpClient.get(
    client: read HttpClient,
    url: read Url,
) -> Result<Response, HttpError>
    effects(native)
```

### 13.6 Native risk categories

Native wrapper metadata should classify:

```text
uses unsafe
uses FFI
uses build.rs
uses proc macros
links native library
performs blocking IO
spawns threads
uses async runtime
uses environment variables
uses filesystem/network during build
```

This metadata is part of package review.

---

## 14. Semantic Dependency Diff

### 14.1 Purpose

A package update should produce a semantic diff of public contracts.

Command:

```sh
rss pkg review update
```

or:

```sh
rss review deps
```

### 14.2 Diff inputs

Inputs:

```text
old rsspkg.lock
new rsspkg.lock
old package .rssi normalized contracts
new package .rssi normalized contracts
old review metadata
new review metadata
Cargo.lock changes if native wrappers are present
```

### 14.3 Diff categories

Breaking or must-review changes:

```text
public function removed
public type removed
parameter added
parameter removed
parameter renamed
parameter type changed
parameter effect read -> mut
parameter effect read/mut -> take
return type changed
fresh guarantee removed
retains effect added
native effect added
unsafe effect added
resource return introduced
resource lifetime behavior changed
guarantee removed, such as no_panic/noalloc/no_block/pure
unknown classification introduced
```

Review-relevant but possibly compatible changes:

```text
new public function added
new public type added
fresh guarantee added
retains effect removed
guarantee added
native implementation changed with unchanged .rssi
Cargo.lock changed for native wrapper package
package risk increased from low to elevated/high
```

Safe or low-risk changes:

```text
comments changed
formatting changed
private implementation changed with unchanged interface and no native change
new tests/examples added
review metadata regenerated with no semantic delta
```

### 14.4 Example output

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

REVIEW IF CHANGED
  Url.parse
    added guarantee no_panic

NATIVE CHANGES
  Cargo.lock changed
    reqwest 0.12.4 -> 0.12.8
    rustls 0.23.12 -> 0.23.18
```

### 14.5 Semantic version check

Publishing should verify that version changes match semantic contract changes.

Example policy:

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

## 15. Package Review Metadata

### 15.1 Metadata file

Generated file:

```text
review/package-review.json
```

Schema example:

```json
{
  "schema": "rss.review.package.v1",
  "package": {
    "name": "rss-json",
    "version": "0.1.0"
  },
  "summary": {
    "risk": "low",
    "public_types": 2,
    "public_functions": 4,
    "mutating_apis": 0,
    "retaining_apis": 0,
    "resource_apis": 0,
    "fresh_returning_apis": 2,
    "native_apis": 0,
    "unsafe_apis": 0,
    "unknown_apis": 0
  },
  "features": [],
  "native": {
    "rust": true,
    "build_scripts": false,
    "proc_macros": false,
    "unsafe": false,
    "links": []
  },
  "exports": [
    {
      "name": "Json.parse",
      "kind": "function",
      "classification": "review_if_changed",
      "reasons": ["returns fresh JsonValue", "returns Result"]
    }
  ]
}
```

### 15.2 Metadata generation

Command:

```sh
rss pkg review --emit-metadata
```

Inputs:

```text
.rssi interfaces
.rss source if present
native wrapper declaration
Cargo metadata if native wrapper exists
review policy
```

### 15.3 Metadata trust

Registry-provided metadata is useful for search and preview.

Consumers should verify metadata by checking package hashes and optionally regenerating metadata locally.

Rule:

```text
Metadata is cacheable.
.rssi contract hash is authoritative.
```

---

## 16. Registry Model

### 16.1 Registry is not required by the language core

The package model must work with:

```text
local path dependencies
git dependencies
vendored dependencies
private registries
future public registry
```

A centralized registry is an ecosystem layer, not a language semantics requirement.

### 16.2 Registry responsibilities

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
```

### 16.3 Registry index entry

Example index entry:

```json
{
  "name": "rss-json",
  "version": "0.1.0",
  "checksum": "sha256:...",
  "interface_hash": "sha256:...",
  "review_hash": "sha256:...",
  "risk": "low",
  "native": true,
  "unsafe": false,
  "dependencies": {
    "rss-core": "^0.5"
  }
}
```

### 16.4 Registry UI should be review-oriented

A package page should show:

```text
public API summary
mutating APIs
retaining APIs
resource APIs
native APIs
unsafe APIs
fresh-returning APIs
semantic changes between versions
risk trend
Cargo native dependency summary
```

Not only:

```text
download count
README
version list
```

### 16.5 Private registries

Organizations should be able to run private registries with stricter review policies.

Example policies:

```text
deny unsafe packages
deny build.rs
deny proc macros
allow only approved native wrappers
require package review metadata
require signed packages
```

---

## 17. CLI Design

### 17.1 Top-level commands

Recommended command namespace:

```sh
rss pkg init
rss pkg add <package>
rss pkg remove <package>
rss pkg update [package]
rss pkg check
rss pkg tree
rss pkg review
rss pkg review update
rss pkg metadata
rss pkg publish
rss pkg vendor
rss pkg clean
```

### 17.2 `rss pkg init`

Creates package skeleton.

```sh
rss pkg init rss-json --lib
rss pkg init my-tool --bin
rss pkg init rss-regex --native-rust
```

### 17.3 `rss pkg add`

Adds a dependency.

```sh
rss pkg add rss-json
rss pkg add rss-http@0.4
rss pkg add ../local-package
```

Behavior:

```text
resolve package
add to rsspkg.toml
update rsspkg.lock
show review summary of introduced dependency
```

Example output:

```text
Added rss-http 0.4.0

Review summary:
  risk: elevated
  native APIs: 3
  resource APIs: 1
  retaining APIs: 0

Run `rss review deps` for full dependency review.
```

### 17.4 `rss pkg update`

Updates dependencies.

```sh
rss pkg update
rss pkg update rss-json
```

Should produce semantic summary before applying or after lock update depending on mode.

Useful flags:

```sh
rss pkg update --dry-run
rss pkg update --review
rss pkg update --deny-high-risk
```

### 17.5 `rss pkg check`

Checks package consistency.

```sh
rss pkg check
```

Runs:

```text
manifest validation
interface parse/check
RSScript source check
native binding declaration check
review metadata generation
Cargo metadata scan if native.rust enabled
```

### 17.6 `rss pkg review`

Shows package review map.

```sh
rss pkg review
rss pkg review --json
```

### 17.7 `rss pkg review update`

Compares dependency changes.

```sh
rss pkg review update
rss pkg review update --from rsspkg.lock.old --to rsspkg.lock
```

### 17.8 `rss pkg tree`

Shows dependency graph with risk.

```text
my-app
├── rss-core 0.5.0 [low]
├── rss-json 0.1.0 [low, native]
└── rss-http 0.4.0 [elevated, native, resource]
```

### 17.9 `rss pkg publish`

Validates and publishes package.

Pre-publish checks:

```text
manifest valid
interfaces parse
public APIs explicit
implementation checks
native metadata generated
semantic version check
package review metadata generated
package archive reproducible
```

### 17.10 `rss pkg vendor`

Vendors dependencies locally for offline/reproducible builds.

```sh
rss pkg vendor
```

---

## 18. Build and Check Workflow

### 18.1 Pure RSScript application

```text
rss check
  -> load rsspkg.toml
  -> load dependency .rssi
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

### 18.2 Application with native wrappers

```text
rss run
  -> resolve RSScript package graph
  -> load .rssi contracts
  -> check RSScript source
  -> generate Rust package
  -> include native/rust crates as Cargo path deps
  -> cargo run
  -> remap rustc diagnostics
```

### 18.3 CI workflow

Recommended CI:

```sh
rss pkg check
rss check src/main.rss
rss review --map src/
rss review deps
rss verify-rust src/main.rss
```

Strict mode:

```sh
rss review deps --deny-high-risk --deny-unknown --deny-unsafe
```

---

## 19. Review Policies and Budgets

### 19.1 Project policy

A project may define dependency review policy.

```toml
[review.policy]
deny_unknown = true
deny_unsafe = true
max_high_risk_dependencies = 0
max_native_dependencies = 5
require_lockfile = true
require_review_metadata = true
```

This may live in `rsspkg.toml` or a future `rsspolicy.toml`.

### 19.2 Policy checks

`rss review deps` should fail CI if policy is violated.

Examples:

```text
error: package rss-crypto introduces unsafe native code, but deny_unsafe=true
error: package rss-http is high risk, max_high_risk_dependencies=0
warning: package rss-image uses build.rs; policy requires review
```

### 19.3 Review budget

A review budget gives teams a way to manage dependency risk.

Possible budget dimensions:

```text
number of high-risk dependencies
number of native dependencies
number of retaining APIs imported
number of resource APIs imported
number of unsafe APIs imported
number of unknown APIs
```

---

## 20. Security and Supply Chain

### 20.1 Threat model

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

### 20.2 Checksums

All registry packages should be checksum-verified.

`rsspkg.lock` records package archive checksums and interface/review hashes.

### 20.3 Native build risk

Native Rust code is powerful. The package manager should classify:

```text
build.rs present
proc macros present
unsafe usage detected or declared
links native libraries
uses environment variables in build
downloads code during build
```

Some checks can be static; others require declared metadata and policy enforcement.

### 20.4 Advisory integration

Future registry metadata may include advisories:

```text
security advisory
semantics advisory
native risk advisory
deprecation advisory
malware/yanked version advisory
```

### 20.5 Sandboxing is future work

Build sandboxing is valuable but not part of the MVP.

The MVP should at least surface build-time native execution risk.

---

## 21. Publishing Workflow

### 21.1 Package author flow

```sh
rss pkg init rss-json --native-rust
# edit interface/json.rssi
# edit native/rust/src/lib.rs
rss pkg check
rss pkg review --emit-metadata
rss pkg publish --dry-run
rss pkg publish
```

### 21.2 Publish validation

Registry publish should validate:

```text
rsspkg.toml parse
package name/version valid
interface files parse
public APIs explicit
review metadata generated
native wrapper metadata present if needed
semantic version compatibility
checksums computed
archive reproducible
```

### 21.3 Yank

A registry should support yanking versions.

Yanking should not break existing lockfile builds, but new resolution should avoid yanked versions unless explicitly allowed.

---

## 22. Example: Wrapping `serde_json`

### 22.1 RSScript interface

```rust
// interface/json.rssi

struct JsonValue
struct JsonError

pub fn Json.parse(
    text: read String,
) -> Result<fresh JsonValue, JsonError>

pub fn Json.field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
```

### 22.2 Rust wrapper manifest

```toml
# native/rust/Cargo.toml

[package]
name = "rss_json_native"
version = "0.1.0"
edition = "2024"

[dependencies]
serde_json = "1"
```

### 22.3 Rust wrapper implementation

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

### 22.4 RSScript usage

```rust
let value = Json.parse(text: read body)?
let name = Json.field_string(value: read value, name: read "name")?
```

The reviewer sees the `.rssi` contract, not `serde_json` internals.

---

## 23. Example: HTTP Wrapper Risk

### 23.1 Interface

```rust
struct HttpClient
struct Response
struct HttpError
struct Url

pub fn HttpClient.get(
    client: read HttpClient,
    url: read Url,
) -> Result<Response, HttpError>
    effects(native)

pub fn Response.body_text(
    response: read Response,
) -> Result<String, HttpError>
```

### 23.2 Review metadata

```text
risk: elevated
native APIs: 1
blocking/network APIs: 1
resource APIs: 0
retaining APIs: 0
```

### 23.3 Registry summary

```text
rss-http 0.4.0
  risk: elevated
  native: yes
  unsafe: no
  build.rs: yes via dependency graph
  public APIs: 8
  network APIs: 3
```

This is more useful to reviewers than only seeing that the package depends on `reqwest`.

---

## 24. Workspace Model

### 24.1 Workspace root

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

### 24.2 Workspace resolution

A workspace should share one `rsspkg.lock` by default.

Native Cargo builds may share one generated Cargo workspace.

### 24.3 Path override

Development overrides:

```toml
[patch]
rss-json = { path = "../rss-json" }
```

Patch syntax can be deferred until after local path dependencies work.

---

## 25. Diagnostics

### 25.1 Package diagnostics

Package manager diagnostics should use stable codes eventually.

Classes:

```text
manifest error
dependency resolution error
interface conflict
semantic version mismatch
native wrapper missing binding
native risk policy violation
lockfile mismatch
registry checksum mismatch
Cargo integration failure
unmappable backend diagnostic
```

### 25.2 Source mapping

When generated Rust or native binding failures map to RSScript source, diagnostics should prefer RSScript spans.

Native wrapper compile errors may not always map to RSScript source. In that case, diagnostics should identify the package/native wrapper boundary clearly.

Example:

```text
error: native wrapper `rss-json` failed to compile
  package: rss-json 0.1.0
  native crate: native/rust
  rust diagnostic: ...

This is a native implementation error, not an RSScript source error.
```

### 25.3 Dependency review diagnostics

Example:

```text
error[PKG0401]: dependency update adds retaining API
  package: rss-cache 0.2.0 -> 0.3.0
  function: Cache.put
  change: +effects(retains(value))

This update requires review because values passed to `Cache.put` may now be retained.
```

---

## 26. MVP Plan

### 26.1 MVP 0: Local package format

```text
rsspkg.toml
local path dependencies
interface path loading
rsspkg.lock skeleton
rss pkg check
```

No registry.
No native wrapper automation beyond manual paths.

### 26.2 MVP 1: Interface dependency graph

```text
resolve local dependency interfaces
detect duplicate exported symbols
check source against dependency .rssi
normalize .rssi and compute interface hash
```

### 26.3 MVP 2: Cargo native wrapper integration

```text
native/rust package discovery
generated Cargo package/workspace
path dependency to native wrapper crate
Cargo.lock preservation
native risk scan via cargo metadata
```

### 26.4 MVP 3: Semantic diff

```text
rss pkg review update
compare old/new .rssi
classify semantic changes
produce human and JSON reports
```

### 26.5 MVP 4: Review metadata

```text
review/package-review.json
package risk summary
API classification
native risk summary
CI policy checks
```

### 26.6 MVP 5: Registry protocol

```text
package archive format
index format
checksums
publish dry-run
local/private registry support
```

### 26.7 MVP 6: Public registry

```text
package search
package page with review metadata
semantic diff between versions
advisories
yanking
signing policy
```

---

## 27. Open Questions

1. Should package imports be explicit in RSScript source, or manifest-driven initially?
2. Should the package manager allow multiple major versions of the same package in one graph?
3. How strict should native wrapper ABI checking be in MVP?
4. Should package features be additive like Cargo features?
5. How much Cargo metadata should be surfaced in review output?
6. Should build scripts be denied by default for public registry packages?
7. Should registry metadata be signed independently from package archives?
8. Should review metadata be mandatory for publishing?
9. Should `.rssi` normalization be part of the compiler or package manager?
10. How should async runtime dependencies be represented without leaking Rust runtime details?

---

## 28. Summary

The RSScript package manager should be designed around one sentence:

```text
Cargo builds the implementation; RSScript packages publish reviewable semantic contracts.
```

It should not compete with Cargo for Rust dependency management. It should reuse Cargo wherever Cargo is already excellent.

The distinctive value of RSScript package management is that package dependencies become semantically reviewable:

```text
.rssi defines public contract
rsspkg.lock locks semantic dependency graph
Cargo.lock locks Rust implementation graph
review metadata summarizes package risk
semantic diff explains dependency upgrades
native wrappers expose Rust crates through reviewable APIs
```

This gives RSScript a package ecosystem aligned with its language philosophy:

```text
Script-like source.
System-level boundaries.
Reviewable dependencies.
Rust-powered implementation.
```
