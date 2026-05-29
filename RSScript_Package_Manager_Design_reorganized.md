# RSScript Package Manager Design — Reorganized Draft

Status: Draft / ecosystem design consolidation candidate
Version: 0.3-editorial
Based on: Package Manager Design v0.2
Audience: RSScript compiler implementers, package authors, native binding authors, registry implementers, review-tool authors
Scope: package model, dependency resolution, Cargo integration, semantic package review, native wrappers, registry protocol direction
Non-scope: RSScript language syntax, core language semantics, full registry product design, centralized service policy

---

## 0. Reading Guide and Boundary Rule

RSScript package management is not a second language semantics layer. It consumes the language's `.rssi` contracts and review metadata.

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
```

If this document appears to conflict with the language specification, the language semantic rule wins. This document specifies package artifacts, dependency resolution, Cargo integration, review metadata, and registry behavior.

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
Package = reviewable semantic contract + implementation artifacts.
```

Cargo remains the build substrate for Rust code and Rust dependencies.

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
  rsspkg.lock
  interface loading
  semantic dependency diff
  review metadata
  native boundary classification
  generated Cargo workspace glue
```

A dependency update should answer:

```text
What public contracts changed?
Which APIs now mutate?
Which APIs now retain values?
Which APIs now retain closure captures?
Which APIs now return or manage resources?
Which package features introduced native, unsafe, build, or proc-macro boundaries?
Which dependency changes require human review?
```

---

## 2. Design Thesis

RSScript exists because AI-era software shifts the bottleneck from writing code to reviewing generated code. Package management must follow the same thesis.

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
Can the reviewer inspect the public contract without reading Rust implementation?
```

Therefore package management is part of the RSScript review story.

---

## 3. Goals and Non-goals

### 3.1 Primary goals

```text
provide a package format for RSScript libraries and applications
make .rssi files the public semantic contract of a package
support pure RSScript packages
support Rust crate wrappers behind RSScript interfaces
reuse Cargo for Rust dependency resolution and native compilation
produce deterministic RSScript dependency locks
support semantic diff for package upgrades
generate machine-readable package review metadata
preserve source-map-aware diagnostics through generated Rust packages
allow future registries to publish review summaries and semantic diff history
```

### 3.2 Secondary goals

```text
support local path dependencies before registry dependencies
support workspace development
support vendored/offline builds
support CI review policies
support native risk classification for build scripts, proc macros, linked libraries, FFI, and unsafe code
make package publishing validate semantic interface consistency
```

### 3.3 Non-goals for MVP

```text
full Rust dependency resolver replacement
crates.io replacement in MVP
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

## 4. Package Concepts

### 4.1 RSScript package

An RSScript package is a versioned unit that may contain:

```text
.rssi semantic interface files
.rss implementation files
Rust native wrapper code
rsspkg.toml
rsspkg.lock metadata
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

```text
serde_json crate
  -> rss-json native wrapper
  -> json.rssi
  -> RSScript Json.parse API
```

The raw Rust crate API is not automatically exposed to RSScript. Only `.rssi` APIs are visible.

### 4.5 Review metadata

Review metadata is generated from `.rssi`, RSScript source, native package declarations, and Cargo metadata where applicable.

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
native APIs
unsafe APIs
unknown APIs
package risk level
semantic diff against previous version
```

Review metadata is advisory and machine-readable. The authoritative contract is still the `.rssi` surface plus package checksums.

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

Core packages still expose `.rssi` contracts. Being core does not remove the need for semantic interfaces.

Tool packages may contain binary entry points and stronger native/build risk policies.

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

Generated Rust is an internal build artifact. It is not the RSScript package contract.

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

### 6.2 `[package]`

Required:

```toml
[package]
name = "my-package"
version = "0.1.0"
edition = "2026"
```

Package names should be stable, lowercase, and registry-unique.

### 6.3 `[interfaces]`

Declares where public `.rssi` files live.

```toml
[interfaces]
paths = ["interface"]
exports = ["Json", "JsonValue", "JsonError"]
```

The package manager loads these interfaces for dependents.

### 6.4 `[sources]`

Declares RSScript implementation source roots.

```toml
[sources]
paths = ["src"]
```

A native wrapper package may omit `[sources]` if all implementation is native Rust behind `.rssi` contracts.

### 6.5 `[dependencies]` and `[dev-dependencies]`

Dependencies are RSScript packages, not arbitrary Rust crates.

```toml
[dependencies]
rss-core = "0.5"
rss-json = { version = "0.2", features = ["streaming"] }
my-local = { path = "../my-local" }
my-git = { git = "https://example.org/my-git", rev = "abc123" }
```

Rust crates belong in `native/rust/Cargo.toml`.

### 6.6 `[features]`

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
If a package feature enables native, unsafe, build scripts, proc macros, linked libraries, or FFI, review metadata must report it.
```

