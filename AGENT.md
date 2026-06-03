# RSScript (`.rss`) — In-Context Language Guide for LLMs

You are writing **RSScript (RSS)**, a new language with **no presence in your
training data**. Do not assume Rust/Swift/TS syntax carries over — follow this
guide literally. RSS is a review-first language for AI-generated systems code: it
makes mutation, retention, resource lifetime, and ownership transitions
*explicit and named*, then lowers to Rust source (rustc is the backend).

Mental model:

```text
Easy by default (managed values, no lifetimes).
Fast when local (opt-in exclusive values).
Reviewable by design (every effect is spelled out).
One canonical spelling per operation — no shorthand alternatives, no inference in public APIs.
```

---

## 0. The rules you will most likely get wrong

1. **Every call argument is named**: `f(name: value)`, never `f(value)`.
2. **Arguments that pass a managed value or reference carry a data-effect
   keyword**: `read` (inspect), `mut` (modify), `take` (consume). Copy scalars
   (`Int`, `Bool`) are passed bare: `width: 800`.
3. **No hidden behavior**: no implicit conversions, no auto-deref/ref, no
   operator overloading, no getters, no macros. Conversions are explicit calls:
   `Int64.from(value: x)`.
4. **Type annotations are checked contracts**, not coercions. `let y: Int64 = x`
   is rejected unless `x` is already `Int64`.
5. **Statements end at newline** (no semicolons). Blocks use `{ }`.
6. **`return` is explicit.** `main` returns `Result<Unit, E>` and ends with
   `return Ok(Unit)`.
7. Method calls are **qualified by type**: `Image.resize(image: mut image, ...)`,
   not `image.resize(...)` (except the receiver shorthand in §3.4).

### The explicitness budget (the meta-rule behind all of the above)

RSS is explicit *to make review cheap* — not for its own sake. More keywords is
not safer; ceremony that states nothing buries the one marker that matters. So:

> **Mark the departure, keep the norm silent.** Reach for a marker only when it
> is true and a reviewer would have to verify it. The safe default needs no word.

| Departure marker | Use **only** when it is actually true |
|------------------|----------------------------------------|
| `mut` / `take`   | you really modify / consume the value (else `read`) |
| `local`          | you need an exclusive fast value (else stay managed — no marker) |
| `manage`         | you are moving a `local` into the managed world |
| `fresh`          | you return a newly-created value across the boundary |
| `effects(retains(...))` | the function keeps a parameter alive after return |
| `native` / `unsafe` / `async` | the body actually crosses that boundary |

Do not add a marker "to be safe": writing `mut` when you only read, or `local`
when managed is fine, is wrong (often a checker error) and adds review noise.
Before emitting any keyword, ask: *is there a real choice here, and would a
reviewer judge wrong if it were absent?* If not, leave it out. (`read` is the one
marker that is **default in meaning but mandatory in syntax**: it is the
least-privilege baseline effect, yet you must still write it at call sites — an
omitted effect is the error RS0202, never an inferred `read`. Write it, but treat
it as the lone exception, not the pattern. Spec: Constitution Article VIII, §2A.)

---

## 1. Running the toolchain (binary is `rss`)

```sh
rss check  [--core|--no-core] [--interface <f.rssi> ...] <file.rss>   # type/effect check
rss check  <package-directory>            # check a package
rss check  --explain <CODE>               # explain a diagnostic code, e.g. RS0026
rss lint   <file.rss>                     # check + style lints
rss fmt    <file.rss>                     # canonical formatter
rss run    <file-or-package-dir> [-- <args>...]   # lower to Rust + build + run
rss test   [--all] [--filter <substr>]
rss dev    [--run] [--once] <file-or-dir> # watch loop
rss pkg    [--json] [dir]            # package health check
rss pkg    review [--json] [dir]     # review surface
rss pkg    diff [--json] <old-dir> <new-dir>
rss pkg    ci [--json] [dir]         # CI-facing package check
rss pkg    publish --dry-run [--json] [--registry <dir>] [dir]
rss pkg    lock [dir]                # (re)write rsspkg.lock from the resolved graph
rss pkg    tree|metadata|vendor [--json|--reir] [dir]
```

