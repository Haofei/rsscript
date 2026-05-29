# RSScript

**A constrained, review-first source format for AI-generated systems code with Rust-backed execution.**

```text
AI codegen target.
Semantic review protocol.
Rust lowering backend.
```

This came out of reviewing 100k+ lines of AI-generated Rust over six months. The same shapes kept hurting: `Arc<Mutex<HashMap<...>>>` stacked four deep, signatures with eight trait bounds where one would do, `Pin<Box<dyn Future<...>>>` blocking the view of what a function actually does, retention buried three call levels down, four hundred lines of correct-but-dense code in a single PR where the important ten percent was hard to find. The Rust compiler accepted it. Review still took too much human attention.

There's also a long-standing wishlist for managed-by-default app code, explicit performance escapes, and a direct path back to native systems work. The request shows up in `/r/rust` threads and language-design posts regularly. AI review pain is what finally made the cost/benefit click for me.

RSScript is an AI codegen target and semantic review protocol that lowers to Rust. It keeps rustc, Cargo, and the crate ecosystem; it compresses the source surface so a reviewer's first read costs less, and pushes mutation, retention, resources, and native boundaries into the signature where review can see them. Advanced Rust remains available through `features: native` as an explicit review boundary.

Rust is excellent at library boundaries: generic abstractions, precise ownership, trait-driven APIs, async runtimes, and zero-cost escape hatches. Application code usually wants a different register: concrete data, direct control flow, visible mutation, visible retention, and few abstraction choices. RSScript makes that application register the default surface for AI-generated code, while keeping Rust as the place for library implementation and native wrapper work.

That distinction matters more with AI in the loop. When a model writes Rust application code, it often reaches for library-author patterns: broad generics, layered traits, deeply nested shared state, and future-proof abstractions before the app needs them. RSScript narrows the default shape toward app-developer code, then uses `features: local` and `features: native` when the implementation really needs to cross into lower-level Rust.

---

## Why a Review Protocol

The obvious alternative is proc-macros, a Clippy ruleset, and a review tool over Rust directly. The problem is that **Rust's signatures themselves are part of the review cost**. `Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>` carries four bits a reviewer needs and a dozen bits that exist to satisfy the type system. A smaller source language changes the surface itself, instead of asking tools to recover intent afterward. And AI, trained on every clever Rust crate on GitHub, is gradient-descending into that surface every time it generates code.

A smaller front end fixes two things at once:

- **The signature** a human reads becomes shorter and load-bearing in different ways.
- **The AI's option space** shrinks. RSScript gives the generator fewer complex shapes to reach for. Constraint is the product.

The product core is the review protocol: `.rssi` semantic contracts, structured diagnostics, source-mapped backend errors, review maps, and semantic diffs. The source language exists to make those review artifacts reliable and cheap to compute. Rust is the backend target and ecosystem substrate.

Before AI, writing code was expensive and reviewing was manageable. That ratio has flipped: generating is cheap, reviewing is the bottleneck. RSScript is designed for the new ratio: AI writes, the compiler checks semantic boundaries, humans focus on the *risk*. What mutates, what gets retained, who owns a resource, where you cross into native or unsafe, what changed in a public API — all of it lives in the signature and in machine-readable diagnostics.