### 6.7 `[review]`

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

Computed metadata wins over declared hints.

### 6.8 `[native.rust]`

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

### 6.9 `[workspace]`

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

## 7. `.rssi` Contract and Interface Hashing

### 7.1 Public contract

Every RSScript-facing public API must be declared in `.rssi`.

```rust
struct JsonValue
struct JsonError

features: native

native fn Json.parse(
    text: read String,
) -> Result<fresh JsonValue, JsonError>
    effects(native)

native fn Json.field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
    effects(native)
```

A Rust native wrapper API must be visible as a native boundary in `.rssi`; otherwise review tools cannot distinguish pure RSScript implementation from external Rust implementation.

### 7.2 Contract is semantic, not only type-level

```rust
pub fn Cache.put(
    cache: mut Cache,
    key: read String,
    value: read Image,
) -> Unit
    effects(retains(key), retains(value))
```

This tells the reviewer that `cache` is mutated and both `key` and `value` may be retained.

### 7.3 Implementation must conform

If the implementation is Rust, the wrapper must conform to the `.rssi` contract.

Bad model:

```text
Rust function type determines RSScript effect semantics.
```

Correct model:

```text
.rssi declares RSScript semantics.
Rust wrapper is checked/adapted against that contract where possible.
```

### 7.4 Interface hash

The package manager computes a stable hash of normalized public `.rssi` content.

Included in hash:

```text
type names and kinds
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
```

Not included:

```text
comments
formatting
private implementation files
non-public test interfaces
```

---

## 8. Dependency Graphs, Resolution, and Lockfiles

### 8.1 Graph layers

RSScript package management has three dependency graphs:

```text
1. Semantic package graph
   RSScript packages and .rssi contracts

2. RSScript implementation graph
   .rss source packages and interface imports

3. Native Rust graph
   Cargo crates used by native wrappers
```

The RSScript package manager resolves graphs 1 and 2. Cargo resolves graph 3.

### 8.2 Resolution order

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

Future versions may support multiple major versions with explicit namespace disambiguation.

### 8.4 Feature resolution

RSScript package features should resolve deterministically. Feature unification may follow Cargo-like additive rules, but review metadata must indicate when a feature changes risk.

Example:

```text
rss-http feature native-tls enabled by rss-client
  -> package risk elevated
  -> native dependency graph changed
```

### 8.5 Interface environment

The checker receives an interface environment assembled from:

```text
bundled core interfaces
root package interfaces
resolved dependency interfaces
explicit user-supplied interfaces
```

Duplicate exported symbols, incompatible exports, and ambiguous package roots are diagnostics.

### 8.6 Two lockfiles

RSScript and Cargo have different lock responsibilities.

```text
rsspkg.lock   RSScript semantic dependency lock
Cargo.lock    Rust implementation dependency lock
```

A single lockfile would mix semantic contract resolution with implementation crate resolution.

`rsspkg.lock` records:

```text
resolved RSScript package graph
package archive checksum
interface hash
review metadata hash
native wrapper hash
selected package features
```

`Cargo.lock` records Rust crate resolution. Applications using native wrappers should commit `Cargo.lock` when reproducibility matters.

