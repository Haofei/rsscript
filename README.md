# RSScript

**A reviewable systems scripting language for code AI writes and humans still have to trust.**

```text
Reviewable source.
Rust-backed execution.
Native escape hatches when they are worth reviewing.
```

This came out of reviewing 100k+ lines of AI-generated Rust over six months. The same shapes kept hurting: `Arc<Mutex<HashMap<...>>>` stacked four deep, signatures with eight trait bounds where one would do, `Pin<Box<dyn Future<...>>>` blocking the view of what a function actually does, retention buried three call levels down, four hundred lines of correct-but-dense code in a single PR where the important ten percent was hard to find. The Rust compiler accepted it. Review still took too much human attention.

There's also a long-standing wishlist for this kind of language: managed-by-default app code, explicit performance escapes, and a direct path back to native systems work. The request shows up in `/r/rust` threads and language-design posts regularly. AI review pain is what finally made the cost/benefit click for me.

RSScript is a smaller front end that lowers to Rust. It keeps rustc, Cargo, and the crate ecosystem; it compresses the surface so a reviewer's first read costs less, and pushes mutation, retention, resources, and native boundaries into the signature where review can see them. Advanced Rust remains available through `features: native` as an explicit review boundary.

---

## Why a Smaller Review Surface

The obvious alternative is proc-macros, a Clippy ruleset, and a review tool over Rust directly. The problem is that **Rust's signatures themselves are part of the review cost**. `Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>` carries four bits a reviewer needs and a dozen bits that exist to satisfy the type system. A smaller source language changes the surface itself, instead of asking tools to recover intent afterward. And AI, trained on every clever Rust crate on GitHub, is gradient-descending into that surface every time it generates code.

A smaller front end fixes two things at once:

- **The signature** a human reads becomes shorter and load-bearing in different ways.
- **The AI's option space** shrinks. RSScript gives the generator fewer complex shapes to reach for. Constraint is the product.

Before AI, writing code was expensive and reviewing was manageable. That ratio has flipped: generating is cheap, reviewing is the bottleneck. RSScript is designed for the new ratio: AI writes, the compiler checks semantic boundaries, humans focus on the *risk*. What mutates, what gets retained, who owns a resource, where you cross into native or unsafe, what changed in a public API — all of it lives in the signature and in machine-readable diagnostics.

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

Under the hood, managed is reference counted — think Swift's ARC, not a tracing GC. Destruction is deterministic and lowers to Rust's `Arc`, so cross-thread sharing works without extra ceremony at the source level. The usual tradeoffs apply: refcounting has a per-access cost, and cycles are broken with a `weak` keyword the same way Swift does it.

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

### Features are review signals

Files are managed-only unless they declare otherwise:

```rust
features: local
```

`local`, `native`, `unsafe`, and `async` are recognized as review capability gates today. `local` enables local ownership features; `native`, `unsafe`, and `async` must be declared before a file can expose those boundaries. Names like `device`, `ffi`, and `reflection` are reserved for capabilities that genuinely change review risk. Ordinary libraries (JSON, File, Image, HTTP, Map, Regex) stay as libraries. Repeated or unknown names are diagnostics so capability boundaries stay explicit.

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

RSScript owns the front end: syntax, semantic checks, effects, managed/local/resource rules, diagnostics, review metadata, source mapping, core signatures. Rust owns everything below: codegen, optimization, platform support, linking, the crate ecosystem.

This is an architectural choice. The value is in the front end; the back end is Rust's strongest territory, and RSScript leans into that strength.

---

## Status

Experimental. The current implementation is a Rust-based front-end prototype: lexer, parser, semantic checker with the review-oriented rules, deterministic formatting for the supported AST surface, structured diagnostics, review-map metadata, package review/diff/lock metadata, Rust source lowering with source maps, rustc diagnostic remapping, a small runtime crate with `Managed<T>` and `WeakManaged<T>` handles backed by Rust `Arc`/`RwLock`, and core `.rssi` interface signatures parsed through the ordinary interface path. CI gates formatting, lint, tests, and generated-Rust fixtures; golden tests cover lowering and source-map shape.

The v0.5 milestone is a runnable review-first frontend over Rust: strong enough to check real examples, lower them to inspectable Rust, map backend diagnostics back to RSScript source, and keep review risk visible. Self-hosting is a separate experiment, not a v0.5 requirement.

Current CLI:

```sh
rss check    [--json] [--core|--no-core] [--interface <f.rssi> ...] <file-or-package-directory>
rss check    --explain <code>
rss lint     [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss fmt      <file.rss>
rss review   [--json] --diff <old.rss> <new.rss>
rss review   [--json] --map  <file-or-directory>
rss package  check  [--json] [package-directory]
rss package  review [--json] <package-directory>
rss package  review update [--json] --from <old-rsspkg.lock> --to <new-rsspkg.lock>
rss package  lock   [--json] <package-directory>
rss package  tree   [--json] [package-directory]
rss package  publish --dry-run [--json] [--registry <directory>] [package-directory]
rss package  vendor [--dry-run] [--json] [package-directory]
rss package  metadata [--dry-run] [--json] [package-directory]
rss package  diff   [--json] <old-package-directory> <new-package-directory>
rss lower    --rust  <file.rss> [--out-dir <directory>]
rss run      [--json] <file-or-package-directory> [--out-dir <directory>]
rss remap-rustc  [--json] <rsscript-source-map.json> <rustc-json-lines>
rss verify-rust  [--json] <file-or-package-directory> [--out-dir <directory>]
```

