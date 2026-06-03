# RSScript

**Prototype semantic review evidence for AI-generated code. Turns behavior changes into PR-reviewable evidence for CI experiments.**

```text
AI writes code → RSScript checks semantic boundaries → REIR produces evidence →
CI can gate the PR based on capability, mutation, and deployment evidence.
```

## What it does

```sh
# RSScript checks language and package boundaries.
rss pkg ci my-service --json > package-check.json

# REIR performs system-level capability checks.
reir collect --producer rsscript --package-check package-check.json --out required.reir.json
reir report-pr --required required.reir.json --granted prod-iam.reir.json --target prod
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

RSScript owns language and package-manager checks. REIR owns system evidence,
deployment grants, reconciliation, PR reporting, and CI gate decisions.

The product surface is the whole toolchain, not only the syntax: `check`, `lint`,
`fmt`, `pkg`, LSP/IDE facts, AGENT guidance, constrained generation, review maps,
and doc/test guards are designed as one AI-native feedback loop. A language with
no training-data footprint needs the toolchain to make the intended shape cheap
for agents to generate and cheap for reviewers to inspect.

RSScript and REIR produce review evidence; they are not an execution sandbox.
Core APIs such as `File`, `Env`, `HTTP`, and `Process` can touch the host when a
program runs. Their role in the review pipeline is to emit explicit capability
facts like filesystem access, environment access, network client access, and
process spawn so CI policy can see and gate the boundary.

They are also not formal proof systems. Formal verification is strongest when a
property is precise and local enough to prove against one function, module, or
protocol. PR review has a different throughput problem: thousands of changed
lines may cross source code, packages, native wrappers, deployment manifests,
IAM, and runtime observations. REIR is the evidence layer for that review path.
It records what each producer can support, keeps unknown and best-effort facts
visible, and lets CI compare changed semantic facts instead of re-reading every
raw artifact.

## GitHub Action

```yaml
- uses: Haofei/rsscript/.github/actions/rsscript-review@main
  with:
    head: my-service/
    grants: infra/prod-grants.reir.json
    target: prod
```

The action posts a PR comment with the review decision and exits non-zero on missing capabilities.
For production use, pin this action to a commit SHA. The toolchain and artifact
schemas are still `0.1.x` prototype surfaces.

## Install

From this repository:

```sh
cargo build --release -p rsscript --bin rss
cargo build --release -p reir --bin reir
```

From GitHub while the toolchain is still prototype-grade:

```sh
cargo install --git https://github.com/Haofei/rsscript rsscript --bin rss
cargo install --git https://github.com/Haofei/rsscript reir --bin reir
```

The GitHub Action builds `rss` and `reir` from the checked-out source when they
are not already available on `PATH`. For production CI, pin the action to an
immutable commit SHA rather than a moving branch.

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

This discipline is binding, not aspirational: the language specification opens with a [Constitution](specs/RSScript_v0.6_Spec.md#constitution) of nine articles that govern every design decision — most importantly that constraint is the product, that review-critical behavior must be explicit in the signature, and that a feature is admitted only if it phrases as a reviewer question and needs no implicit rule to be ergonomic. Restraint is anchored to a measurable property (review cost), which is what keeps it from eroding as the language grows.

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
rss review --diff old.rss new.rss
```

answers *what changed* — a function now mutates a parameter, retains a value, lost its `fresh` guarantee, opened a new resource scope, or crossed a native boundary.

For package-level directory changes, use package review/diff commands instead:

```sh
rss pkg diff base-package/ changed-package/
```

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

RSScript owns the review-facing front end: syntax, semantic checks, effects, managed/local/resource rules, diagnostics, review metadata, source mapping, and core/standard-package signatures. Rust owns everything below: codegen, optimization, platform support, linking, the crate ecosystem.

This makes RSScript a review-first source format with a deliberately borrowed backend. The value is in the semantic protocol; the back end is Rust's strongest territory, and RSScript leans into that strength. Module boundaries are tracked in [ARCHITECTURE.md](specs/ARCHITECTURE.md).

---

## Status

