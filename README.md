# RSScript

**Semantic review evidence for AI-generated code. Turns behavior changes into PR-reviewable, CI-gateable proof.**

```text
AI writes code → RSScript checks semantic boundaries → REIR produces evidence →
CI blocks/approves the PR based on capability, mutation, and deployment proof.
```

## What it does in one command

```sh
# Does the PR's code require capabilities the deployment doesn't grant?
rss review-pr --head my-service/ --grants prod-iam.reir.json --format markdown
```

Output:

```markdown
## RSScript / REIR deployment review

Status: FAIL

### Required capabilities needing deployment grant

- subject: Reports.cleanup_old_reports
  capability: object_storage.delete aws/s3 s3:DeleteObject arn:aws:s3:::reports-prod/*
  evidence: src/upload.rss:28 Reports.cleanup_old_reports -> S3.delete_object

### Missing capabilities

- s3:DeleteObject on arn:aws:s3:::reports-prod/*

### Review decision

Block this PR before deploy.
```

Formats: `markdown` (PR comment), `ci-json` (stable schema for CI gates), `sarif` (GitHub code scanning).

## GitHub Action

```yaml
- uses: Haofei/rsscript/.github/actions/rsscript-review@v0
  with:
    head: my-service/
    grants: infra/prod-grants.reir.json
    target: prod
```

The action posts a PR comment with the review decision and exits non-zero on missing capabilities.

---

## Why this exists

AI makes writing code cheap. Reviewing AI-generated code is now the bottleneck.

RSScript is a **constrained source format** and **semantic review protocol** for AI-generated systems code, backed by Rust execution. It pushes mutation, retention, resource ownership, native/unsafe boundaries, and external capabilities into the signature — where review can see them and CI can gate on them.

The product core is not the language. It's the **evidence pipeline**:

```text
.rss source → compiler semantic checks → review map + REIR bundle →
  capability reconciliation against deployment grants →
  PR comment / CI gate / SARIF report
```

RSScript is the constrained AI codegen target that makes these artifacts reliable. REIR (Review Evidence IR) is the cross-layer evidence format that connects source analysis to deployment reality.

---

## Try the review demo

PR review story: an AI-style patch adds `Reports.cleanup_old_reports -> S3.delete_object`, but the existing prod IAM role grants only `s3:PutObject`. RSScript package review turns that new external ability into a REIR fact, and deployment reconciliation blocks the PR before deploy.

```sh
cargo test --test s3_iam_reir_demo_e2e s3_iam_reir_demo_pr_review -- --nocapture
```

Expected output:

```text
s3 iam pr review: blocked missing=s3:DeleteObject evidence=src/upload.rss:28
```

Fast preflight: RSScript code requires an S3 capability, Terraform/OpenTofu IAM policy grants are reconciled before deploy.

```sh
cargo test --test s3_iam_reir_demo_e2e s3_iam_reir_demo_preflight -- --nocapture
```

Expected output:

```text
s3 iam preflight: missing=s3:PutObject fixed=covered excess=s3:DeleteObject
```

Reviewer scenario matrix:

```sh
cargo test --test s3_iam_reir_demo_e2e s3_iam_reir_demo_scenarios -- --nocapture
```

Release/demo runtime path, including Tokio-backed native async IO and sync comparison:

```sh
cargo test --test s3_iam_reir_demo_e2e s3_iam_reir_demo_fails_preflight_then_passes_and_shows_async_io_gain -- --ignored --nocapture
```

The demo lives in [`demos/s3-iam-reir`](demos/s3-iam-reir): RSScript source -> package capability binding -> REIR required facts -> Terraform/OpenTofu IAM grants plus runtime grants -> missing/fixed/excess/code-change/native-risk/missing-binding review outcomes.

---

## Status

```text
Toolchain crate version: 0.1.x
Language spec target: v0.6
Artifact schemas: unstable unless explicitly marked
```

Stable enough for demos:

- diagnostics JSON
- review map JSON
- package review REIR output

Not stable:

- RSS syntax
- REIR ontology details
- package registry metadata