### 8.7 Update behavior

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
.rssi interface loading
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
  -> check RSScript source against interfaces
  -> lower RSScript to Rust source
  -> generate Cargo package/workspace glue
  -> include native/rust wrapper crates as path dependencies
  -> invoke cargo check/build/run
  -> remap rustc diagnostics through RSScript source maps
```

### 9.3 Native wrapper Cargo.toml

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

### 9.4 Binding manifest

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
```

The binding manifest is part of native review metadata and native hashing.

### 9.5 Type bridge

Common bridge types:

```text
RSScript String     <-> Rust String / &str
RSScript Bytes      <-> Vec<u8> / &[u8]
RSScript Buffer     <-> Vec<u8> / wrapper buffer
RSScript Result     <-> Rust Result
RSScript Option     <-> Rust Option
RSScript resource   <-> Rust type implementing rsscript_runtime::Resource
RSScript class/managed <-> rsscript_runtime::Managed<T>
```

### 9.6 Native risk categories

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

Build scripts and proc macros are native execution boundaries.

Policy values:

```text
forbid   fail if present
review   allow, but mark package high/elevated risk
allow    allow without error, still report in metadata
```

Recommended MVP default:

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

## 10. Package Check and Build Workflow

### 10.1 `rss pkg check`

Canonical command name is `rss pkg`. v0.5 does not define command aliases.

`rss pkg check` runs:

```text
manifest validation
interface parse/check
RSScript source check
implementation-vs-interface conformance check
native binding declaration check
review metadata generation
Cargo metadata scan if native.rust enabled
rsspkg.lock consistency check
```

When `[review] unknown_is_error = true`, any package review result with unknown risk makes package check fail even if the lock is current and there are no frontend errors.

### 10.2 Pure RSScript application

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

### 10.3 Application with native wrappers

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

### 10.4 CI workflow

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

## 11. Semantic Dependency Diff

### 11.1 Purpose

A package update should produce a semantic diff of public contracts.

Commands:

```sh
rss pkg review update
rss review deps
```

### 11.2 Diff inputs

```text
old rsspkg.lock
new rsspkg.lock
old normalized package .rssi contracts
new normalized package .rssi contracts
old review metadata
new review metadata
Cargo.lock changes if native wrappers are present
```

### 11.3 Breaking or must-review changes

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
managed closure capture retention introduced
ResourcePool factory contract changed
ResourcePool factory changed from eager/noescape to retained/lazy
native effect added
unsafe effect added
resource return introduced
resource lifetime behavior changed
constructor/variant field or payload effect changed
handle/weak field marker changed
guarantee removed such as no_panic/noalloc/no_block/pure
unknown classification introduced
```

### 11.4 Review-relevant but possibly compatible changes

```text
new public function added
new public type added
fresh guarantee added
retains effect removed
guarantee added
native implementation changed with unchanged .rssi
Cargo.lock changed for native wrapper package
package risk increased from low to elevated/high
new package feature changes native/build/proc-macro risk
```

### 11.5 Safe or low-risk changes

```text
comments changed
formatting changed
private implementation changed with unchanged interface and no native change
new tests/examples added
review metadata regenerated with no semantic delta
```

### 11.6 Example output

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
```

### 11.7 Semantic version check

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

## 12. Package Review Metadata