Experimental. The current implementation is a Rust-based front-end prototype: lexer, parser, semantic checker with the review-oriented rules, deterministic formatting for the supported AST surface, structured diagnostics, review-map metadata, package review/diff/lock metadata, Rust source lowering with source maps, rustc diagnostic remapping, a small single-isolate runtime crate with `Managed<T>` and `WeakManaged<T>` handles backed by Rust `Rc`/`RefCell`, and core `.rssi` interface signatures parsed through the ordinary interface path. CI gates formatting, lint, tests, and generated-Rust fixtures; golden tests cover lowering and source-map shape.

The v0.6 milestone is a runnable review-first frontend over Rust: strong enough to check real examples, lower them to inspectable Rust, map backend diagnostics back to RSScript source, and keep review risk visible. Self-hosting is a separate experiment, not a v0.6 requirement.

The spec includes a semantic guarantee table that marks each promise as `static`, `dynamic`, `review-only`, or `unsupported`. In short: read/mut/take, retains(local), resource escape, freshness, local move/use, and handle restrictions are frontend obligations; managed alias conflicts are runtime obligations through `Managed<T>`; native/runtime guarantees remain explicit review boundaries unless their signatures carry matching guarantees.

### What's implemented

- **Runtime hooks and async library surface** are split deliberately. The language owns `async fn`, `await`, `task_group`, `select`, and `await for`; the compiler runtime owns the hidden `Pending`/executor substrate; the standard `rss-async` package owns the user-facing async API: `Deadline`, `Timer`, `CancellationSource`/`CancellationToken`, bounded MPSC `Channel`, `Stream`, async file IO, async HTTP client calls, async process IO, TCP client sockets, WebSocket client IO, and CSV/file streaming. Executable packages must select a reviewed backend provider such as `rss-async-runtime` with `[providers] async = "rss-async-runtime"`; single-file scripts keep a default async surface for quick iteration. RSScript's review and effect boundaries stay explicit throughout. A user `async fn` lowers to a value implementing `Pending<Ret>` rather than running an executor inline. Top-level suspension boundaries compose into that pending chain; `if`/`loop`/`match`/`with` statements that contain awaits lower as explicit async statement boundaries, while awaits embedded in ordinary expression arguments remain rejected with `RS0411` until full async expression lowering lands. `task_group { async let ... }` constructs child pendings without running them, drives siblings with one cooperative poll loop, wires structured cancellation through `Task.cancellation_token()`, and drains discarded scoped background tasks. `Channel.bounded<T>` is a bounded MPSC channel with explicit sender/receiver endpoints, async send/recv plus cancellable variants, and `Receiver.into_stream`/`Stream.next` support `await for`. `ChannelError` remains opaque (`ChannelError.message`/`?`) rather than a matchable `Closed`/`Cancelled`/`InvalidCapacity` sum. The remaining core runtime hooks cover `Log`, `Assert`, `Args`, `OS`, and common `List` / `String` / `Json` / `Path` / `Directory` / `File` / `Map` / `Set` / `Buffer` helpers, plus `Toml`, `Yaml`, `Csv`, `Clock`, `Regex`, `Hash`, `TempDir`, `Env`, `Process`, `Random` / `Uuid`, encoding helpers, `Cache`, `Image` / `ImageCache`, HTTP handler/client facades, a DB resource-pool, config and rules reload, interpreter object links, `StringBuilder`, and `Counter`.
- **Compiler-owned derives:** `derives(...)` is a closed set the compiler expands into generated Rust — `Debug`, `Clone`, `Eq`, `Ord`, `Hash`, `JsonEncode`/`JsonDecode` (serde), and the review-only `Schema`/`ReviewSchema` markers. `Eq` and `Ord` are the canonical spellings (Rust's `PartialEq`/`PartialOrd` are not a separate surface). Because the expansion is compiler-owned, the checker validates derive *requirements* before lowering (`RS0211`): `Eq`/`Ord`/`Hash` reject `Float` fields, `handle`/`weak` fields (which lower to `Managed<T>`), `Map`/`Set` fields for `Ord`/`Hash`, and struct/sum fields whose type does not derive the same trait — recursing through `List`/`Option`/`Result` and `Map`/`Set` element types so `Map<String, Float>` is caught for `Eq`. `JsonEncode`/`JsonDecode` require struct/sum fields to derive the matching JSON trait, and `JsonDecode` additionally rejects non-`Eq`/`Hash` `Map` keys and `Set` elements (e.g. a `Float` key). Generic parameters are accepted as ordinary fields (the derive adds the matching `T: Trait` bound) but rejected in a `Map`-key/`Set`-element position at any nesting depth — where the required `Hash` bound cannot be expressed — and a local generic type's arguments are checked too, so `Key<Float>` deriving `Eq` is caught. This `RS0211` check is conservative — it only rejects fields the backend would reject, so it never refuses a program rustc would accept — and it keeps the `Float: Eq` style trait-bound error explained in RSScript instead of leaking from rustc. Separately, `resource` types are move-only RAII values that default to `Debug` only: a distinct policy (`RS0212`) rejects value derives like `Clone`/`Eq`/`Ord`/`Hash`/`JsonEncode`/`JsonDecode` on a resource and allows only the implicit `Debug` and the review-only `Schema`/`ReviewSchema` markers. Unlike `RS0211`, this is a deliberate RSScript ownership rule, so it rejects some derives the Rust backend could itself expand (e.g. `Eq` on a resource with only `Int` fields).
- **Controlled assignment:** `let mut x = e` declares a reassignable local and `x = e` updates it. Assignment is allowed only when the compiler can prove the left side is a legal mutable place: the target's root must be a `let mut` local — a plain `let`/`local` binding is immutable (`RS0311`), a parameter is not a reassignable local (`RS0311`), and the left side must be a place (a local, field, or index) rather than a call result like `get_user().name = ...` (`RS0311`). Field assignment (`obj.field = e`) and `List` index assignment (`list[i] = e`) are executable controlled-assignment forms; other indexed types still require explicit APIs such as `Map.insert` and report `RS0312`. When both sides are known, the assigned value's type must match the place's type (`RS0313`), so an `Int = String` style error is reported in RSScript instead of leaking from rustc. `mut` must appear explicitly in the binding, so mutation stays visible to the type system and review facts. A local reassignment is a behavior fact, not a risk elevation — the review map classifies it like any other local mutation.
- **Lowering basics:** simple operations keep `.rssi` signatures for checking but lower directly to Rust std expressions and runtime hooks. Literals, arithmetic/comparison operators, `Option<T>` constructors, and surface types (`Bytes`, `Buffer`, `Path`, `List<T>`, `Map<K,V>`, `Set<T>`) lower to the matching Rust forms. User-defined operator overloading stays forbidden.
- **Control flow:** `if`, `while`, `loop`, `break`, `continue`, and statement-form `match` for `Option<T>`, `Result<T, E>`, and declared `sum` types. A `match` must cover `Some`/`None`, `Ok`/`Err`, every declared sum variant, or include `_`; non-exhaustive matches are diagnostics before lowering.
- **Modules and protocols:** `module` / `use` declarations are parsed and formatted as large-codebase organization metadata. Protocols are effect-carrying capability contracts, not Rust traits in the source model; calls stay explicit as `Protocol.method(...)` with no auto method resolution.
- **Async:** direct `await`, structured `task_group { async let ... }`, `select`, bounded channels, streams, and `await for` are implemented for the single-isolate cooperative model. Public task handles, async closures, richer async IO packages, and cross-isolate task execution remain future work.
- **Closures:** `noescape Fn()` parameters allow temporary callbacks that may use local values without becoming managed closures.

---

## CLI

```sh
rss check    [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss check    [--json] <package-directory>
rss check    --explain <code>
rss lint     [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss dev      [--once] [--core|--no-core] [--interface <f.rssi> ...] <file-or-package-directory>
rss dev      --lint [--once] [--core|--no-core] [--interface <f.rssi> ...] <file-or-package-directory>
rss dev      --run [--release] [--once] <file-or-package-directory>
rss dev      --json --once [...]   # --json requires --once
rss eval     <file.rss>
rss fmt      <file.rss>
rss review   [--json] --diff <old.rss> <new.rss>
rss review   [--json] --map  <file-or-directory>
rss pkg      [--json] [package-directory]
rss pkg      review [--json] [package-directory]
rss pkg      diff   [--json] <old-package-directory> <new-package-directory>
rss pkg      ci     [--json] [package-directory]
rss pkg      publish --dry-run [--json] [--registry <directory>] [package-directory]
rss pkg      lock     [--json|--reir] [package-directory]
rss pkg      tree     [--json|--reir] [package-directory]
rss pkg      metadata [--verify|--dry-run] [--json|--reir] [package-directory]
rss pkg      vendor   [--dry-run] [--json|--reir] [package-directory]
rss run      [--json] <file-or-package-directory> [--out-dir <directory>] [-- <args>...]
rss test     [--all] [--json] [--filter <substring>]
```

### Command notes

- `rss check` loads bundled core `.rssi` signatures by default for single files; pointed at a directory with `rsspkg.toml`, it runs package check.
- `rss lint` reuses the frontend checks and emits warnings. The first lint is `RSL001` — public signatures over the review budget for parameter count, generics, effects, or nested-type depth.
- `rss dev` is the inner-loop watcher: it reruns `rss check` (add `--lint`, or `--run` for the cargo-backed run path) on every save, polling source modification times with no extra dependency. The default `check` loop never invokes cargo, so frontend feedback stays in the tens-of-milliseconds range; `--once` runs a single pass for scripts and CI. It watches `.rss`, `.rssi`, `.toml`, and `.lock` files, skipping `target/`, `.git/`, and generated directories. To keep behavior unambiguous, `rss dev` rejects flag combinations it would otherwise ignore: `--json` requires `--once` (watch mode interleaves human status lines that would corrupt JSON); `--lint` runs the lint loop alone and is mutually exclusive with `--run`; `--release` applies only to `--run`; and `--run` does not accept `--core`/`--no-core`/`--interface` (those apply only to the check loop).
- `rss eval` runs the checked tree-walk interpreter for pure local feedback. P0 supports scalar/control-flow/user-function/struct/sum-pattern programs plus a small pure runtime-intrinsic subset; unsupported native, host, async, and resource boundaries fail closed instead of falling back to Rust lowering.
- `rss test` runs the default test set; `--all` runs the full test set. `--filter` selects tests by name substring, and `--json` emits a machine-readable summary.
- Human diagnostics render the offending source line in a rustc-style gutter with an aligned caret and inline label (falling back to a caret-only view when the source file is unavailable, e.g. synthetic spans). `--json` output is unchanged.
- `rss review --map` validates inputs first, so files with frontend errors get diagnostics instead of misleading classifications. `--json` reports `unknown_ratio` and `unknown_function_ratio` directly.
- `rss pkg` validates the current package: dependency contracts, implementation/native bindings, package review, semantic lock freshness, and native wrapper metadata. It is the default package health check.
- `rss pkg review` shows the review surface for public contracts, dependencies, mutating/retaining/resource/native/unsafe APIs, and unknown risk.
- `rss pkg diff` compares two local package directories and reports semantic package changes.
- `rss pkg ci` is the CI-facing package check entrypoint. It uses the same package health rules as `rss pkg`, with stable `--json` output for automation.
- `rss pkg publish --dry-run` runs pre-publish checks without uploading and reports whether the package is ready.
- `rss run` lowers a single file (or a package with `src/main.rss`) to a temporary Rust package and delegates to `cargo run`; package lowering carries enabled `[native.rust]` wrappers through as generated Cargo path dependencies and maps `native/bindings.rssbind.toml` call bindings into generated Rust calls. `--dry-run` prints the generated `Cargo.toml`, lowered Rust, and cargo invocation without executing it; `--release` delegates to Cargo's release profile, `--out-dir` keeps the generated package, and arguments after `--` reach the program through the core `Args` API.

### Hello world

```sh
cargo run -- run examples/scripts/basic/hello.rss
```

```rust
fn main() -> Unit {
    Log.write(message: read "hello RSScript")
    return Unit
}
```

Development discipline and the full local verification flow live in [DEVELOPMENT.md](specs/DEVELOPMENT.md): spec prerequisites first, self-hosted validation as the main pressure test, no fixture-only shortcuts, and a broad-first testing loop.

---

## Roadmap

Near term for v0.6 hardening: close remaining static-checker gaps against the spec, keep `.rssi` normalization compiler-owned, tighten package/source/interface consistency checks, expand self-hosted validation that exercises review and package tooling, and keep Rust lowering, source maps, and runtime diagnostics aligned with the documented semantic guarantee table.

Package-management hardening: keep implemented commands documented under their actual `--json` surface, treat dependency updates as review events, preserve unknown risk instead of downgrading it, and land design-only graph-audit/native-ABI/semver workflows only after their underlying interface and native facts are available without weakening review semantics. The package manager itself should be implemented in RSScript as the language core becomes capable enough — package review, dependency-risk classification, semantic lock diffing, and registry metadata shaping are exactly the application-layer systems code RSScript is meant to make reviewable. Any part that still needs Rust should mark the missing RSScript capability clearly instead of growing a parallel Rust-only model.

Longer term: deeper semantic review tooling, a larger core library, agent and runtime examples, stronger optimization paths, optional native ABI adapter checks, graph-level audit-surface reporting, and an experimental self-hosted frontend.

Post-v0.6 design directions (see spec §20.1) build on the single-isolate, non-`Send` managed model, which is what lets RSScript extend async without exposing Rust's `Pin`/`Poll`/`Waker`:

- **Extended async packages.** The implemented `rss-async` package is the base surface; future packages should add custom TLS policy, async queue, semaphore, and broader IO abstractions without exposing Rust's `Future`/`Pin`/`Poll` model to RSScript users. Public task handles, async closures, and cross-isolate task execution remain future work. Read/mut guards may not cross `await`; current `task_group` remains isolate-local structured async, not Rust-style threaded spawning.
- **Cross-isolate messaging with zero-copy transfer.** Explicit typed channels between isolates; `take`-based moves are the zero-copy transfer path, with single ownership enforced at compile time. Managed handles never cross isolates — only explicit messages do.
- **Two-tier execution.** A HIR-level interpreter for the managed subset gives a fast edit-run loop; Rust lowering stays the production/AOT path. Both observe identical semantics and diagnostics.
- **Structured-fix tooling.** An `rss fix` command applying machine-applicable fixes, plus an analysis server streaming diagnostics and fixes to both human editors and AI repair agents.
- **Sum type hardening.** Current `sum` declarations are closed and exhaustively checked before lowering; future work should strengthen package/interface metadata without importing Rust enum complexity.
- **Registry review-risk badges.** The package registry surfaces review-risk signals (native, unsafe, unknown, mutating/retaining ratios) as first-class quality badges, reusing existing package review metadata.

These intentionally exclude Dart-style conveniences that conflict with review-first semantics: cascade (`..`), extension methods / implicit method resolution, and positional records / implicit flow promotion.

---

## Non-goals

RSScript prioritizes reviewable semantics over syntactic cleverness or maximal expressiveness. It deliberately avoids implicit conversions, user-defined operator overloading, hidden allocation, hidden retention, macro-heavy metaprogramming, complex public signatures, Rust-style lifetime syntax, C++-style implicit magic, and TypeScript-style type gymnastics.

RSScript also does not own a separate build graph or build executor. `rss run` lowers to a Rust package and delegates execution to Cargo; fast edit-run feedback belongs to the HIR interpreter, while Cargo remains the Rust build substrate.

RSScript deliberately holds the lower-to-Rust niche. It is not trying to be a
multi-target/full-stack language that also emits JavaScript, mobile UI code, or
database schemas. Breadth would dilute the review-first bet; Rust lowering is
the backend contract.

The goal is code humans and tools can review reliably.

---

## License

Dual-licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
