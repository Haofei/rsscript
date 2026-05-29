# RSScript (Reviewable System Script) Language Specification v0.5 — Reorganized Draft

Status: Draft / editorial consolidation candidate
Version: 0.5.1-editorial
Based on: RSScript v0.5 draft
Audience: language designers, compiler implementers, standard-library authors, review-tool authors
Architecture note: v0.5 uses **RSScript frontend -> Rust source lowering -> rustc backend**.

---

## 0. Reading Guide and Normative Hierarchy

This document reorganizes the v0.5 draft around the semantic boundaries that the compiler must enforce before Rust lowering. The previous draft had many correct rules, but several were scattered across runtime, data-effect, resource, closure, review, and example chapters. This edition treats the following chapters as the primary semantic authority:

```text
Chapter 5   Expression modes and materialization
Chapter 8   Places, conflict roots, and same-call conflicts
Chapter 9   Call-like expressions, constructors, and variants
Chapter 10  Data effects, retention, and managed closure capture
Chapter 12  Resources, with, and ResourcePool
Chapter 17  Diagnostics and source mapping
```

If an example or later explanatory section conflicts with these chapters, the semantic boundary chapter wins.

RSScript package management is specified separately. Package tooling consumes `.rssi` semantic contracts; it must not redefine or weaken language semantics.

---

## 1. Executive Summary

RSScript (**Reviewable System Script**) is a constrained, review-first source format for AI-generated systems code with Rust-grade execution.

Its design target is:

```text
AI codegen target.
Semantic review protocol.
Rust lowering backend.
```

The language core is:

```text
let       = managed binding
local     = local exclusive binding
with      = scoped resource management
fresh     = newly-created struct shell
manage    = move local value into managed runtime
read      = parameter is inspected
mut       = parameter is modified
take      = local value is consumed
retains   = function may retain a parameter after return
```

RSScript exists because AI-era software shifts the bottleneck from writing code to reviewing generated code. The language makes review-critical behavior explicit:

```text
mutation
retention
resource lifetime
local performance boundaries
managed/local transitions
freshness guarantees
native/unsafe boundaries
```

v0.5 makes one implementation decision normative for the MVP:

```text
RSScript lowers to Rust source.
rustc is the backend.
Generated Rust is typed implementation IR, not the RSScript semantic model.
```

---

## 2. Design Principles

### 2.1 Managed by default

Ordinary application code uses managed values and avoids ownership/lifetime reasoning.

```rust
fn main() -> Result<Unit, ImageError> {
    let image = Image.load(path: read "in.png")?
    Image.resize(image: mut image, width: 800, height: 600)
    Image.save(image: read image, path: read "out.png")?
    return Ok(Unit)
}
```

### 2.2 Fast when local

Performance-sensitive code opts into local exclusive values with `features: local`.

```rust
features: local

fn process(path: read Path) -> Result<fresh Image, ImageError> {
    local image = Image.load(path: read path)?
    Image.resize(image: mut image, width: 800, height: 600)
    Image.normalize(image: mut image)
    return Ok(image)
}
```

Local values enable static move checking, field-level mutation checking, escape analysis, buffer reuse, and explicit managed crossing through `manage`.

### 2.3 One canonical review style

There are no script/review/performance syntax profiles.

Canonical:

```rust
Image.save(image: read image, path: read output)
```

Non-canonical:

```rust
save(image: image, path: output)
```

The same operation must not have multiple equivalent spellings.

### 2.4 No hidden behavior

RSScript forbids hidden conversions and hidden calls.

Forbidden:

```text
implicit conversion
auto-deref
auto-ref
implicit From / Into chains
user-defined operator overloading
getter magic
dynamic field creation
macro systems that hide control flow
```

If a conversion happens, it is visible:

```rust
let y: Int64 = Int64.from(value: x)
```

### 2.5 Public APIs are review contracts

A public API must expose:

```text
argument names
argument types
data effects: read / mut / take
return freshness: fresh or managed
retention effects: effects(retains(...))
guarantees: no_panic / noalloc / pure / no_block
native boundaries
unsafe boundaries
```

Public APIs must not rely on inference.

### 2.6 Diagnostics are part of the language

Diagnostics must serve humans and AI repair agents.

Each diagnostic has:

```text
stable code
human summary
primary source span
causal chain
structured fixes
machine-readable JSON form
```

### 2.7 Rust is a backend, not the language model

RSScript lowers to Rust source while keeping Rust lifetimes, trait-bound complexity, borrow-checker diagnostics, and backend representation details behind the RSScript review protocol.

Valid RSScript code should not require the user to understand generated Rust.

### 2.8 Feature admission rule

RSScript features must aggregate rather than interact. Most feature-interaction
complexity in other languages comes from independently designed mechanisms that
meet in an implicit resolution layer, producing combinations no one designed.
RSScript avoids this by construction: its features are coordinated projections
of one model — the review protocol — and they are surfaced only through
explicit, named syntax.

A candidate feature is admissible only if both hold:

```text
1. It can be phrased as a reviewer question
   (what mutates, what is retained, who owns a resource, what is fresh,
    what crosses local/managed/native/unsafe, what public behavior changed).
2. It can be expressed with explicit, named, single-canonical syntax,
   without adding an implicit rule to make it ergonomic.
```

If a feature can only be made ergonomic by an implicit mechanism — implicit
conversion, auto method resolution, hidden control flow, positional magic, or
inferred promotion — it is rejected, even when convenient. Convenience bought
with implicitness is how a coherent language slides into feature fights. The
rejected influences in section 20.1 are the first applications of this rule.

---

## 3. v0.5 Scope

### 3.1 Executable MVP

v0.5 executable support is limited to:

```text
managed application code
local values under features: local
fresh struct returns
read / mut / take effects
resource values through with
ResourcePool<T: Resource>
bodyless native declarations through package binding metadata
Rust source lowering with source maps
review map / review diff metadata
```

### 3.2 Review-visible but not executable in v0.5

The following may be parsed and surfaced for review but are not executable lowering targets in v0.5:

```text
async fn bodies
await
spawn
future task runtime
full user-defined enum system beyond standard Option/Result-like variants
general user FFI
advanced protocol/dynamic dispatch model
```

`async fn` signatures are review-visible contracts. Executable async bodies, `await`, and `spawn` must be rejected before lowering in v0.5.

Future executable async is expected to follow the same single-isolate model:
`spawn` means isolate-local cooperative task creation, not Rust `std::thread`
or multi-threaded `tokio::spawn`. Managed values may cross isolate-local
suspension and task boundaries; local values, resources, and runtime read/write
guards may not cross `await`. Cross-isolate or cross-thread work must use an
explicit message/channel API rather than shared managed handles.

### 3.3 Unsupported syntax must not lower to placeholders

Unsupported source must not become generated Rust `todo!()`, silently skipped code, or deferred backend failure. Unsupported constructs require stable frontend diagnostics.

---