---

## Why a Review Protocol

The obvious alternative is proc-macros, a Clippy ruleset, and a review tool over Rust directly. The problem is that **Rust's signatures themselves are part of the review cost**. `Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>` carries four bits a reviewer needs and a dozen bits that exist to satisfy the type system. A smaller source language changes the surface itself, instead of asking tools to recover intent afterward. And AI, trained on every clever Rust crate on GitHub, is gradient-descending into that surface every time it generates code.

A smaller front end fixes two things at once:

- **The signature** a human reads becomes shorter and load-bearing in different ways.
- **The AI's option space** shrinks. RSScript gives the generator fewer complex shapes to reach for. Constraint is the product.

The product core is the review protocol: `.rssi` semantic contracts, structured diagnostics, source-mapped backend errors, review maps, and semantic diffs. The source language exists to make those review artifacts reliable and cheap to compute. Rust is the backend target and ecosystem substrate.

Before AI, writing code was expensive and reviewing was manageable. That ratio has flipped: generating is cheap, reviewing is the bottleneck. RSScript is designed for the new ratio: AI writes, the compiler checks semantic boundaries, humans focus on the *risk*. What mutates, what gets retained, who owns a resource, where you cross into native or unsafe, what changed in a public API — all of it lives in the signature and in machine-readable diagnostics.