A few details worth knowing:

- `rss check` loads bundled core `.rssi` signatures by default for single files; when pointed at a directory with `rsspkg.toml`, it runs package check.
- `rss lint` reuses the frontend checks and emits warnings. The first lint is `RSL001` — public signatures over the review budget for parameter count, generics, effects, or nested-type depth.
- `rss review --map` validates inputs first, so files with frontend errors get diagnostics instead of misleading classifications.
- `rss package check` validates a local package, loads local path dependency `.rssi` contracts, checks package `.rssi` type and function contracts against source implementations, rejects unresolved or conflicting local dependency graphs, runs package review, compares the current semantic lock against `rsspkg.lock`, and scans enabled native Rust wrappers with Cargo metadata.
- `rss package review` reads `rsspkg.toml`, treats `.rssi` files as the public semantic contract, and raises risk for native Rust wrappers, build scripts, proc macros, unsafe policy, external links, frontend diagnostics, and unknown review-map regions.
- `rss package review update` compares two `rsspkg.lock` files and reports package version, source, checksum, `.rssi` interface, review metadata, native wrapper, and feature-selection changes.
- `rss package lock` emits semantic lock metadata for the root package and local path dependency graph, with SHA-256 hashes for public `.rssi` contracts, review metadata, package contents, and native Rust wrapper contents when enabled.
- `rss package tree` shows the dependency graph with review risk. Local path dependencies are expanded recursively; unresolved registry or git dependencies are classified as unknown.
- `rss package publish --dry-run` runs pre-publish checks without uploading anything: package consistency, dependency graph review, semver shape, review metadata, native metadata, a reproducible archive manifest with per-file checksums, and a registry index entry. `--registry <directory>` reports the local registry index and archive-manifest paths that would be written.
- `rss package vendor` copies local path dependencies into `vendor/<name>-<version>/` and writes `vendor/rss-vendor.json`; unresolved registry or git dependencies stay unknown.
- `rss package metadata` writes `review/package-review.json` using the local package review result; `--dry-run` reports the metadata path without writing.
- `rss package diff` compares two local package directories and reports package version changes, RSScript dependency changes, package feature changes, native Rust wrapper metadata changes, and public `.rssi` semantic contract changes.
- `rss run` lowers a single file, or a package directory with `src/main.rss`, to a temporary Rust package and delegates to `cargo run`; package lowering carries enabled `[native.rust]` wrappers through as generated Cargo path dependencies. `--out-dir` keeps the generated package around for inspection. Diagnostics support `--json`; program stdout stays the program's own.
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

CI sets `RSSCRIPT_FULL_TESTS=1` so the same scripts run the full workspace test suite and execute every `examples/*.rss` file through `rss run`.

The runtime hooks wired through so far: `Log.write`, `Assert.equal`, `OS.close`, `List.consume`, `Buffer.consume`, plus the core `File`, `Json`, `Csv`, `Cache`, `Image`, `ImageCache`, HTTP handler, DB resource-pool, config reload, rules config reload, interpreter object links, and `Counter` APIs. `ImageCache` is the first retained managed-container hook; the interpreter hooks model `Environment` and `FunctionObject` as managed handles with the closure link stored weakly. Simple operations like `String.concat` keep `.rssi` signatures for checking but lower directly to Rust std expressions. Built-in literals, arithmetic and comparison operators, `Option<T>` constructors, and surface types (`Bytes`, `Buffer`, `Path`, `List<T>`, `Map<K,V>`, `Set<T>`) lower to the matching Rust forms. User-defined operator overloading stays forbidden.

---

## Roadmap

Near term: checker prototype → real AST parser → HIR and symbol table → semantic checker → Rust lowering contract → runtime crate type surface → Rust source generation with source maps → rustc diagnostic mapping → core library signatures → runnable MVP through rustc.

Longer term: deeper semantic review tooling, a larger core library, agent and runtime examples, stronger optimization paths, and an experimental self-hosted frontend.

---

## Non-goals

RSScript prioritizes reviewable semantics over syntactic cleverness or maximal expressiveness. It deliberately avoids implicit conversions, user-defined operator overloading, hidden allocation, hidden retention, macro-heavy metaprogramming, complex public signatures, Rust-style lifetime syntax, C++-style implicit magic, and TypeScript-style type gymnastics.

The goal is code humans and tools can review reliably.

---

## License

Dual-licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