This discipline is binding, not aspirational: the language specification opens with a [Constitution](RSScript_v0.5_Spec.md#constitution) of seven articles that govern every design decision — most importantly that constraint is the product, that review-critical behavior must be explicit in the signature, and that a feature is admitted only if it phrases as a reviewer question and needs no implicit rule to be ergonomic. Restraint is anchored to a measurable property (review cost), which is what keeps it from eroding as the language grows.

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

Under the hood, the v0.5 managed runtime is single-isolate reference counting — think Swift's ARC inside one isolate — and uses Rust `Rc`/`RefCell`-like primitives internally. Managed handles are intentionally not `Send` or `Sync`; they do not cross Rust threads or RSScript isolates. Managed `class` and `struct` values have no user-observable destructor; deterministic cleanup is expressed only through `resource`, `with`, and `ResourcePool`, which are orthogonal to how managed memory is reclaimed. Because managed objects expose no user-visible finalization order, reference counting versus a future tracing collector is a backend decision rather than a language guarantee — a later major version could swap it without changing observable semantics. For v0.5 the tradeoffs are the usual refcounting ones: a per-access dynamic borrow check, and reference cycles do not collect on their own, so they are broken with a `weak` keyword the same way Swift does it. Future cross-isolate or cross-thread transfer should be explicit message passing, not implicit shared heap access.

That per-access cost is relative to native Rust, not to other managed languages. Primitives (`Int`, `Bool`, `Float`, etc.) stay on the stack; refcount only touches heap objects you actually share; there's no GIL, no interpreter loop, no per-object dict header. Managed-only RSScript still lowers to monomorphic Rust through LLVM — typically an order of magnitude faster than Python without ever opting into `local`. `local` is for when you want to compete with hand-tuned Rust, not for routine performance.

### Local when it matters

Hot paths can opt in to local exclusive values, and the checker protects those values from silent retention by managed objects or managed closures:

```rust
features: local

fn fill_scratch(path: read Path) -> Result<Unit, FileError> {
    local scratch = Buffer.new(size: 4096)

    with File.open(path: read path) as file {
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

`local`, `native`, `unsafe`, and `async` are recognized as review capability gates today. `local` enables local ownership features; `native`, `unsafe`, and `async` must be declared before a file can expose those boundaries. Bodyless `native fn` declarations are native-wrapper bindings; executable `native fn` and `async fn` bodies are still outside the v0.5 runtime and are reported before Rust lowering. Async signatures remain useful in interfaces and review diffs. `device`, `ffi`, and `reflection` are reserved review markers: they raise review risk when present, but they do not unlock syntax, lowering, or runtime behavior in v0.5. Ordinary libraries (JSON, File, Image, HTTP, Map, Regex) stay as libraries. Repeated or unknown names are diagnostics so capability boundaries stay explicit.

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

---

## A small example

```rust
features: local

fn copy_file(input: read Path, output: read Path) -> Result<Unit, FileError> {
    local buffer = Buffer.new(size: 8192)

    with File.open_read(path: read input) as reader {
        with File.open_write(path: read output) as writer {
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

---

## Library shape

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

This makes RSScript a review-first source format with a deliberately borrowed backend. The value is in the semantic protocol; the back end is Rust's strongest territory, and RSScript leans into that strength.

---

## Status

Experimental. The current implementation is a Rust-based front-end prototype: lexer, parser, semantic checker with the review-oriented rules, deterministic formatting for the supported AST surface, structured diagnostics, review-map metadata, package review/diff/lock metadata, Rust source lowering with source maps, rustc diagnostic remapping, a small single-isolate runtime crate with `Managed<T>` and `WeakManaged<T>` handles backed by Rust `Rc`/`RefCell`, and core `.rssi` interface signatures parsed through the ordinary interface path. CI gates formatting, lint, tests, and generated-Rust fixtures; golden tests cover lowering and source-map shape.

The v0.5 milestone is a runnable review-first frontend over Rust: strong enough to check real examples, lower them to inspectable Rust, map backend diagnostics back to RSScript source, and keep review risk visible. Self-hosting is a separate experiment, not a v0.5 requirement.

The spec now includes a semantic guarantee table that marks each promise as `static`, `dynamic`, `review-only`, or `unsupported`. In short: read/mut/take, retains(local), resource escape, freshness, local move/use, and handle restrictions are frontend obligations; managed alias conflicts are runtime obligations through `Managed<T>`; native/runtime guarantees remain explicit review boundaries unless their signatures carry matching guarantees.

Current CLI:

```sh
rss check    [--json] [--core|--no-core] [--interface <f.rssi> ...] <file-or-package-directory>
rss check    --explain <code>
rss lint     [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss fmt      <file.rss>
rss review   [--json] --diff <old.rss> <new.rss>
rss review   [--json] --map  <file-or-directory>
rss pkg      check  [--json] [package-directory]
rss pkg      review [--json] <package-directory>
rss pkg      review update [--json] --from <old-rsspkg.lock> --to <new-rsspkg.lock>
rss pkg      lock   [--json] <package-directory>
rss pkg      tree   [--json] [package-directory]
rss pkg      publish --dry-run [--json] [--registry <directory>] [package-directory]
rss pkg      vendor [--dry-run] [--json] [package-directory]
rss pkg      metadata [--dry-run] [--json] [package-directory]
rss pkg      diff   [--json] <old-package-directory> <new-package-directory>
rss lower    --rust  <file.rss> [--out-dir <directory>]
rss run      [--json] <file-or-package-directory> [--out-dir <directory>]
rss remap-rustc  [--json] <rsscript-source-map.json> <rustc-json-lines>
rss verify-rust  [--json] <file-or-package-directory> [--out-dir <directory>]
```

A few details worth knowing:

- `rss check` loads bundled core `.rssi` signatures by default for single files; when pointed at a directory with `rsspkg.toml`, it runs package check.
- `rss lint` reuses the frontend checks and emits warnings. The first lint is `RSL001` — public signatures over the review budget for parameter count, generics, effects, or nested-type depth.
- `rss review --map` validates inputs first, so files with frontend errors get diagnostics instead of misleading classifications.
- `rss review --map --json` reports `unknown_ratio` and `unknown_function_ratio` directly. `tests/fixtures/pass` is treated as the current review-map confidence corpus: it currently reports 280 functions / 3441 lines with 0 unknown functions and 0 unknown lines, including `complex-supported-review-map.rss`, `realistic-supported-review-corpus.rss`, `app-review-benchmark.rss`, managed-closure retention coverage, and the file-backed review-map dogfood classifiers; tests fail if this known-good corpus regresses to unknown or if this documented count drifts from the actual corpus.
- `rss pkg check` validates a local package, loads local path dependency `.rssi` contracts, checks package `.rssi` type and function contracts against source implementations or explicit native bindings, rejects unresolved or conflicting local dependency graphs, runs package review, compares the current semantic lock against `rsspkg.lock`, and scans enabled native Rust wrappers with Cargo metadata. Packages can set `[review.policy] deny_unknown = true` to make unknown review risk fail the check.
- `rss pkg review` reads `rsspkg.toml`, treats `.rssi` files as the public semantic contract, reports package feature names, summarizes public type/function/API counts plus mutating, retaining, resource, fresh-returning, native, unsafe, and unknown APIs, emits per-export review classifications, counts frontend errors in `.rssi` contracts as unknown contract exports, and raises risk for native Rust wrappers, build scripts, proc macros, unsafe policy, external links, frontend diagnostics, and unknown review-map regions.
- `rss pkg review update` compares two `rsspkg.lock` files and reports package version, source, checksum, `.rssi` interface, review metadata, native wrapper, and feature-selection changes.
- `rss pkg lock` emits semantic lock metadata for the root package and local path dependency graph, with SHA-256 hashes for public `.rssi` contracts, review metadata, package contents, and native Rust wrapper contents when enabled.
- `rss pkg tree` shows the dependency graph with review risk. Local path dependencies are expanded recursively; unresolved registry or git dependencies are classified as unknown.
- `rss pkg publish --dry-run` runs pre-publish checks without uploading anything: package consistency, dependency graph review, semver shape, review risk classification, native metadata, a reproducible archive manifest with per-file checksums, and a registry index entry. Unknown package review risk blocks publish readiness instead of being treated as safe. `--registry <directory>` reports the local registry index and archive-manifest paths that would be written.
- `rss pkg vendor` copies local path dependencies into `vendor/<name>-<version>/` and writes `vendor/rss-vendor.json`; unresolved registry or git dependencies stay unknown.
- `rss pkg metadata` writes `review/package-review.json` using the local package review result; `--dry-run` reports the metadata path without writing. Unknown review risk is preserved in the metadata and makes the command result not ok.
- `rss pkg diff` compares two local package directories and reports package version changes, RSScript dependency changes, package feature changes, native Rust wrapper metadata changes, and public `.rssi` semantic contract changes.
- `rss run` lowers a single file, or a package directory with `src/main.rss`, to a temporary Rust package and delegates to `cargo run`; package lowering carries enabled `[native.rust]` wrappers through as generated Cargo path dependencies and maps `native/bindings.rssbind.toml` call bindings into generated Rust calls. `--out-dir` keeps the generated package around for inspection. Diagnostics support `--json`; program stdout stays the program's own.
- `rss verify-rust --out-dir` works for the same file-or-package inputs and keeps the generated package and source map, so unmappable rustc diagnostics can be inspected against the actual generated Rust.

Smallest runnable example:

```sh
cargo run -- run examples/hello.rss
```

```rust
fn main() -> Unit {
    Log.write(message: read "hello RSScript")
    return Unit
}
```

Local verification:

```sh
bash scripts/check.sh
RSSCRIPT_FULL_TESTS=1 bash scripts/check.sh
bash scripts/lint_sources.sh
bash scripts/run_examples.sh
```

Development discipline is documented in [DEVELOPMENT.md](DEVELOPMENT.md): spec
prerequisites first, dogfood as the main pressure test, no fixture-only
shortcuts, and a broad-first testing loop.

CI sets `RSSCRIPT_FULL_TESTS=1` so the same scripts run the full workspace test suite and execute every `examples/*.rss` file through `rss run`.

The runtime hooks wired through so far: `Log.write`, `Assert.equal`, `OS.close`, `List.consume`, `Buffer.consume`, `Path.from_string`, plus the core `File`, `Json`, `Csv`, `Cache`, `Image`, `ImageCache`, HTTP handler, DB resource-pool, config reload, rules config reload, interpreter object links, and `Counter` APIs. `ImageCache` is the first retained managed-container hook; the interpreter hooks model `Environment` and `FunctionObject` as managed handles with the closure link stored weakly. Simple operations like `String.concat` keep `.rssi` signatures for checking but lower directly to Rust std expressions. Built-in literals, arithmetic and comparison operators, `Option<T>` constructors, and surface types (`Bytes`, `Buffer`, `Path`, `List<T>`, `Map<K,V>`, `Set<T>`) lower to the matching Rust forms. User-defined operator overloading stays forbidden.

Control-flow lowering supports `if`, `while`, `loop`, `break`, `continue`, and statement-form `match` for the current `Option<T>` / `Result<T, E>` variant shapes. `match` must cover `Some`/`None`, `Ok`/`Err`, or include `_`; non-exhaustive matches are RSScript diagnostics before Rust lowering.

The supported closure surface includes `noescape Fn()` parameters for temporary callbacks that may use local values without becoming managed closures.

---

## Roadmap

Near term: checker prototype → real AST parser → HIR and symbol table → semantic checker → Rust lowering contract → runtime crate type surface → Rust source generation with source maps → rustc diagnostic mapping → core library signatures → runnable MVP through rustc.

Longer term: deeper semantic review tooling, a larger core library, agent and runtime examples, stronger optimization paths, and an experimental self-hosted frontend.

Post-v0.5 design directions (see spec §20.1) build on the single-isolate, non-`Send` managed model, which is what lets async stay ergonomic without exposing Rust's `Pin`/`Poll`/`Waker`:

- **Ergonomic async.** `Future<T>` as an ordinary isolate-local managed handle, `await`, and a `Stream<T>` / `await for` async-sequence form, on a single-threaded cooperative executor per isolate. Read/mut guards may not cross `await`.
- **Cross-isolate messaging with zero-copy transfer.** Explicit typed channels between isolates; `take`-based moves are the zero-copy transfer path, with single ownership enforced at compile time. Managed handles never cross isolates — only explicit messages do.
- **Two-tier execution.** A HIR-level interpreter for the managed subset gives a fast edit-run loop; Rust lowering stays the production/AOT path. Both observe identical semantics and diagnostics.
- **Structured-fix tooling.** An `rss fix` command applying machine-applicable fixes, plus an analysis server streaming diagnostics and fixes to both human editors and AI repair agents.
- **User-defined sum types** modeled on sealed types with exhaustive `match` (not Rust enums), with exhaustiveness checked before lowering.
- **Registry review-risk badges.** The package registry surfaces review-risk signals (native, unsafe, unknown, mutating/retaining ratios) as first-class quality badges, reusing existing package review metadata.

These intentionally exclude Dart-style conveniences that conflict with review-first semantics: cascade (`..`), extension methods / implicit method resolution, and positional records / implicit flow promotion.

---

## Non-goals

RSScript prioritizes reviewable semantics over syntactic cleverness or maximal expressiveness. It deliberately avoids implicit conversions, user-defined operator overloading, hidden allocation, hidden retention, macro-heavy metaprogramming, complex public signatures, Rust-style lifetime syntax, C++-style implicit magic, and TypeScript-style type gymnastics.

The goal is code humans and tools can review reliably.

---

## License

Dual-licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
