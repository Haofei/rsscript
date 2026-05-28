# RSScript

**Reviewable System Script — a small language that lowers to Rust, built for code AI writes but humans still have to trust.**

```text
Easy by default.
Fast when local.
Reviewable by design.
Compiled through Rust.
```

I like Rust, and that is where this project starts. AI-generated Rust can be hard to review at speed: lifetimes, ownership shapes, trait-heavy APIs, mutation boundaries, retention — all technically correct, all expensive to audit when a single PR drops hundreds of new lines on you.

RSScript is an experiment in putting a smaller, review-first semantic layer in front of Rust. It keeps rustc, Cargo, and the crate ecosystem; it just makes mutation, retention, resources, native boundaries, and managed/local transitions explicit *before* the Rust gets generated.

One question drives the design:

> If AI can write hundreds of lines of code in seconds, how can humans review that code safely?

---

## Why bother

Before AI, writing code was expensive and reviewing was manageable. That ratio has flipped — generating code is now cheap, reviewing generated code is the bottleneck.

Existing languages were designed for humans writing code by hand. RSScript is designed for a workflow where AI writes, the compiler checks semantic boundaries, and humans review the *risk profile first*. So the things that actually matter to a reviewer — what mutates, what gets retained, who owns a resource, where you cross into native or unsafe, what changed in a public API — are pushed into source and machine-readable diagnostics instead of being inferred from context.

---

## The model, in three layers

### Managed by default

Most code should look ordinary:

```rust
let user = User.load(id: read user_id)
let response = Response.ok(body: read user)
```

Managed values are easy to share, store, and drop into long-lived graphs. This is the default for business logic, agent memory, configuration, caches, ASTs, request/response objects — the broad layer outside hot paths.

### Local when it matters

Hot paths can opt in to local exclusive values, and the checker protects those values from silent retention by managed objects or managed closures:

```rust
features: local

local scratch = Buffer.new(size: 4096)

with File.open(path: read path) as file {
    File.read_into(file: mut file, buffer: mut scratch)?
}
```

That gives you a clear performance path for parser buffers, JSON decoding, prompt buffers, image preprocessing, and the like — without leaking that complexity into the rest of the program.

### Features are review signals

Files are managed-only unless they declare otherwise:

```rust
features: local
```

Only `local` is implemented today. Names like `native`, `unsafe`, `async`, `device`, `ffi`, and `reflection` are reserved for capabilities that genuinely change review risk. Ordinary libraries (JSON, File, Image, HTTP, Map, Regex) stay as libraries. Repeated or unknown names are diagnostics so capability boundaries stay explicit.

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

---

## A small example

```rust
fn copy_file(input: read Path, output: read Path) -> Result<Unit, IOError> {
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

Experimental. The current implementation is a Rust-based front-end prototype: lexer, parser, semantic checker with the review-oriented rules, structured diagnostics, review-map metadata, package review/diff/lock metadata, Rust source lowering with source maps, rustc diagnostic remapping, a small runtime crate, and core `.rssi` interface signatures parsed through the ordinary interface path. CI gates formatting, lint, tests, and generated-Rust fixtures; golden tests cover lowering and source-map shape.

The first milestone is a checker strong enough to validate the language model against real examples.

Current CLI:

```sh
rss check    [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss check    --explain <code>
rss lint     [--json] [--core|--no-core] [--interface <f.rssi> ...] <file.rss>
rss fmt      <file.rss>
rss review   [--json] --diff <old.rss> <new.rss>
rss review   [--json] --map  <file-or-directory>
rss package  review [--json] <package-directory>
rss package  lock   [--json] <package-directory>
rss package  diff   [--json] <old-package-directory> <new-package-directory>
rss lower    --rust  <file.rss> [--out-dir <directory>]
rss run      [--json] <file.rss> [--out-dir <directory>]
rss remap-rustc  [--json] <rsscript-source-map.json> <rustc-json-lines>
rss verify-rust  [--json] <file.rss> [--out-dir <directory>]
```

A few details worth knowing:

- `rss check` loads bundled core `.rssi` signatures by default; `--no-core` is for testing against isolated user interfaces.
- `rss lint` reuses the frontend checks and emits warnings. The first lint is `RSL001` — public signatures over the review budget for parameter count, generics, effects, or nested-type depth.
- `rss review --map` validates inputs first, so files with frontend errors get diagnostics instead of misleading classifications.
- `rss package review` reads `rsspkg.toml`, treats `.rssi` files as the public semantic contract, and raises risk for native Rust wrappers, build scripts, proc macros, unsafe policy, external links, frontend diagnostics, and unknown review-map regions.
- `rss package lock` emits root package lock metadata with SHA-256 hashes for the public `.rssi` contract, review metadata, package contents, and native Rust wrapper contents when enabled.
- `rss package diff` compares two local package directories and reports package version changes, RSScript dependency changes, package feature changes, native Rust wrapper metadata changes, and public `.rssi` semantic contract changes.
- `rss run` lowers to a temporary Rust package and delegates to `cargo run`; `--out-dir` keeps the generated package around for inspection. Diagnostics support `--json`; program stdout stays the program's own.
- `rss verify-rust --out-dir` keeps the generated package and source map, so unmappable rustc diagnostics can be inspected against the actual generated Rust.

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

The runtime hooks wired through so far: `Log.write`, `Assert.equal`, `OS.close`, `List.consume`, `Buffer.consume`, plus the core `File`, `Json`, `Csv`, `Cache`, `Image`, `ImageCache`, HTTP handler, DB resource-pool, config reload, rules config reload, interpreter object-cycle, and `Counter` APIs. `ImageCache` is the first retained managed-container hook; the interpreter hooks model `Environment` and `FunctionObject` as managed handles with a closure cycle between them. Simple operations like `String.concat` keep `.rssi` signatures for checking but lower directly to Rust std expressions. Built-in literals, arithmetic and comparison operators, `Option<T>` constructors, and surface types (`Bytes`, `Buffer`, `Path`, `List<T>`, `Map<K,V>`, `Set<T>`) lower to the matching Rust forms. User-defined operator overloading stays forbidden.

---

## Roadmap

Near term: checker prototype → real AST parser → HIR and symbol table → semantic checker → Rust lowering contract → runtime crate type surface → Rust source generation with source maps → rustc diagnostic mapping → core library signatures → runnable MVP through rustc.

Longer term: semantic review tooling, a core library MVP, agent and runtime examples, a self-hosted frontend, stronger optimization paths.

---

## Non-goals

RSScript prioritizes reviewable semantics over syntactic cleverness or maximal expressiveness. It deliberately avoids implicit conversions, user-defined operator overloading, hidden allocation, hidden retention, macro-heavy metaprogramming, complex public signatures, Rust-style lifetime syntax, C++-style implicit magic, and TypeScript-style type gymnastics.

The goal is code humans and tools can review reliably.

---

## License

Dual-licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