This discipline is binding, not aspirational: the language specification opens with a [Constitution](RSScript_v0.6_Spec.md#constitution) of seven articles that govern every design decision — most importantly that constraint is the product, that review-critical behavior must be explicit in the signature, and that a feature is admitted only if it phrases as a reviewer question and needs no implicit rule to be ergonomic. Restraint is anchored to a measurable property (review cost), which is what keeps it from eroding as the language grows.

---

## Scope

RSScript targets **application-level systems**: backend services, agent runtimes, data processing tools, internal infrastructure, glue code that needs to be fast and correct while keeping review cost low. Rust remains the right tool for kernels, drivers, embedded firmware, compiler internals, and code that benefits from its full expressivity.

---

## The model, in three layers

The default writing experience is `let` everywhere, named arguments, regular type annotations. `local`, `with`, and `effects(retains)` are tools you reach for when something specific is true — a hot loop, a resource handle, an actually-retained value — not decisions you make on every line. Most code only touches the first layer.

### Managed by default

Most code should look ordinary:

```rust
let user = User.load(id: read user_id)
let response = Response.ok(body: read user)
```

Managed values are easy to share, store, and drop into long-lived graphs. This is the default for business logic, agent memory, configuration, caches, ASTs, request/response objects — the broad layer outside hot paths.

Under the hood, the v0.6 managed runtime is single-isolate reference counting — think Swift's ARC inside one isolate — and uses Rust `Rc`/`RefCell`-like primitives internally. Managed handles are intentionally not `Send` or `Sync`; they do not cross Rust threads or RSScript isolates. Managed `class` and `struct` values have no user-observable destructor; deterministic cleanup is expressed only through `resource`, `with`, and `ResourcePool`, which are orthogonal to how managed memory is reclaimed. Because managed objects expose no user-visible finalization order, reference counting versus a future tracing collector is a backend decision rather than a language guarantee — a later major version could swap it without changing observable semantics. For v0.6 the tradeoffs are the usual refcounting ones: a per-access dynamic borrow check, and reference cycles do not collect on their own, so they are broken with a `weak` keyword the same way Swift does it. Future cross-isolate or cross-thread transfer should be explicit message passing, not implicit shared heap access.

That per-access cost is relative to native Rust, not to other managed languages. Primitives (`Int`, `Bool`, `Float`, etc.) stay on the stack; refcount only touches heap objects you actually share; there's no GIL, no interpreter loop, no per-object dict header. Managed-only RSScript still lowers to monomorphic Rust through LLVM — typically an order of magnitude faster than Python without ever opting into `local`. `local` is for when you want to compete with hand-tuned Rust, not for routine performance.

### Local when it matters

Hot paths can opt in to local exclusive values, and the checker protects those values from silent retention by managed objects or managed closures:

```rust
features: local

fn fill_scratch(path: read Path) -> Result<Unit, FileError> {
    local scratch = Buffer.new(size: 4096)

    with File.open(path: read path)? as file {
        File.read_into(file: mut file, buffer: mut scratch)?
    }

    return Ok(Unit)
}
```

That gives you a clear performance path for parser buffers, JSON decoding, prompt buffers, image preprocessing, and the like — without leaking that complexity into the rest of the program.

The capability model is a gradual descent: managed is the default reviewable application layer; `features: local` opens explicit exclusive ownership for hot paths and resource-heavy internals; `features: native` crosses into Rust wrapper code and carries a higher review burden. `unsafe` is separate: it is not the next normal layer after native, but an explicit hazard marker for code that may violate RSScript's safety model. `features: native, unsafe` is therefore a native boundary with an additional unsafe review obligation.

The safe RSScript surface is designed to have no user-visible undefined behavior. Managed aliasing and mutation conflicts are runtime errors or diagnostics, not hidden memory hazards, and the compiler/runtime crates forbid Rust `unsafe` internally. `features: native` and `features: unsafe` are explicit review boundaries for code outside that safe surface.

### Features are review signals

Files are managed-only unless they declare otherwise:

```rust
features: local
```

`local`, `native`, `unsafe`, and `async` are recognized as review capability gates today. `local` enables local ownership features; `native`, `unsafe`, and `async` must be declared before a file can expose those boundaries. Bodyless `native fn` declarations are native-wrapper bindings; executable `native fn` bodies remain outside the v0.6 runtime and are reported before Rust lowering. `async fn` bodies support the restricted v0.6 executable MVP: direct `await` inside an async function, isolate-local `task_group { async let ... }`, single-isolate cooperative runtime polling, and no public `Future`/`Waker` surface. Unstructured `spawn`, streams, channels, async closures, and public task handles remain future work. Runtime-only scheduler hooks for native pending operations are implementation ABI. The reference runtime can host Tokio-backed Rust IO futures behind that ABI, so high-concurrency native IO does not leak Tokio, `Future`, `Pin`, `Poll`, or `Waker` into RSScript source. `device`, `ffi`, and `reflection` are reserved review markers: they raise review risk when present, but they do not unlock syntax, lowering, or runtime behavior in v0.6. Ordinary libraries (JSON, File, Image, HTTP, Map, Regex) stay as libraries. Repeated or unknown names are diagnostics so capability boundaries stay explicit.

---

## Calls are explicit at the boundary

Function calls use named arguments and visible effects:

```rust
Cache.put(
    cache: mut cache,
    key: read key,
    value: read image,
)
```

If a function keeps a value after it returns, it has to say so:

```rust
fn put(
    cache: mut Cache,
    key: read String,
    value: read Image,
) -> Unit
    effects(retains(key), retains(value))
```

A reviewer reads that signature and immediately knows: `cache` is modified, `key` and `value` are read and held on to. No source dive required.

The core vocabulary stays small on purpose:

- `let` — default managed value
- `local` — local exclusive value
- `with` — scoped resource lifetime
- `manage` — move a local value into the managed runtime
- `read` / `mut` / `take` — inspected / modified / consumed
- `fresh` / `retains` — newly created / may be held after return

Each maps directly to a question a reviewer is going to ask anyway: *what mutates? what gets retained? what resource opens, what closes? where does local data enter managed state? what public behavior changed?*

There's a useful asymmetry built in here. At a call site you just follow the function's signature — `read x`, `mut y` — you don't decide anything new. The decisions live at the function definition, written once and read by every caller. Writer-side cost stays small; reader-side gain is large. The same logic explains why managed is the default: daily writing stays close to Python's ergonomics, while every signature carries enough structure that review doesn't have to dig into bodies.

### A small example

```rust
features: local

fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    local buffer = Buffer.new(size: 8192)

    with File.open_read(path: read input)? as reader {
        with File.open_write(path: read output)? as writer {
            while File.read_into(file: mut reader, buffer: mut buffer)? {
                File.write(file: mut writer, data: read buffer)?
                Buffer.clear(buffer: mut buffer)
            }
        }
    }

    return Ok(Unit)
}
```

`input` and `output` are read-only paths; `buffer` is local scratch; `reader` and `writer` are scoped resources that close at the end of their `with` blocks. Nothing about ownership or retention is implicit.

### Library shape

The rule of thumb for libraries: **managed at the surface, local in the engine, reviewable at the boundary.**

User-facing APIs stay simple:

```rust
let json = Json.parse(text: read body)?
```

Internals can use local scratch and `*_into` forms for low-allocation paths, while the public surface stays managed and reviewable.

---

## Semantic review

Review is meant to be semantic and stronger than textual diff.

```sh
rss review --diff base/ changed/
```

answers *what changed* — a function now mutates a parameter, retains a value, lost its `fresh` guarantee, opened a new resource scope, or crossed a native boundary.

```sh
rss review --map generated.rss
```

answers *what do I actually need to read?* For a 400-line AI-generated file, the reviewer sees a map first:

```text
FILE FEATURES
  local, native      risk high

ENTRY POINTS
  run_agent

REVIEW REQUIRED
  update_cache      mutates shared state, retains(value)
  save_report       resource boundary
  charge_card       native boundary

FOLDABLE
  private pure helpers, no retention, no resources
```

RSScript moves human review up a level.

---

## How it compiles

RSScript builds on Rust's existing backend and ecosystem:

```text
RSScript source
  → parser / checker / review metadata
  → generated Rust source
  → rustc
  → executable or library
```

RSScript owns the review-facing front end: syntax, semantic checks, effects, managed/local/resource rules, diagnostics, review metadata, source mapping, core signatures. Rust owns everything below: codegen, optimization, platform support, linking, the crate ecosystem.

This makes RSScript a review-first source format with a deliberately borrowed backend. The value is in the semantic protocol; the back end is Rust's strongest territory, and RSScript leans into that strength. Module boundaries are tracked in [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Status

Experimental. The current implementation is a Rust-based front-end prototype: lexer, parser, semantic checker with the review-oriented rules, deterministic formatting for the supported AST surface, structured diagnostics, review-map metadata, package review/diff/lock metadata, Rust source lowering with source maps, rustc diagnostic remapping, a small single-isolate runtime crate with `Managed<T>` and `WeakManaged<T>` handles backed by Rust `Rc`/`RefCell`, and core `.rssi` interface signatures parsed through the ordinary interface path. CI gates formatting, lint, tests, and generated-Rust fixtures; golden tests cover lowering and source-map shape.

The v0.6 milestone is a runnable review-first frontend over Rust: strong enough to check real examples, lower them to inspectable Rust, map backend diagnostics back to RSScript source, and keep review risk visible. Self-hosting is a separate experiment, not a v0.6 requirement.

The spec includes a semantic guarantee table that marks each promise as `static`, `dynamic`, `review-only`, or `unsupported`. In short: read/mut/take, retains(local), resource escape, freshness, local move/use, and handle restrictions are frontend obligations; managed alias conflicts are runtime obligations through `Managed<T>`; native/runtime guarantees remain explicit review boundaries unless their signatures carry matching guarantees.

### What's implemented

- **Runtime hooks** span the core library surface: `Log`, `Assert`, `Args`, `OS`, and the common `List` / `String` / `Json` / `Path` / `Directory` / `File` / `Map` / `Set` / `Buffer` helpers, plus `Toml`, `Csv`, `Clock`, `Regex`, `Hash`, `TempDir`, `Env`, `Process`, `Random` / `Uuid`, encoding helpers, `Cache`, `Image` / `ImageCache`, HTTP handler/client facades, a DB resource-pool, config and rules reload, interpreter object links, `StringBuilder`, and `Counter`. The HTTP client facade lowers to review-visible runtime hooks; the safe runtime returns a structured "not configured" error unless a package supplies an audited native client. `ImageCache` is the first retained managed-container hook; the interpreter hooks model `Environment` and `FunctionObject` as managed handles with the closure link stored weakly.
- **Lowering basics:** simple operations keep `.rssi` signatures for checking but lower directly to Rust std expressions and runtime hooks. Literals, arithmetic/comparison operators, `Option<T>` constructors, and surface types (`Bytes`, `Buffer`, `Path`, `List<T>`, `Map<K,V>`, `Set<T>`) lower to the matching Rust forms. User-defined operator overloading stays forbidden.
- **Control flow:** `if`, `while`, `loop`, `break`, `continue`, and statement-form `match` for `Option<T>`, `Result<T, E>`, and declared `sum` types. A `match` must cover `Some`/`None`, `Ok`/`Err`, every declared sum variant, or include `_`; non-exhaustive matches are diagnostics before lowering.
- **Modules and protocols:** `module` / `use` declarations are parsed and formatted as large-codebase organization metadata. Protocols are effect-carrying capability contracts, not Rust traits in the source model; calls stay explicit as `Protocol.method(...)` with no auto method resolution.
- **Async MVP:** direct `await` works inside `async fn`, and structured `task_group { async let ... }` is accepted for isolate-local child operations. `spawn`, streams, channels, async closures, and public task handles remain future work.
- **Closures:** `noescape Fn()` parameters allow temporary callbacks that may use local values without becoming managed closures.

---

## CLI

```sh
rss check    [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss check    [--json] <package-directory>
rss check    --explain <code>
rss lint     [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss fmt      <file.rss>
rss review   [--json] --diff <old.rss> <new.rss>
rss review   [--json] --map  <file-or-directory>
rss pkg      check  [--json|--reir] [package-directory]
rss pkg      review [--json|--reir] [package-directory]
rss pkg      review update [--json|--reir] --from <old-rsspkg.lock> --to <new-rsspkg.lock>
rss pkg      lock   [--json|--reir] <package-directory>
rss pkg      tree   [--json|--reir] [package-directory]
rss pkg      publish --dry-run [--json|--reir] [--registry <directory>] [package-directory]
rss pkg      vendor [--dry-run] [--json|--reir] [package-directory]
rss pkg      metadata [--dry-run|--verify] [--json|--reir] [package-directory]
rss pkg      diff   [--json|--reir] <old-package-directory> <new-package-directory>
rss pkg      reir diff [--json] [--fail-on-change] --from <baseline-reir.json> --to <current-reir.json>
rss lower    --rust  <file.rss> [--out-dir <directory>]
rss run      [--json] <file-or-package-directory> [--out-dir <directory>] [-- <args>...]
rss remap-rustc  [--json] <rsscript-source-map.json> <rustc-json-lines>
rss verify-rust  [--json] <file-or-package-directory> [--out-dir <directory>]
```

### Command notes

- `rss check` loads bundled core `.rssi` signatures by default for single files; pointed at a directory with `rsspkg.toml`, it runs package check.
- `rss lint` reuses the frontend checks and emits warnings. The first lint is `RSL001` — public signatures over the review budget for parameter count, generics, effects, or nested-type depth.
- `rss review --map` validates inputs first, so files with frontend errors get diagnostics instead of misleading classifications. `--json` reports `unknown_ratio` and `unknown_function_ratio` directly.
- `rss pkg check` validates a local package: loads local path dependency `.rssi` contracts, checks package `.rssi` contracts against implementations or native bindings, rejects unresolved or conflicting dependency graphs, runs package review, compares the semantic lock against `rsspkg.lock`, and scans enabled native Rust wrappers with Cargo metadata. `[review.policy] deny_unknown = true` makes unknown review risk fail the check. `--reir` emits the CI gate status, graph/lock/native check facts, lock-change facts, native unsafe/build-time facts, and diagnostics as a REIR bundle.
- `rss pkg review` treats `.rssi` files as the public semantic contract and summarizes public type/function/API counts plus direct dependency identities, mutating, retaining, resource, fresh-returning, native, unsafe, and unknown APIs, with per-export classifications. Frontend errors in `.rssi` contracts count as unknown exports. `--reir` emits package risk facts, direct dependency facts/edges, native boundary facts, and capability facts as a REIR bundle so it can feed `reir show`, `reir diff`, `reir merge`, `reir slice`, and bundle-mode `reir reconcile` directly.
- `rss pkg review update` compares two `rsspkg.lock` files and reports version, source, checksum, `.rssi` interface, review metadata, native wrapper, and feature-selection changes. `--reir` emits update-risk, package-risk, and changed-field facts with `lockfile_entry` evidence.
- `rss pkg lock` emits semantic lock metadata for the root package and local path dependency graph, with SHA-256 hashes for public `.rssi` contracts, review metadata, package contents, and native Rust wrapper contents when enabled. `--reir` emits those lockfile hashes as REIR `supply_chain` facts with `lockfile_entry` evidence.
- `rss pkg tree` shows the dependency graph with review risk. Local path dependencies expand recursively; unresolved registry dependencies are unknown; git dependency sources are rejected with a stable unsupported-source diagnostic. `--reir` emits transitive dependency-risk facts, effective-interface hash facts, and `depends_on` edges with `dependency_path` evidence.
- `rss pkg publish --dry-run` runs pre-publish checks without uploading: package consistency, dependency review, semver shape, review-risk classification, native metadata, a reproducible archive manifest with per-file checksums, and a registry index entry. Unknown review risk blocks publish readiness. `--registry <directory>` reports the index and archive-manifest paths that would be written. `--reir` emits registry/archive supply-chain facts and publish check results with `registry_metadata` evidence.
- `rss pkg vendor` copies local path dependencies into `vendor/<name>-<version>/` and writes `vendor/rss-vendor.json`; unresolved registry dependencies stay unknown, git sources stay unsupported. `--reir` emits vendored checksum `supply_chain` facts and unresolved dependency-risk facts with `package_metadata` evidence.
- `rss pkg metadata` writes `review/package-review.json` and `review/reir/rsscript.json` from the local package review result; `--dry-run` reports both paths without writing, and `--verify` recomputes both artifacts and fails if committed metadata is missing or stale. Unknown review risk is preserved and makes the result not ok. `--reir` emits metadata status, artifact, and mismatch facts with `package_metadata` evidence so CI can merge stale-or-missing artifact results with other REIR bundles.
- `rss pkg diff` compares two local package directories and reports version, RSScript dependency, feature, native Rust wrapper, and public `.rssi` contract changes. `--reir` emits a `reir.diff.v0.1` JSON diff over the REIR bundles derived from each package review.
- `rss pkg reir diff` compares already-generated REIR bundle artifacts, so CI can diff a locked baseline against `review/reir/rsscript.json` without re-running package review for the baseline package. Add `--fail-on-change` when the CI gate should return non-zero for any semantic REIR diff item.
- `reir merge` combines REIR bundle artifacts for cross-repo or cross-producer review, rejects schema/ontology mismatches, dedupes stable ids, rebuilds the subject index, and recomputes derived review slices.
- `reir collect --producer rsscript` converts existing RSScript JSON artifacts into REIR. Besides `--review-map` and `--package-review`, it accepts package-manager JSON artifacts from `--package-check`, `--package-lock`, `--lock-update`, `--package-tree`, `--package-publish`, `--package-metadata`, and `--package-vendor`, then merges them into one deduped bundle.
- `reir reconcile <bundle.json> --target <name> --out <reconciled.json>` reconciles required and granted facts from one merged bundle, records the target name on each reconciliation result, writes those results back into the bundle, and recomputes slices for review. The older `--required required.json --granted granted.json --target <name>` form emits the same target field without writing a merged bundle.
- `reir slice --bundle <bundle.json> --kind <slice-kind>` recomputes review slices from a bundle and can filter any implemented slice kind, using either short names such as `package_risk` or full schema names such as `package_risk_slice`.
- `rss run` lowers a single file (or a package with `src/main.rss`) to a temporary Rust package and delegates to `cargo run`; package lowering carries enabled `[native.rust]` wrappers through as generated Cargo path dependencies and maps `native/bindings.rssbind.toml` call bindings into generated Rust calls. `--release` delegates to Cargo's release profile, `--out-dir` keeps the generated package, and arguments after `--` reach the program through the core `Args` API.
- `rss verify-rust --out-dir` keeps the generated package and source map so unmappable rustc diagnostics can be inspected against the actual generated Rust.
- `rss bbom` is an experimental behavior BOM command for capability summaries, deltas, and policy checks over RSScript source.

### Hello world

```sh
cargo run -- run examples/hello.rss
```

```rust
fn main() -> Unit {
    Log.write(message: read "hello RSScript")
    return Unit
}
```

Development discipline and the full local verification flow live in [DEVELOPMENT.md](DEVELOPMENT.md): spec prerequisites first, self-hosted validation as the main pressure test, no fixture-only shortcuts, and a broad-first testing loop.

---

## Roadmap

Near term for v0.6 hardening: close remaining static-checker gaps against the spec, keep `.rssi` normalization compiler-owned, tighten package/source/interface consistency checks, expand self-hosted validation that exercises review and package tooling, and keep Rust lowering, source maps, and runtime diagnostics aligned with the documented semantic guarantee table.

Package-management hardening: keep implemented commands documented under their actual `--json` surface, treat dependency updates as review events, preserve unknown risk instead of downgrading it, and land design-only graph-audit/native-ABI/semver workflows only after their underlying interface and native facts are available without weakening review semantics. The package manager itself should be implemented in RSScript as the language core becomes capable enough — package review, dependency-risk classification, semantic lock diffing, and registry metadata shaping are exactly the application-layer systems code RSScript is meant to make reviewable. Any part that still needs Rust should mark the missing RSScript capability clearly instead of growing a parallel Rust-only model.

Longer term: deeper semantic review tooling, a larger core library, agent and runtime examples, stronger optimization paths, optional native ABI adapter checks, graph-level audit-surface reporting, and an experimental self-hosted frontend.

Post-v0.6 design directions (see spec §20.1) build on the single-isolate, non-`Send` managed model, which is what lets RSScript extend async without exposing Rust's `Pin`/`Poll`/`Waker`:

- **Extended async.** Unstructured `spawn`, async closures, optional isolate-local task handles, and a `Stream<T>` / `await for` async-sequence form. Read/mut guards may not cross `await`; current `task_group` remains isolate-local structured async, not Rust-style threaded spawning.
- **Cross-isolate messaging with zero-copy transfer.** Explicit typed channels between isolates; `take`-based moves are the zero-copy transfer path, with single ownership enforced at compile time. Managed handles never cross isolates — only explicit messages do.
- **Two-tier execution.** A HIR-level interpreter for the managed subset gives a fast edit-run loop; Rust lowering stays the production/AOT path. Both observe identical semantics and diagnostics.
- **Structured-fix tooling.** An `rss fix` command applying machine-applicable fixes, plus an analysis server streaming diagnostics and fixes to both human editors and AI repair agents.
- **Sum type hardening.** Current `sum` declarations are closed and exhaustively checked before lowering; future work should strengthen package/interface metadata without importing Rust enum complexity.
- **Registry review-risk badges.** The package registry surfaces review-risk signals (native, unsafe, unknown, mutating/retaining ratios) as first-class quality badges, reusing existing package review metadata.

These intentionally exclude Dart-style conveniences that conflict with review-first semantics: cascade (`..`), extension methods / implicit method resolution, and positional records / implicit flow promotion.

---

## Non-goals

RSScript prioritizes reviewable semantics over syntactic cleverness or maximal expressiveness. It deliberately avoids implicit conversions, user-defined operator overloading, hidden allocation, hidden retention, macro-heavy metaprogramming, complex public signatures, Rust-style lifetime syntax, C++-style implicit magic, and TypeScript-style type gymnastics.

The goal is code humans and tools can review reliably.

---

## License

Dual-licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