`--core` (default) loads the implicit standard library (`String`, `List`, `Map`,
`File`, `Path`, `Result`, `Option`, …). Diagnostics have stable codes
(`RSxxxx`), a summary, a source span, and a JSON form.

---

## 2. File anatomy

```rust
features: local            // optional capability line, MUST be first if present

fn main() -> Result<Unit, FileError> {
    let path = Path.from_string(value: read "out.txt")
    Log.write(message: read "hello")          // bare call = statement
    return Ok(Unit)
}
```

- `features:` declares file-level capabilities. Known values: `local`, `async`,
  `native`. Combine with commas: `features: async, native, local`. Omit the line
  for plain managed code.
- `.rss` = source/implementation. `.rssi` = interface contract (signatures only,
  see §9).

---

## 3. Functions, calls, effects

### 3.1 Declaration

```rust
fn handle(request: read Request) -> Result<fresh Response, HttpError> {
    let body = Request.path(request: read request)
    return Response.ok(body: read body)        // last value still needs `return`
}
```

Signature = `fn name(params) -> ReturnType [effects(...)] { body }`.
Each param is `name: <effect> Type`. Return may be `fresh T` (newly created, see
§5), `T` (managed), or `Result<...>` / `Option<...>`.

### 3.2 Data effects (the core of every call)