## 4. Compilation Architecture and Runtime Target

### 4.1 Pipeline

```text
RSScript source
  -> lexer
  -> parser / AST
  -> HIR
  -> name/type/effect checking
  -> freshness/local/resource checks
  -> review metadata
  -> Rust source lowering
  -> rustc
  -> executable / library
```

`rss run <file.rss>` lowers the file to a Rust package and invokes Cargo on that package. `rss run <package-directory>` uses the package manifest, package source set, and interface environment. For packages with multiple source files, `src/main.rss` is the runnable entry source.

`rss verify-rust <file.rss>` performs the same lowering and asks rustc to check the generated package. With `--out-dir`, the generated package and `rsscript-source-map.json` are retained for inspection.

The RSScript frontend is responsible for RSScript semantics. rustc is responsible for Rust type checking of generated code, optimization, machine code generation, linking, and platform integration.

### 4.2 Generated Rust is typed implementation IR

Generated Rust is:

```text
a deterministic compiler output
a backend target
a snapshot-testable artifact
```

It is not:

```text
a public API
a source format users are expected to edit
RSScript's semantic definition
```

### 4.3 Lowering shape contract

Every RSScript semantic construct must lower to a deterministic, documented Rust shape.

Example contract form:

```text
let binding       -> runtime managed handle binding
local binding     -> owned/local runtime value
with block        -> RAII guard / drop scope
manage expr       -> rss_rt::manage(...)
read arg          -> runtime read view or value copy according to value kind
mut arg           -> runtime write operation according to value kind
take arg          -> Rust move of local value
fn main() -> Unit -> binary harness calling generated library main
```

Lowering must preserve source maps for function definitions, bindings, `with`, `manage`, call-like expressions, field paths, resource drops, and native calls.

### 4.4 Runtime crate target ABI

Generated Rust targets a Rust runtime crate, referred to here as `rss_rt`.

Minimum conceptual surface:

```text
Managed<T> (single-isolate, non-atomic, intentionally !Send/!Sync)
Handle<T>
Local<T> or equivalent generated-owned form
Resource<T> / ResourceGuard<T>
Weak<T>
Result / Error interop
String / Buffer / Bytes bridges
panic/trap conversion
source-span hooks
native function registry
```

For v0.5, `Managed<T>` is part of the single-isolate ABI: it is non-atomic,
intentionally `!Send` and `!Sync`, and valid only inside one RSScript isolate.
Generated Rust must not require or promise ordinary Rust thread sharing for
managed handles.
This is an ABI contract, not merely a reference-runtime implementation detail:
native bindings, lowered Rust, and runtime helpers must treat managed handles as
single-isolate, non-atomic, non-thread-shareable values.

The runtime type surface must be defined before lowering is implemented. A compiler release pins a compatible runtime crate version.

```text
rssc 0.5.x -> rss_rt 0.5.x
```

### 4.5 Managed runtime reference model

Managed values are runtime-mediated handles. The reference v0.5 implementation
is single-isolate and `Rc<RefCell<T>>`-like:

```text
read x  acquires a shared runtime read view
mut x   acquires an exclusive runtime write view
```

RSScript v0.5 exposes a single-isolate source model. Within that model,
frontend-visible conflicts such as same-call `read`/`mut`/`take`/`manage`
overlap are static diagnostics. Managed handles are intentionally not `Send` or
`Sync`; Rust's type system must prevent them from being moved to ordinary
multi-threaded execution. Runtime borrow conflicts that remain after frontend
checking are treated as reentrant managed-access conflicts or internal runtime
failures; they must become RSScript runtime diagnostics with source spans, not
raw Rust panics or deadlocks.

Waiting or serializing ordinary contention is a future cross-thread or
cross-isolate runtime behavior, not a v0.5 source-level promise.

Alternative runtimes may optimize internally only if they preserve RSScript-observable semantics:

```text
aliases observe managed mutation
read/mut do not expose backend-specific borrow errors
runtime failures are reported as RSScript diagnostics
```

RSScript v0.5 has a single-isolate model. Managed handles do not cross isolates.
Future cross-thread or cross-isolate transfer requires explicit message or
channel capabilities rather than implicit shared managed handles.

---

## 5. Expression Modes and Materialization

Expression modes are checker states, not user-written type kinds:

```text
CopyValue
ManagedHandle
FreshShell
LocalValue
ResourceValue
WeakHandle
```

Materialization contexts select representation:

```text
let binding context
local binding context
return context
call-like argument context
with context
container insertion context
approved resource container context
```

A `fresh T` expression denotes an unmaterialized fresh struct shell.

```text
let x = fresh_expr
    materializes as managed T

local x = fresh_expr
    materializes as local T

return fresh_expr from a function returning fresh T
    preserves the fresh return mode

Ok(fresh_expr) / Some(fresh_expr)
    preserves freshness only when the enclosing return type is
    Result<fresh T, E> / Option<fresh T>

read fresh_expr
    materializes a managed temporary

mut fresh_expr / take fresh_expr
    rejected unless the value is explicitly bound as local first
```

These are fixed language contexts, not user-defined conversions.

Canonical call-site syntax uses parentheses when a data-effect wrapper applies to a postfix expression:

```rust
image: read (Image.load(path: read input)?)
```

This avoids ambiguity around `?`, field access, and indexing in review.

---

## 6. Type and Field Model

### 6.1 User-facing type declaration kinds

RSScript has three user-facing type declaration kinds:

```text
class
struct
resource
```

There is no `own struct`.

### 6.2 `class`

A `class` is a managed identity object.

```rust
class User {
    id: UserId
    name: String
}
```

Properties:

```text
always managed
has reference identity
may be shared
may be cyclic
fields are managed or Copy
weak fields may break managed cycles
cannot be local
cannot be fresh
cannot be resource
```

### 6.3 `struct`

A `struct` is a value object.

```rust
struct Image {
    pixels: Buffer
    metadata: handle Map<String, String>
}
```

Properties:

```text
may be managed through let
may be local through local
may be returned fresh
has no observable pointer identity
fields may be inline, handle, or weak handle
```

### 6.4 `resource`

A `resource` requires deterministic cleanup.

```rust
resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}
```

Resources:

```text
must be consumed by with or an approved resource container
cannot be managed
cannot be returned as ordinary values
cannot be stored in ordinary class/struct fields
cannot be captured by managed closures
```

### 6.5 Inline, handle, and weak fields

Every field is one of:

```text
inline
handle
weak handle
```

Inline fields are stored inside their containing value. Copy fields and struct fields are inline by default.

A handle field stores a managed handle. Fields of `class` type are always handles. A struct field can explicitly request a handle:

```rust
struct Config {
    name: String
    rules: handle List<Rule>
}
```

A weak field stores a non-owning managed handle:

```rust
struct Session {
    owner: weak User
}
```

Weak fields:

```text
are handles
do not keep the target object alive
terminate local-inline paths
cannot be taken as inline local fields
may target class types in the MVP
must be explicitly upgraded before use
must be initialized from an explicit weak-handle expression
```

```rust
let session = Session(owner: Weak.from(value: read user))
let maybe_owner = Weak.upgrade(value: read session.owner)
```

Invalid:

```rust
User.log(user: read session.owner)
```

### 6.6 Local structs may contain handles

A local struct can contain managed handles. Those handles remain runtime-managed references and are not hidden inline ownership.

```rust
features: local

local cfg = Config.load(path: read path)?
```

If `cfg.rules` is a handle field, then `cfg.rules` is a managed handle even while `cfg` itself is local.

### 6.7 Resource fields

Resource fields are not allowed in ordinary `class` or `struct` declarations.

Invalid:

```rust
struct Logger {
    file: File
}
```

Use `with` or `ResourcePool<T: Resource>`.

---

## 7. Bindings and World Boundaries

### 7.1 `let`

`let` creates a managed binding for non-Copy values.

```rust
let image = Image.load(path: read path)?
```

For Copy values, the value is copied normally.

### 7.2 `local`

`local` creates a local exclusive binding.

```rust
features: local

local image = Image.load(path: read path)?
```

A local binding:

```text
has exactly one local owner
can be passed as read
can be passed as mut with explicit mut
can be passed as take with explicit take
can be moved into managed runtime with manage
cannot be stored in managed objects
cannot be retained by managed closures or retaining APIs
```

`local` is allowed only with `features: local`.

### 7.3 `with`

`with` introduces a scoped resource binding.

```rust
with File.open(path: read path)? as file {
    File.write(file: mut file, data: read data)?
}
```

The resource is dropped when the block exits.

### 7.4 Managed -> local does not exist

Invalid:

```rust
let image = Image.load(path: read path)?
local working = image
```

Reason:

```text
managed values may have arbitrary aliases
extracting an exclusive local value would be unsafe or require deep clone
```

If a type supports deep cloning into local form, it must expose an explicit API with visible cost:

```rust
local image = Image.deep_clone_to_local(image: read managed_image)
```

### 7.5 `manage`

`manage x` moves a local value into the managed runtime.

```rust
features: local

local image = Image.load(path: read path)?
let shared = manage image
```

Semantics:

```text
requires x to be local
recursively migrates inline local fields
preserves existing handle fields
does not deep-clone managed handles
returns a managed handle
marks x as moved
may allocate
```

`manage` is evaluated before any outer data-effect wrapper:

```rust
Cache.put(value: read (manage image))
```

is equivalent to creating an expression-scoped managed temporary and passing it as `read`.

If migration allocation fails, the current isolate aborts. No rollback is guaranteed and no broken state is exposed.

---

## 8. Places, Conflict Roots, and Same-Call Conflicts

This chapter is the hard boundary for same-call conflict checking.

### 8.1 Place paths

A place path is one of:

```text
base
base.field
base.field.field
base[index]
base.field[index]
base[index].field
```

Field paths contain only field access. Index paths contain at least one index operation.

### 8.2 Conflict root computation

For every call-like argument expression that reads, mutates, takes, manages, or initializes from a place, the checker computes a conflict root.

Rules:

```text
1. Start at the base binding.
2. Inline field access extends the root.
3. A handle field stops root extension; the root includes that handle field.
4. A weak field stops root extension; the root includes that weak field.
5. An index operation stops root extension; the root is the container path before the index.
6. Anything after the first handle / weak / index boundary is ignored for static same-call disjointness.
```

Examples:

```text
state.inner.cache
  root = state.inner.cache        # if all fields are inline

state.cache.entries
  root = state.cache              # if cache is a handle field

state.owner.name
  root = state.owner              # if owner is weak

buffers[0]
  root = buffers                  # index operation reaches the whole container

state.groups[0].name
  root = state.groups             # root stops before [0]
```

### 8.3 Overlap relation

Two roots overlap if:

```text
they are identical
one is a prefix of the other through inline fields
they are truncated to the same handle / weak / index boundary
```

Two local-inline field paths are disjoint only if:

```text
1. they have the same local base
2. neither path is a prefix of the other
3. all fields in both paths are inline
4. after the longest common prefix, the next field differs
```

Allowed:

```rust
Foo.run(
    x: mut state.inner.cache,
    y: mut state.inner.buffer,
)
```

Rejected because of prefix overlap:

```rust
Foo.run(
    x: read state.inner,
    y: mut state.inner.cache,
)
```

Rejected because `cache` is a handle boundary:

```rust
Foo.run(
    x: mut state.cache.entries,
    y: mut state.cache.stats,
)
```

Rejected because index access means whole-container access:

```rust
Foo.run(
    a: mut buffers[0],
    b: mut buffers[1],
)
```

RSScript v0.5 does not prove index inequality.

### 8.4 Same-call compatibility matrix

Within one call-like expression:

```text
read + read on overlapping roots    allowed
read + mut on overlapping roots     rejected
mut + mut on overlapping roots      rejected
take + anything overlapping         rejected
manage + any other use of original  rejected
```

Examples:

```rust
Foo.run(a: read x, b: read x)              // allowed
Foo.run(a: mut x, b: read x)               // error
Foo.run(a: mut x, b: mut x)                // error
Foo.run(a: take x, b: read x)              // error
Foo.run(a: read (manage x), b: read x)     // error
```

Arguments are evaluated in written source order, but this conflict rule is independent of order. Reviewers should not need to reason that one named argument happens to evaluate before another.

### 8.5 Dynamic alias conflicts

The static rule catches syntactic conflicts. If two different managed variables alias the same runtime handle and a conflict is visible only dynamically, the managed runtime must report an RSScript runtime diagnostic rather than deadlocking or surfacing a raw Rust lock/borrow error.

---

## 9. Call-Like Expressions, Constructors, and Variants

This chapter makes the constructor/variant boundary explicit.

### 9.1 Call-like expressions

The following are call-like:

```text
ordinary function calls
native function calls
struct constructors
class constructors
standard enum-like variants such as Ok(...), Err(...), Some(...), None
future user-defined variant constructors
ResourcePool factory calls
immediate resource lease APIs
```

Every call-like expression participates in:

```text
argument name checking where applicable
data-effect checking
same-call conflict-root checking
local move/use checking
resource escape checking
freshness analysis
retention pollution analysis
review metadata classification
```

This is not optional for constructors or variants. `Ok(x)`, `Some(x)`, `Point(x: ..., y: ...)`, and `Session(owner: ...)` cannot be used to hide local moves, retained values, resources, managed closure captures, or conflict roots.

### 9.2 Named arguments

All ordinary function arguments and struct/class constructor fields are named.

```rust
Image.resize(image: mut image, width: 800, height: 600)
let point = Point(x: 1.0, y: 2.0)
```

Invalid:

```rust
Image.resize(mut image, 800, 600)
```

Standard single-payload variants keep their conventional form:

```rust
Ok(value)
Err(error)
Some(value)
None
```

The payload position is still a call-like argument slot for checker purposes.

### 9.3 Constructor field effects

For struct/class constructors:

```text
inline non-Copy local field initialization requires take
handle field initialization from a managed value requires read
weak field initialization requires a weak-handle-producing expression
Copy field initialization copies normally
resource fields are forbidden outside approved resource containers
```

Example:

```rust
features: local

return Ok(Config(
    name: "default",
    rules: read rules,
    workspace: take workspace,
))
```

### 9.4 Variant wrappers do not hide semantics

These shapes remain illegal when the inner value would be illegal directly:

```rust
return Ok(resource)
return Some(resource)
return Ok(local_value_to_retaining_api)
return Some(managed_closure_capturing_local)
```

`Ok(fresh_expr)` and `Some(fresh_expr)` preserve freshness only when the enclosing return type explicitly expects `Result<fresh T, E>` or `Option<fresh T>`.

---

## 10. Data Effects, Retention, and Managed Closure Capture

### 10.1 Data effects

RSScript has exactly three data effects:

```text
read
mut
take
```

There is no `share` data effect.

### 10.2 `read`

A `read` parameter may be inspected.

```rust
fn hash(data: read Bytes) -> UInt64
hash(data: read bytes)
```

For managed values, implementation may acquire a runtime read guard. Such guards are implementation details and cannot escape a function call.

A `read` parameter may not be retained unless `effects(retains(param))` is present.

### 10.3 `mut`

A `mut` parameter may be modified during the call.

```rust
fn resize(image: mut Image, width: Int, height: Int) -> Unit
Image.resize(image: mut image, width: 800, height: 600)
```

For managed arguments, mutation is dynamically shared and aliases observe the change. For local arguments, the checker enforces local exclusivity.

### 10.4 `take`

A `take` parameter consumes a local value.

```rust
features: local

fn consume(buffer: take Buffer) -> Unit
consume(buffer: take buffer)
```

A managed value cannot be passed to `take`. A handle field cannot be taken. A local-inline field path can be taken, after which that path is moved and cannot be used again. Disjoint local-inline fields may remain usable.

`take` is allowed only with `features: local`.

### 10.5 Copy parameters

Copy parameters do not require data effects.

```rust
fn resize(image: mut Image, width: Int, height: Int) -> Unit
```

`width` and `height` are Copy.

### 10.6 Runtime effects and guarantees

Standard effects:

| Effect | Meaning |
|---|---|
| `no_panic` | function will not intentionally panic through known RSScript calls |
| `noalloc` | function performs no obvious heap allocation through supported constructs |
| `no_block` | function does not call known blocking APIs |
| `pure` | no observable external side effects and no reachable managed mutation through known RSScript calls |
| `native` | function crosses a native/Rust implementation boundary |
| `unsafe` | function exposes behavior requiring unsafe review |
| `retains(x)` | function may retain a managed value derived from parameter `x` after returning |

Guarantees are checked only over RSScript-known constructs and trusted signature metadata. They are not whole-program proofs over arbitrary native or runtime internals.

### 10.7 `retains(x)`

`retains(x)` means the function may keep a managed value derived from parameter `x` after returning.

```rust
fn cache_put(cache: mut Cache, key: read String, value: read Image) -> Unit
    effects(retains(value))
```

`retains(x)` may retain a managed handle or managed value derived from `x`. It must not retain an active runtime read/write guard.

A local value cannot be passed directly to a retaining parameter. This includes local-inline fields reached without crossing a handle or weak field.

Correct:

```rust
cache_put(cache: mut cache, key: read key, value: read (manage image))
```

Retention analysis is wrapper-aware. A local value cannot be hidden inside:

```text
Ok(local)
Some(local)
Struct(field: take local) passed to a retaining parameter
read/mut/take wrappers
closure captures
nested constructor or variant payloads
```

### 10.8 Managed closure capture retention

This is a hard implementation boundary.

A closure bound with `let` is a managed closure. A managed closure value may outlive the current expression. Therefore closure creation is a retention boundary for its captures unless the closure is consumed immediately by a `noescape Fn` parameter.

Managed closures may capture:

```text
Copy values
managed values
handle field paths as managed handles
weak field paths as WeakHandle values
```

Managed closures must not capture:

```text
local values
local-inline fields
resources
with-bound resources
runtime read/write guards
```

A managed closure retaining a managed capture must be reported in review metadata as synthetic retention. The closure value retains every non-Copy managed capture until the closure is dropped.

The following forms are retention-equivalent and must not hide captures:

```rust
let cb = || Image.save(image: read image, path: read output)
return Some(cb)
return Ok(cb)
Registry.register(callback: read cb)
Widget(on_click: read cb)
```

If `image` is managed, the closure retains `image`. If `image` is local or a resource, the closure is rejected unless it is a `noescape` temporary.

### 10.9 Noescape and local closure escape hatches

A `noescape Fn(...)` parameter cannot store, return, or retain the closure. Noescape closures may temporarily use local values.

```rust
fn apply(callback: noescape Fn()) -> Unit
```

A closure bound with `local` is a local closure and may move-capture local values, but it is allowed only under `features: local` and cannot become managed, be returned as a managed value, be stored in managed data, or be passed to a retaining parameter.

---

## 11. Freshness

### 11.1 Meaning of `fresh`

`fresh T` means the returned top-level struct shell is newly created and unaliased.

`fresh` is shallow. It does not make handle fields unique.

Legal:

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

Illegal if `User` is a class:

```rust
fn current_user() -> fresh User
```

Resources are not fresh values.

### 11.2 Fresh sources

An expression is fresh if it is one of:

```text
struct constructor expression creating a new shell
call to a function returning fresh T
clean local binding
composition of valid fresh/managed fields into a fresh shell
```

### 11.3 Clean local binding

A local binding is clean if it has not been:

```text
managed with manage
stored into a managed object or managed container
captured by a managed closure
passed to a function that retains it
moved by take
returned previously
assigned into a handle field
wrapped in a constructor or variant that escapes or is retained
```

### 11.4 Fresh analysis

Freshness analysis is intra-procedural. Inter-procedural facts come only from function signatures.

Pseudocode:

```text
is_fresh(expr):
    StructConstructor(fields):
        true if field rules create a fresh shallow shell

    Variant(Ok|Some, inner):
        preserves freshness only in an enclosing fresh Result/Option return mode

    Call(f, args):
        true if f.return_mode == fresh

    LocalVar(x):
        true if x is clean local

    ManagedVar(_):
        false

    FieldAccess(base, field):
        false if field is handle or weak
        true if base is clean local and field is inline, subject to move rules

    ContainerLookup(_):
        false

    GlobalLookup(_):
        false
```

All return branches of a `fresh` function must return fresh values.

---

## 12. Resources, `with`, and `ResourcePool`

### 12.1 Resource values are transient