### 12.1 Metadata file

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
    "risk": "elevated",
    "public_types": 2,
    "public_functions": 2,
    "mutating_apis": 0,
    "retaining_apis": 0,
    "closure_capture_retaining_apis": 0,
    "resource_apis": 0,
    "fresh_returning_apis": 1,
    "native_apis": 2,
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
      "classification": "must_review",
      "reasons": ["native boundary", "returns fresh JsonValue", "returns Result"]
    }
  ]
}
```

### 12.2 Metadata generation

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

If a `.rssi` contract has frontend errors, those diagnostics are reported as unknown contract exports and counted as unknown APIs, because the public semantic contract cannot be trusted.

If a package has no `.rssi` surface, prototype tooling may fall back to public source declarations for counts and exports, but publishing should eventually require `.rssi` for public packages.

### 12.3 Metadata trust

Registry-provided metadata is useful for search and preview. Consumers should verify metadata by checking package hashes and optionally regenerating metadata locally.

Rule:

```text
Metadata is cacheable.
.rssi contract hash is authoritative.
```

---

## 13. Registry, Publishing, and Security

### 13.1 Registry model

A registry is not required by the language core.

The package model must work with:

```text
local path dependencies
git dependencies
vendored dependencies
private registries
future public registry
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
```

### 13.2 Registry index entry

```json
{
  "name": "rss-json",
  "version": "0.1.0",
  "checksum": "sha256:...",
  "interface_hash": "sha256:...",
  "review_hash": "sha256:...",
  "risk": "elevated",
  "native": true,
  "unsafe": false,
  "dependencies": {
    "rss-core": "^0.5"
  }
}
```

### 13.3 Review-oriented registry UI

A package page should show:

```text
public API summary
mutating APIs
retaining APIs
closure-capture retention APIs
resource APIs
native APIs
unsafe APIs
fresh-returning APIs
semantic changes between versions
risk trend
Cargo native dependency summary
```

### 13.4 Publish validation

`rss pkg publish --dry-run` should validate:

```text
manifest valid
interfaces parse
public APIs explicit
implementation checks
native metadata generated
semantic version check
package review metadata generated
package archive reproducible
unknown package review risk blocks publish readiness
```

Yanking should not break existing lockfile builds, but new resolution should avoid yanked versions unless explicitly allowed.

### 13.5 Security and supply chain

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

All registry packages should be checksum-verified. `rsspkg.lock` records package archive checksums and interface/review hashes.

Sandboxing is future work. MVP should at least surface build-time native execution risk.

---

## 14. CLI Design

Canonical command namespace:

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

No `rss package ...` command is defined for v0.5 tooling.

### 14.1 `rss pkg add`

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

### 14.2 `rss pkg update`

```sh
rss pkg update
rss pkg update rss-json
rss pkg update --dry-run
rss pkg update --review
rss pkg update --deny-high-risk
```

Should produce a semantic summary before applying or after lock update depending on mode.

### 14.3 `rss pkg tree`

Shows dependency graph with risk:

```text
my-app
├── rss-core 0.5.0 [low]
├── rss-json 0.1.0 [elevated, native]
└── rss-http 0.4.0 [elevated, native, resource]
```

### 14.4 `rss pkg vendor`

Vendors dependencies locally for offline/reproducible builds.

```sh
rss pkg vendor
```

For the prototype, local path dependencies can be copied into:

```text
vendor/<name>-<version>/
vendor/rss-vendor.json
```

Registry and git dependencies remain unresolved until their resolvers exist.

---

## 15. Review Policies and Budgets

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

Policy checks should fail CI if violated:

```text
error: package rss-crypto introduces unsafe native code, but deny_unsafe=true
error: package rss-http is high risk, max_high_risk_dependencies=0
warning: package rss-image uses build.rs; policy requires review
```

Budget dimensions:

```text
number of high-risk dependencies
number of native dependencies
number of retaining APIs imported
number of closure-capture-retaining APIs imported
number of resource APIs imported
number of unsafe APIs imported
number of unknown APIs
```

---

## 16. Diagnostics

Package manager diagnostics should use stable codes eventually.

Classes:

```text
manifest error
dependency resolution error
interface conflict
semantic version mismatch
native wrapper missing binding
native binding target mismatch
native risk policy violation
lockfile mismatch
registry checksum mismatch
Cargo integration failure
unmappable backend diagnostic
```

Example:

```text
error[PKG0401]: dependency update adds retaining API
  package: rss-cache 0.2.0 -> 0.3.0
  function: Cache.put
  change: +effects(retains(value))

This update requires review because values passed to Cache.put may now be retained.
```

Native wrapper compile errors may not map to RSScript source. Diagnostics should identify the package/native wrapper boundary clearly:

```text
error: native wrapper `rss-json` failed to compile
  package: rss-json 0.1.0
  native crate: native/rust
  rust diagnostic: ...