| Keyword | Meaning at a parameter / argument                       |
|---------|---------------------------------------------------------|
| `read`  | inspected, not modified, not retained                   |
| `mut`   | modified in place                                       |
| `take`  | consumed (ownership moves in; caller can't use it after)|

```rust
fn resize_all(image: mut Image, width: Int) -> Unit { ... }
```

Called as:

```rust
resize_all(image: mut image, width: 800)   // `mut` arg matches `mut` param; Int bare
```

Copy scalars (`Int`, `Bool`, …) never take an effect keyword. Managed values and
references always do.

### 3.3 Retention — `effects(retains(...))`

If a function keeps a parameter alive after it returns (stores it in a field, a
collection, or a closure capture), it must declare it:

```rust
fn cache_put(cache: mut ImageCache, key: read String, value: read Image) -> Unit
    effects(retains(key), retains(value))
{
    Map.insert(map: mut cache.entries, key: read key, value: read value)
}
```

Other effect/guarantee tokens that may appear in `effects(...)`: `native`,
`pure`, `no_panic`, `noalloc`, `no_block`.

### 3.4 Receiver-call shorthand (the only alt spelling, expands 1:1)

```rust
mut image.resize(width: 256, height: 256)   // shorthand
Image.resize(self: mut image, width: 256, height: 256)   // canonical expansion
```

The leading effect keyword (`read`/`mut`/`take`) is **mandatory**; it must
resolve to exactly one method or it is rejected.

### 3.5 Error handling

`?` propagates the error arm of a `Result`/`Option`:

```rust
let image = Image.load(path: read input)?      // returns early on Err
```

`Result<T, E>` constructors: `Ok(x)`, `Err(e)`. `Option<T>`: `Some(x)`, `None`.
`Unit` is the unit type and its value.

---

## 4. Control flow

```rust
if count == 1 {
    Log.write(message: read "one")
} else {
    Log.write(message: read "many")
}

for query in queries {            // `for` iterates a `List<T>` only (v0.6)
    DbConnection.query(conn: mut conn, sql: read query.sql)?
}

loop { ... }                      // infinite loop (no condition); exit with `break`
while count == 0 { ... }          // conditional loop; `break` / `continue`

match result {
    Ok(value) => {
        return Ok(value)
    }
    Err(error) => {
        Log.write(message: read "failed")
        return Ok(Unit)
    }
}
```

`match` is exhaustive over a sealed sum type / `Result` / `Option`. Patterns:
`Variant(binding)`, literals, and `_` wildcard. `match` is also usable in
expression position.

---

## 5. Bindings, freshness, and the managed/local boundary

```text
let      binding to a managed value (default; no lifetime reasoning)
let mut  reassignable binding; reassign with `x = e` (no `let`)
local    exclusive, statically move-checked value (needs `features: local`)
manage   one-way move of a local value into the managed runtime
fresh    a newly-created value returned across a boundary
```

```rust
let mut total = 0
total = total + 1          // assignment to a `mut` binding, no `let`
```

```rust
features: local

fn make_thumbnail(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?   // exclusive, fast
    mut image.resize(width: 256, height: 256)
    mut image.normalize()

    let shared = manage image                      // local -> managed, one-way
    read shared.save(path: read output)?
    return Ok(Unit)
}
```

- A `fresh T` return is a value created in the function and handed out clean:
  `fn load() -> Result<fresh Config, E>`.
- `take` a local into a fresh struct field to move it; `read` to copy/share.
- After `manage x` or `take x`, the original `local` binding is dead.

---

## 6. Types

### 6.1 Declarations

```rust
class ImageCache {            // managed reference type (default for app types)
    entries: Map<String, Image>
}

struct Config {              // value/struct type
    name: String
    rules: handle List<Rule>     // `handle` field: a retained managed handle
    workspace: Buffer
    parent: weak Node            // `weak`: non-owning handle, must be upgraded to use
}

resource DbConnection { ... }   // a resource: must be acquired/released via `with`

struct ReadArgs derives(Clone, JsonDecode) {   // derives are explicit & listed
    path: Option<String>
    max_lines: Option<Int>
}
```

- `class` = managed (heap, ref-counted, GC-swappable); **no user destructor**.
- `struct` = composite value type.
- `resource` = lifetime-managed external thing (connections, files, pools).
- `handle` / `weak` field modifiers control retention vs non-owning references.

### 6.2 Sum types (sealed)

```rust
sum ToolRuntime {
    CoreToolRuntime
    // each line is a variant; may carry fields like a struct variant
}
```

Sum types are sealed (closed) — `match` over them is exhaustive.

### 6.3 Protocols & generics

```rust
protocol Ord {
    fn compare(self: read Self, other: read Self) -> Int   // `Self` = implementing type
}

fn write_line<W: Writer>(writer: mut W, message: read String) -> Unit {
    mut writer.write(message: read message)
}
```

Generic bound kinds: `<T: Managed>`, `<T: Struct>`, `<T: Resource>`, or
`<T: ProtocolName>`. Protocols are the explicit interface mechanism (no implicit
trait resolution).

### 6.4 Constructors & aliases

```rust
let cfg = Config(name: "default", rules: read rules, workspace: take workspace)
```

Type aliases and constants are top-level declarations:

```rust
type WorkspacePath = Path        // type alias
const MAX: Int = 16              // const
```

Constructors are call-like and use the same named-arg + effect rules.

---

## 7. Async (restricted in v0.6)

```rust
features: async

async fn fetch(url: read Url) -> Result<Int, HttpError> {
    let response = await Http.get_async(url: read url)?   // await directly consumes an async call
    return Ok(HttpResponse.status(response: read response))
}
```

Rules: `await` appears **only inside an `async fn`** and must directly consume an
async call. There is no `Future`/`Poll`/`spawn` surface in v0.6 (single-isolate,
cooperative). `spawn` and `Stream<T>` are review-visible but not executable yet.

---

## 8. Forbidden (will be rejected — do not produce these)

```text
positional call args            f(x)                 -> use f(name: read x)
missing effect keyword          f(x: image)          -> f(x: read image)
implicit conversion             let p: Path = "s"    -> Path.from_string(value: read "s")
auto-deref / auto-ref / getters
operator overloading / macros
user destructor on class/struct
await outside async fn / await not consuming an async call
unsupported syntax lowering to placeholders (todo!())
```

Selected diagnostics (run `rss check --explain <code>`): `RS0201` unnamed
argument, `RS0202` missing data effect, `RS0203` unknown argument, `RS0204`
missing argument, `RS0026` unknown binding, `RS0206` unknown callee, `RS0207`
argument type mismatch, `RS0029` await outside async, `RS0601` fresh return not
clean, `RS0301` managed-to-local, `RS0015` unsupported syntax.

---

## 9. Interface files (`.rssi`)

A `.rssi` is the reviewable public contract: signatures only, no bodies.

```rust
features: async, native, local

pub async native fn File.read_all_async(path: read Path) -> Result<fresh Bytes, FileError>
    effects(native)

pub fn File.bytes_stream(path: read Path, chunk_size: Int)
    -> Result<fresh Stream<Bytes>, ChannelError>
    effects(native)

protocol Ord {
    fn compare(self: read Self, other: read Self) -> Int
}
```

`pub` exports it; `native` marks a body provided by a Rust wrapper; type and
protocol declarations may appear too.

---

## 10. Package manager (`rss pkg`)

The package manager is a **semantic / review layer over Cargo**, not a Cargo
replacement. Cargo builds native Rust and owns `Cargo.lock`; `rss pkg` owns
`.rssi` contracts, RSS dependency resolution, `rsspkg.lock`, feature-conditioned
interfaces, semantic package diff, and computed review/risk metadata.

### 10.1 `rsspkg.toml`

```toml
[package]
name = "rss-json"
version = "0.1.0"
edition = "2026"
description = "Reviewable JSON APIs"
license = "MIT"
kind = "native-wrapper"            # optional package kind

[interfaces]
paths = ["interface"]              # where .rssi contracts live
exports = ["Json"]

[interfaces.features.streaming]    # feature-conditioned interface surface
paths = ["interface/streaming"]
exports = ["Json"]

[sources]
paths = ["src"]                    # where .rss sources live

[dependencies]
rss-core = "0.5"                   # registry version
rss-async = { path = "../async" }  # path dependency

[dev-dependencies]
rss-test = "0.5"

[features]
default = []
streaming = []

[providers]                        # executable packages must pick a provider
async = "rss-async-runtime"        #   for each virtual capability they use

[review.policy]                    # machine-checked review gates
deny_unknown = false
deny_native = false
deny_unsafe_apis = true
max_public_params = 8
build_execution_default = "forbid" # forbid | review | allow

[native.rust]                      # optional Rust native wrapper
enabled = true
path = "native/rust"
crate = "rss_json_native"
```

### 10.2 Virtual capabilities & providers

A capability (e.g. `async`) is a **virtual package**: an interface with no single
built-in implementation. A **provider** package declares it implements one:

```toml
# in the provider's rsspkg.toml
[implements."async"]
version = "0.1"
interface_effective_hash = "sha256:..."
```

A consumer that is **executable** must explicitly select a provider in
`[providers]` (e.g. `async = "rss-async-runtime"`). Libraries stay
provider-agnostic.

### 10.3 Layout (typical package)

```text
my-pkg/
  rsspkg.toml
  rsspkg.lock          # semantic lock checked by rss pkg
  interface/*.rssi     # public contracts
  src/*.rss            # implementation
  native/rust/         # optional Rust wrapper (Cargo crate) + bindings
```

### 10.4 Review-first workflow

```sh
rss pkg [--json] [dir]                         # manifest + interfaces + sources + lock + graph, NO native build
rss pkg review [--json] [dir]                  # public-contract / risk / native-boundary report
rss pkg diff [--json] <old-dir> <new-dir>      # semantic diff of two package versions
rss pkg ci [--json] [dir]                      # CI-facing package check
rss pkg publish --dry-run [--json] [--registry <dir>] [dir]
rss pkg lock [dir]                             # (re)write rsspkg.lock from the resolved graph
rss pkg tree [--json|--reir] [dir]             # dependency graph annotated with risk
rss pkg metadata [--verify|--dry-run] [dir]    # write/verify review + REIR metadata
rss pkg vendor [--dry-run] [dir]               # vendor path dependencies
```

Commands that only read/resolve/review **never execute** a dependency's
build-time code; running native builds is a separate, explicit step. A resolved
graph is not, by itself, an acceptable review — the point is to answer *what
public contracts, mutations, retentions, resources, native/unsafe/async
boundaries, or build-time execution changed* before you build or run.

---

## 11. Worked example (managed cache with retention)

```rust
class ImageCache {
    entries: Map<String, Image>
}

fn cache_put(cache: mut ImageCache, key: read String, value: read Image) -> Unit
    effects(retains(key), retains(value))
{
    Map.insert(map: mut cache.entries, key: read key, value: read value)
}

fn main() -> Result<Unit, ImageError> {
    let cache = ImageCache.new(capacity: 16)
    let path = Path.from_string(value: read "in.png")
    let image = Image.load(path: read path)?
    cache_put(cache: mut cache, key: read "in", value: read image)
    return Ok(Unit)
}
```

That is the whole canonical style: named args, explicit effects, explicit
retention, explicit construction, explicit `return`. When unsure, prefer the
**most explicit** spelling — RSS has exactly one, and inference is never allowed
in public contracts.
```