A resource-producing expression may return `R` or `Result<R, E>` only if the resource value is immediately consumed by an approved resource context.

Legal contexts:

```text
with File.open(...)? as file { ... }
ResourcePool<T>.new(...)
approved resource container insertion
immediate resource lease APIs
```

Invalid:

```rust
let file = File.open(path: read path)?
return Ok(file)
let files = List<File>.new()
```

Canonical syntax for `Result<R, E>` resource producers uses explicit `?`:

```rust
with File.open(path: read path)? as file {
    File.write(file: mut file, data: read data)?
}
```

Compatibility tooling may warn on older `with File.open(...) as file` when the producer returns `Result<R, E>`. v0.6 should require explicit `?` for Result-returning resource producers.

### 12.2 Drop points

A `with` resource is dropped on:

```text
normal block exit
return
break
continue
panic unwind if implementation supports unwinding
```

Inside a `with` block, the resource cannot be:

```text
returned
wrapped in Ok/Some and returned
managed
taken out of the block
stored in a managed object
captured by a managed closure
stored in an ordinary container
```

### 12.3 `ResourcePool<T: Resource>`

`ResourcePool<T: Resource>` is the standard-library escape hatch for long-lived resources.

Hard rules:

```text
ResourcePool itself must be local.
ResourcePool is allowed only with features: local.
ResourcePool is the privileged long-lived resource container in v0.5.
Pool drop releases all held resources.
Borrow returns a with-compatible resource lease.
Resource values cannot escape the pool lease.
```

### 12.4 ResourcePool factory contract

This is a hard implementation boundary.

The v0.5 standard ResourcePool factory is eager and noescape.

Conceptual contracts:

```rust
fn ResourcePool<T: Resource>.new(
    create: noescape Fn(),
    max_size: Int,
) -> fresh ResourcePool<T>
```

`new` is for infallible resource factories. During construction, the runtime may call `create` up to `max_size` times, store the resulting resources inside the local pool, and then discard the factory closure.

Because `create` is not retained by the pool:

```text
managed captures of create are not retained beyond construction
local captures are allowed only under ordinary noescape closure rules
resource or with-bound captures are rejected
review metadata reports ResourcePool construction, but not post-construction factory retention
```

An implementation may not silently change `ResourcePool.new` into a retained or lazy factory API behind the same `.rssi` signature. If a future API retains the factory for lazy replenishment, it must use a distinct name and contract such as `ResourcePool.lazy_new` or `ResourcePool.retained_new`, and it must declare retention with `effects(retains(create))`.

Canonical example:

```rust
features: local

fn run_queries(url: read Url, queries: read List<Query>) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 16,
    )

    for query in queries {
        with ResourcePool.borrow(pool: mut pool) as conn {
            DbConnection.query(conn: mut conn, sql: read query.sql)?
        }
    }

    return Ok(Unit)
}
```

### 12.5 ResourcePool borrow

`ResourcePool.borrow(pool: mut pool)` returns a resource lease that must be consumed by `with`.

```rust
with ResourcePool.borrow(pool: mut pool) as conn {
    DbConnection.query(conn: mut conn, sql: read sql)?
}
```

The lease cannot escape the `with` body, be returned through `Ok`/`Some`, be captured by a managed closure, or be stored in managed data.

---

## 13. Containers

### 13.1 Managed containers

Ordinary containers are managed.

```rust
let images = List<Image>.new()
```

Managed containers may store:

```text
Copy values
managed handles
managed structs
```

They cannot store local values directly.

Correct:

```rust
List.push(list: mut images, value: read (manage image))
```

### 13.2 Local containers

Local containers are advanced standard-library types and require `features: local`.

```rust
features: local

local buffers = LocalVec<Buffer>.new()
```

Local containers may hold local struct values. Container elements do not participate in language-level partial access; element splitting must be expressed through explicit library APIs such as `with_two_mut` or `split_at_mut`.

### 13.3 Resource containers

Only approved resource containers may store resources. In v0.5, the standard resource container is:

```text
ResourcePool<T: Resource>
```

---

## 14. Functions, Effects, Generics, and Interfaces

### 14.1 Function signatures

Public functions must have explicit:

```text
parameter names
parameter types
parameter data effects for non-Copy parameters
return type
guarantee effects if any
native/unsafe/retention effects if any
```

Example:

```rust
pub fn resize(image: mut Image, width: Int, height: Int) -> Unit
    effects(no_panic)
```

### 14.2 Return modes

For non-Copy, non-resource returns:

```text
T                       = managed T
fresh T                 = fresh struct shell
Result<T, E>            = managed T on success
Result<fresh T, E>      = fresh struct shell on success
Option<T>               = managed T on Some
Option<fresh T>         = fresh struct shell on Some
```

For resource returns:

```text
R where R: Resource
    = transient ResourceValue R

Result<R, E> where R: Resource
    = Result containing a transient ResourceValue on success
```

A resource return mode is valid only for resource-producing functions and must be consumed immediately by an approved resource context.

### 14.3 Failure and async

`may_fail` is not an effect. Failure is represented by return type.

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

`async` is a function kind, not an effect. `fresh` is a return contract, not an effect.

Invalid:

```rust
fn load(path: read Path) -> Result<Image, ImageError>
    effects(fresh)
```

### 14.4 Async signatures in v0.5

`async fn` is a review-visible signature boundary. Executable async function bodies, `await`, and `spawn` are unsupported before lowering in v0.5.

Future executable async must not expose Rust's `Future`, `Pin`, `Poll`, `Waker`, executor internals, or lifetime-across-await machinery to RSScript users.

The future execution target is a single-isolate cooperative executor. `spawn`
lowers to an isolate-local task primitive such as `spawn_local`; it must not
imply `Send`, shared heap transfer, or multi-threaded execution. Managed values
may cross isolate-local suspension points, but local values, resources, and
runtime read/write guards may not be live across `await`.

### 14.5 Generics

Generic type parameters default to `Managed`:

```rust
fn first<T>(items: read List<T>) -> Option<T>
```

means:

```rust
fn first<T: Managed>(items: read List<T>) -> Option<T>
```

`T: Managed` means managed-capable:

```text
values of T may appear in managed bindings
values of T may appear in managed containers
values of T may be retained through declared retains effects
values of T may be represented by managed handles when needed
```

Other bounds:

```text
T: Struct    for fresh or local-capable values
T: Resource  for resource generic APIs
```

The recognized generic bounds are exactly `Managed`, `Struct`, and `Resource`;
any other bound is a malformed-generic-parameter diagnostic. A `Copy`-only bound
is not recognized: managed containers already accept `Copy` values under the
default `Managed` bound, so a restrictive `Copy` bound has no current use. If a
genuine `Copy`-only generic API appears, the bound may be added later under the
feature admission rule (section 2.8).

Resource types are not `Managed`. Ordinary `List<T>` cannot be instantiated with resource types.

### 14.6 Protocols are future capability contracts