This is a native implementation error, not an RSScript source error.
```

---

## 17. Examples

### 17.1 Wrapping `serde_json`

RSScript interface:

```rust
// interface/json.rssi

features: native

struct JsonValue
struct JsonError

native fn Json.parse(
    text: read String,
) -> Result<fresh JsonValue, JsonError>
    effects(native)

native fn Json.field_string(
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

The reviewer sees `.rssi` native boundaries and semantic contracts, not `serde_json` internals.

### 17.2 HTTP wrapper risk

```rust
features: native

struct HttpClient
struct Response
struct HttpError
struct Url

native fn HttpClient.get(
    client: read HttpClient,
    url: read Url,
) -> Result<Response, HttpError>
    effects(native)

pub fn Response.body_text(
    response: read Response,
) -> Result<String, HttpError>
```

Review metadata:

```text
risk: elevated
native APIs: 1
blocking/network APIs: 1
resource APIs: 0
retaining APIs: 0
```

Registry summary:

```text
rss-http 0.4.0
  risk: elevated
  native: yes
  unsafe: no
  build.rs: yes via dependency graph
  public APIs: 8
  network APIs: 3
```

---

## 18. Workspace Model

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

A workspace should share one `rsspkg.lock` by default. Native Cargo builds may share one generated Cargo workspace.

Development overrides may later use:

```toml
[patch]
rss-json = { path = "../rss-json" }
```

Patch syntax can be deferred until after local path dependencies work.

---

## 19. MVP Plan

```text
MVP 0: Local package format
  rsspkg.toml
  local path dependencies
  interface path loading
  rsspkg.lock skeleton
  rss pkg check

MVP 1: Interface dependency graph
  resolve local dependency interfaces
  detect duplicate exported symbols
  check source against dependency .rssi
  normalize .rssi and compute interface hash

MVP 2: Cargo native wrapper integration
  native/rust package discovery
  generated Cargo package/workspace
  path dependency to native wrapper crate
  Cargo.lock preservation
  native risk scan via cargo metadata

MVP 3: Semantic diff
  rss pkg review update
  compare old/new .rssi
  classify semantic changes
  produce human and JSON reports

MVP 4: Review metadata
  review/package-review.json
  package risk summary
  API classification
  native risk summary
  CI policy checks

MVP 5: Registry protocol
  package archive format
  index format
  checksums
  publish dry-run
  local/private registry support

MVP 6: Public registry
  package search
  package page with review metadata
  semantic diff between versions
  advisories
  yanking
  signing policy
```

---

## 20. Open Questions

```text
1. Should package imports be explicit in RSScript source, or manifest-driven initially?
2. Should the package manager allow multiple major versions of the same package in one graph?
3. How strict should native wrapper ABI checking be in MVP?
4. Should package features be additive like Cargo features?
5. How much Cargo metadata should be surfaced in review output?
6. Should build scripts be denied by default for public registry packages?
7. Should registry metadata be signed independently from package archives?
8. Should review metadata be mandatory for publishing?
9. Should .rssi normalization be part of the compiler or package manager?
10. How should async runtime dependencies be represented without leaking Rust runtime details?
11. Should future retained ResourcePool factory signatures use a first-class retained-closure syntax or remain expressed through effects(retains(create))?
12. Should schema v2 remove legacy review category names and emit only `low_semantic_risk`?
```

---

## 21. Summary

The RSScript package manager should be designed around one sentence:

```text
Cargo builds the implementation; RSScript packages publish reviewable semantic contracts.
```

The distinctive value of RSScript package management is that package dependencies become semantically reviewable:

```text
.rssi defines public contract
rsspkg.lock locks semantic dependency graph
Cargo.lock locks Rust implementation graph
review metadata summarizes package risk
semantic diff explains dependency upgrades
native wrappers expose Rust crates through reviewable APIs
unknown risk is classified as unknown, not safe
```

This gives RSScript a package ecosystem aligned with its language philosophy:

```text
Script-like source.
System-level boundaries.
Reviewable dependencies.
Rust-powered implementation.
```