Terminology note: the future capability-contract feature is named **`protocol`**,
not `interface` and not `trait`. The word "interface" in RSScript refers only to
`.rssi` semantic-contract files (the public signature surface). A `protocol` is a
language-level capability that a type can satisfy. The two are related — a
`protocol` is, in effect, a named bundle of `.rssi`-style effect-carrying method
contracts raised to the type level — but they are not the same thing, and the
shared word must not be reused for the language feature.

A `protocol` is an app-layer capability contract, not a general trait system. It
is not part of the v0.5 executable MVP.

#### Positive model (what a protocol is)

A protocol must satisfy the feature admission rule (section 2.8): it is a
reviewer question ("what capability does this type promise, and what is the
effect of each capability method?") expressed through explicit, named syntax.

```text
nominal            a type satisfies a protocol only by an explicit declaration,
                   never structurally / by accident
effect-carrying    every protocol method declares read/mut/take, return
                   freshness, and retention/guarantee effects, exactly like a
                   .rssi function contract
contract-checked   an implementation is validated against the protocol the same
                   way package .rssi contracts are checked against source today;
                   the protocol feature reuses that checker, it does not invent a
                   new dispatch or resolution mechanism
explicit calls     protocol methods are called in qualified form
                   `Protocol.method(self: <effect> value, ...)`, never
                   `value.method(...)`; there is no auto method resolution
self convention    `self` is a reserved first-parameter name carrying a data
                   effect (`self: mut logger`); user parameters may not be named
                   `self`
```

Excluded permanently (these conflict with sections 2.4 and 2.8):

```text
structural / accidental satisfaction
associated types
blanket impls
specialization
higher-ranked bounds
lifetime bounds
arbitrary where clauses
operator overloading
auto method resolution
protocol inheritance
default method bodies
implicit coercion to a protocol type
```

#### Static dispatch (the default)

A protocol is usable as a generic bound, monomorphized, with no hidden
indirection:

```rust
fn write_line<W: Writer>(writer: mut W, message: read String) -> Unit
```

This covers "write code against a capability" and is fully review-resolvable.

#### Dynamic dispatch (admitted, in a reviewable form)

RSScript admits protocol-typed dynamic dispatch (an open set of implementing
types chosen at runtime). It is a future feature, not part of the v0.5 executable
MVP, but the design decision is settled: dynamic dispatch is supported, because
forbidding it makes users write timidly around capabilities that the review
model can in fact express safely. The constraints below are what make it
reviewable; they are normative for the eventual implementation, not open
questions.

Closed sets should still prefer sealed sum types with exhaustive match
(section 20.1), which are strictly more reviewable. Dynamic dispatch is the
escape hatch for genuinely open sets (runtime-registered plugins, third-party
extensions), not the default tool.

The dynamic-dispatch design must satisfy all of the following. A form that
cannot meet them is not admitted:

```text
1. Only through a protocol whose methods carry full effect contracts. A dynamic
   call's concrete type is unknown but its effects are bounded by the protocol
   contract, so the call's mutation/retention/resource behavior stays known.
2. Coercion to a protocol-typed value is explicit (e.g. an explicit `as Protocol`
   or wrapper construction), never an implicit upcast.
3. Calls stay explicit and qualified: `Protocol.method(self: read value, ...)`.
4. A protocol-typed value is an ordinary managed handle (single-isolate, not
   `Send`); its allocation is the normal managed allocation, not hidden boxing.
5. Review classification: a protocol-dynamic call is `review_if_changed` /
   must-review with effects bounded by the protocol contract. It is NOT `unknown`
   (section 16.5), because the effects are known even though the type is not.
```

This is why dynamic dispatch passes the feature admission rule (section 2.8):
the concrete type is hidden, but the reviewer question — what does this call
mutate, retain, or own — is answered by the protocol's effect contract, and both
coercion and call stay explicit. Review-first does not require knowing the
concrete callee; it requires knowing the effects, and an effect-carrying protocol
provides exactly that.

---

## 15. Native and Unsafe Boundaries

### 15.1 File features

A file without a `features:` declaration is managed-only.

Recognized v0.5 review-relevant file features:

```text
local
native
unsafe
async
```

Reserved future features:

```text
device
ffi
reflection
```

Feature names are semantic capability gates, not library categories. `Json`, `HTTP`, `Image`, and `Regex` are not file features.

Each feature may appear at most once. Duplicate features are diagnostics.

### 15.2 `features: local`

Required for:

```text
local bindings
manage
parameter: take T
local closure
ResourcePool<T: Resource>
local containers
```

### 15.3 `native` effect and declarations

A native function must be declared with `effects(native)` or through a native module declaration.

```rust
features: native

native fn File.open(path: read Path) -> Result<File, IOError>
    effects(native)
```

`native fn` declarations are bodyless in v0.5. A function with an RSScript body may be marked `effects(native)` only when its contract crosses a native boundary through calls or package wrapper bindings.

Native implementations must preserve RSScript semantics:

```text
must not retain local values unless expressed through manage
must not fake fresh values
must not allow resource escape
must translate native panics/errors into RSScript diagnostics or Result errors
must preserve managed handle identity and weak-reference requirements
must preserve source location hooks where applicable
```

### 15.4 `unsafe` boundary

`unsafe` is separate from `native`. A native wrapper may expose safe RSScript contracts, and an unsafe function may be implemented without a native wrapper boundary.

The safe RSScript surface has no specified undefined behavior. Managed aliasing conflicts, resource-pool borrow conflicts, and runtime ownership conflicts must become diagnostics or runtime errors, not unchecked memory behavior.

---

## 16. Review Tools and Review Metadata

### 16.1 Review modes

```text
rss review --diff
rss review --map
```

`rss review --diff` compares two checked RSScript programs and reports semantic changes. `rss review --map` classifies a single file/module/directory by review risk.

### 16.2 Review map categories

Human-facing categories:

```text
entry_point
must_review
review_if_changed
low_semantic_risk
unknown
```

The old skip-safety label is not a v0.5 review-map category. Implementations
must emit `low_semantic_risk`.

### 16.3 Must-review facts

A region is must-review if it contains or exposes:

```text
public API surface
mut parameter
take parameter
manage operation
effects(retains(...))
managed closure capture retention
with resource
ResourcePool
file features local/native/unsafe/async/device/ffi/reflection
native boundary
unsafe boundary
unknown external call
writes to managed state
writes through handle fields
fresh guarantee boundary
runtime guarantee boundary: no_panic/noalloc/no_block/pure
error handling boundary
removed guarantee
```

File-level features add file-level risk:

```text
local        elevated risk
async        elevated risk
native       high risk
unsafe       high risk
device       high risk
ffi          high risk
reflection   elevated risk
```

### 16.4 Low semantic risk

A function may be `low_semantic_risk` only if all of the following hold:

```text
private
not an entry point
no mut parameters
no take parameters
no retains effects
no managed closure capture retention
no with resources
no ResourcePool
no manage operation
no native or unsafe boundary
no unknown calls
no mutation of reachable managed state
no writes through handle fields
all callees are also low-risk or proven safe under the same rules
```

Unknown is never low semantic risk.

Call propagation must use the resolved fully qualified RSScript function name.

### 16.5 Unknown

If the tool cannot classify a region, it must mark it `unknown`. Unknown must not be treated as safe.

An unresolved direct call makes the containing region unknown even when the function is public or otherwise review-required; the public/API reason remains context, but classification is unknown.

Review map JSON should report both line-based and function-based unknown ratios.

---

## 17. Diagnostics and Source Mapping

### 17.1 Required diagnostic classes

Implementations must provide diagnostics for:

```text
use after manage
managed -> local attempt
missing named argument
missing read/mut/take effect
same-call place conflict
constructor/variant call-like conflict
handle-field same-call conflict
retaining local value
managed closure capturing local/resource
managed closure capture retention in retained contexts
fresh function returning aliased value
mut/take of unbound fresh expression
resource escaping with
resource wrapped in Ok/Some and escaping
resource-producing expression used outside resource context
Result-returning resource producer missing explicit ?
invalid resource type in ordinary Result/Option/container context
ResourcePool.new used with fallible factory
ResourcePool factory contract violation
local captured by managed closure
take of handle field
weak field initialized without explicit weak handle
weak field used without explicit upgrade
implicit conversion attempt
operator overload attempt
feature violation
unsupported syntax
async body / await / spawn used in v0.5 executable lowering
async call not consumed by await or spawn
unmappable rustc diagnostic
native boundary violation
```

### 17.2 JSON form

```json
{
  "code": "RS0401",
  "severity": "error",
  "summary": "`image` was moved into the managed runtime by `manage image`.",
  "spans": [
    {
      "file": "image.rss",
      "line": 12,
      "column": 18,
      "length": 12,
      "label": "moved here"
    }
  ],
  "causes": [
    "After `manage`, the local binding no longer exists."
  ],
  "fixes": [
    {
      "kind": "move_use_before_manage",
      "title": "Move this use before `manage image`.",
      "applicability": "machine-applicable"
    }
  ]
}
```

### 17.3 Frontend-first diagnostics

The frontend should catch ordinary user errors before Rust lowering. rustc diagnostics should usually indicate:

```text
compiler bug
runtime crate mismatch
missing native implementation
lowering bug
unexpected backend limitation
```

### 17.4 Source mapping is mandatory

Generated Rust must carry sufficient source mapping for every user-originating construct.

Unmapped diagnostics must be classified by origin:

```text
mapped_user_diagnostic
mapped_backend_diagnostic
unmapped_backend_environment_error
unmapped_native_binding_error
compiler_bug
```

If a diagnostic originates from a user-originating lowered construct and cannot be mapped, classify it as `compiler_bug`.

Raw rustc diagnostics may be attached under a verbose flag but must not be the primary user diagnostic.

---

## 18. Standard Library and Package Boundary

### 18.1 Standard library philosophy

```text
Managed at the surface.
Local in the engine.
Reviewable at the boundary.
```

Simple public APIs should be managed-first:

```rust
let json = Json.parse(text: read body)?
```

Library internals may use local scratch buffers and `*_into` APIs where performance matters.

If a function creates a new struct value, it should return `fresh T`. If it returns an existing shared object, it should return managed `T`.

Functions that store or retain parameters must declare `retains`.

### 18.2 Core signatures

Core APIs are signature-first and should be declared in `.rssi` files before implementations exist.

Minimum core signatures include:

```text
Unit
Bool
Int
Float
String
Bytes
Buffer
Option<T>
Result<T,E>
List<T>
Map<K,V>
Set<T>
Path
File
ResourcePool<T: Resource>
Diagnostic
Span
Log
Test
Assert
```

Agent, GPU, HTTP, networking, and model-client packages are use-case libraries, not language core.

### 18.3 Package manager boundary

RSScript package management is specified separately.

The package manager:

```text
loads .rssi semantic contracts
resolves package dependency graphs
emits semantic review metadata
invokes Cargo for Rust native wrapper implementation builds
```

It must not infer RSScript semantic contracts from Rust signatures and must not weaken language checks such as conflict roots, constructor/variant call-like checking, ResourcePool factory contracts, or managed closure capture retention.

Package features declared in `rsspkg.toml` are package selection features. They are not the same as RSScript file features declared with `features:`.

---

## 19. Examples

### 19.1 File write

```rust
fn write_text(path: read Path, text: read String) -> Result<Unit, IOError> {
    with File.open_write(path: read path)? as file {
        File.write(file: mut file, data: read text)?
    }

    return Ok(Unit)
}
```

### 19.2 Image pipeline

```rust
features: local

fn make_thumbnail(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?

    Image.resize(image: mut image, width: 256, height: 256)
    Image.normalize(image: mut image)

    let shared = manage image
    Image.save(image: read shared, path: read output)?

    return Ok(Unit)
}
```

### 19.3 Cache retention

```rust
class ImageCache {
    entries: Map<String, Image>
}

fn cache_put(cache: mut ImageCache, key: read String, value: read Image) -> Unit
    effects(retains(value))
{
    Map.insert(map: mut cache.entries, key: read key, value: read value)
}
```

With local image:

```rust
cache_put(cache: mut cache, key: read key, value: read (manage image))
```

### 19.4 Config with handle fields

```rust
features: local

struct Config {
    name: String
    rules: handle List<Rule>
    workspace: Buffer
}

fn load_config(path: read Path) -> Result<fresh Config, ConfigError> {
    local workspace = Buffer.new(size: 4096)
    let rules = RuleLoader.load_rules(path: read path)?

    return Ok(Config(
        name: "default",
        rules: read rules,
        workspace: take workspace,
    ))
}
```

### 19.5 Resource pool

```rust
features: local

fn run_queries(url: read Url, queries: read List<Query>) -> Result<Unit, DbError> {
    local pool = ResourcePool<DbConnection>.new(
        create: || DbConnection.open(url: read url),
        max_size: 16,
    )

    for query in queries {
        with ResourcePool.borrow(pool: mut pool) as conn {
            DbConnection.query(conn: mut conn, sql: read query.sql)?
        }
    }

    return Ok(Unit)
}
```

---

## 20. Implementation Roadmap

v0.5 follows a Rust-lowering roadmap.

```text
0.5.0  real AST parser
0.5.1  HIR + symbol table
0.5.2  semantic checker complete enough for examples
0.5.3  lowering shape contract + runtime type surface
0.5.4  Rust source lowering with source maps
0.5.5  rustc diagnostic mapping
0.5.6  runtime crate implementation for managed core
0.5.7  resource/with lowering and runtime
0.5.8  local/fresh/manage lowering and runtime
0.5.9  core .rssi signatures + native core stubs
0.5.10 runnable MVP via rustc
```

Correct dependency order:

```text
runtime type surface
  -> lowering target shapes
  -> Rust source lowering + source maps
  -> rustc diagnostic mapping
  -> runtime implementation fill-in
  -> runnable MVP
```

Do not implement lowering before defining the runtime target surface. Do not defer source mapping until after lowering.

### 20.1 Post-v0.5 design directions

These directions are not part of the v0.5 executable MVP. They are recorded
because they have high future value and are consistent with RSScript's
review-first, no-hidden-machinery model. Several are influenced by Dart, which
demonstrates that an ergonomic managed application surface can be built without
exposing low-level execution machinery to users.

The single-isolate runtime model with non-`Send` managed handles (Chapter 4 and
section 3.2) is the enabling decision for the async and concurrency directions
below: because managed values never move across threads, future tasks never move
across threads either, so RSScript can expose ergonomic async boundaries without
Rust's `Pin`/`Poll`/`Waker` machinery leaking into source.

```text
A. Ergonomic async surface (executable async milestone)
   - Async operation/task handles, if exposed, are isolate-local managed handles,
     not a user-facing Future/Pin/Poll type system.
   - await and a stream / "await for" async-sequence form.
   - single-threaded cooperative executor per isolate.
   - read/mut guards must not be held across await (section 10.2 becomes a
     checker rule when async lowering lands).
   - must not expose Future / Pin / Poll / Waker to RSScript users (section 14.4).

B. Cross-isolate message API with zero-copy transfer
   - explicit typed send/receive channels between isolates.
   - cross-isolate payloads are owned/Copy data or values moved with take.
   - take-based move across an isolate boundary is the no-shared-alias transfer
     path; implementations may make it zero-copy when representation permits.
     Single ownership is enforced statically rather than by runtime convention.
   - managed handles never cross isolates; only explicit messages do.

C. Two-tier execution: dev interpreter + Rust-lowering AOT
   - a HIR-level interpreter for the managed subset for a fast edit-run loop,
     since rustc compilation cost is poor for inner-loop iteration.
   - the Rust-lowering path remains the production/AOT target.
   - both paths must observe identical RSScript semantics and diagnostics.

D. Structured-fix tooling and analysis server
   - an `rss fix` command applying machine-applicable structured fixes
     (section 17.2 already carries fix applicability).
   - a language/analysis server streaming structured diagnostics and fixes,
     serving both human editors and AI repair agents as first-class consumers.

E. User-defined sum types via sealed types + exhaustive match
   - when user-defined variant types are added beyond Option/Result, model them
     on sealed type hierarchies with exhaustive match, not Rust enums with
     lifetime/generic complexity.
   - exhaustiveness is a review property and must be checked before lowering,
     consistent with current Option/Result match exhaustiveness.

F. Registry-level review-risk badges
   - the package registry should surface review-risk signals as first-class
     quality badges: native, unsafe, unknown, mutating/retaining ratios.
   - this reuses the package review metadata already produced by package tooling
     (section 18.3) rather than inventing a separate scoring system.
```

Explicitly rejected influences. The following Dart-style conveniences conflict
with RSScript's review-first and no-hidden-behavior principles (Chapter 2) and
must not be adopted:

```text
cascade operator (..)        hides repeated receiver mutation; RSScript requires
                             visible mut at each call
extension methods /          conflicts with explicit Type.method(self: ...) calls
implicit method resolution   and no auto method resolution
positional records /         conflicts with named-everything canonical style;
implicit flow promotion       any record-like form must use named fields
```

---

## 21. Non-goals

RSScript v0.5 does not attempt to support:

```text
custom VM as primary execution target
custom native backend
LLVM backend
JIT
surface Rust lifetimes
surface & / &mut
Rust-style traits as source semantics
associated types
blanket impls
Rust-style trait objects (implicit coercion, auto method resolution)
Future / Pin / Poll / Waker source model
general user-defined FFI
GPU kernel language
agent runtime as language core
managed -> local demotion
operator-overloaded numeric DSLs
macro-heavy metaprogramming
```

What is excluded is the *Rust-style* trait-object machinery: implicit coercion,
auto method resolution, object safety rules, and type-erased dispatch with no
effect contract. Protocol-typed dynamic dispatch in its explicit, effect-carrying
form is admitted, not excluded (section 14.6).

### 21.1 Deferred, not excluded: managed memory strategy

The v0.5 managed runtime is single-isolate reference counted
(`Rc`/`RefCell`-like). Reference counting does not collect reference cycles on
its own, so v0.5 requires `weak` fields to break managed cycles, the same way
Swift does. This is an accepted v0.5 limitation, not a permanent language
guarantee.

A future major version may add a tracing or moving collector for managed memory
as an alternative backend. The following are therefore **deferred beyond v0.5,
not permanent non-goals**:

```text
tracing collector for managed memory
moving / compacting collector for managed memory
automatic managed-cycle collection (removing the need for weak)
```

This option stays open only while one invariant holds: **managed `class` and
`struct` values have no user-observable destructor**. Deterministic cleanup is
expressed exclusively through `resource`, `with`, and `ResourcePool`, which are
orthogonal to the managed memory strategy. As long as managed objects expose no
user-visible finalization order, "reference counted vs garbage collected" is an
unobservable backend choice and may change between major versions. If managed
objects ever gain side-effecting user destructors, the language would be welded
to reference counting and this option would close.

---

## 22. Reviewer Checklist

Reviewers should evaluate v0.5 by asking:

```text
1. Is RSScript still managed-first?
2. Is local still an explicit capability, not the default world?
3. Are read/mut/take effects visible and canonical?
4. Is retention expressed through effects(retains(...))?
5. Do constructor and variant expressions participate in call-like checking?
6. Are conflict roots hard enough around handle/weak/index boundaries?
7. Are fresh and manage semantics clear?
8. Are ResourcePool factory contracts explicit about eager/noescape versus retained/lazy behavior?
9. Are managed closure captures reported as retention and blocked for local/resource captures?
10. Are partial local access rules implementable?
11. Are container element restrictions conservative enough?
12. Does Rust lowering preserve RSScript diagnostics?
13. Are runtime crate surfaces defined before lowering?
14. Are generated Rust diagnostics source-mapped?
15. Does rss review support both diff and map modes?
16. Is the spec free of domain-specific agent/GPU core pollution?
```

---

## 23. Final Model Summary

```text
one canonical syntax
managed by default
local when performance matters
with for scoped resources
fresh at creation boundaries
manage as one-way local -> managed transition
read/mut/take for parameter behavior
retains for post-call retention
constructors and variants are call-like
conflict roots stop at handle/weak/index boundaries
managed closure captures are retention
ResourcePool factory contracts are explicit
semantic review by diff and map
Rust source lowering as primary backend
```

Or shorter:

```text
Easy by default.
Fast when local.
Reviewable by design.
Compiled through Rust.
```
