# RSScript Language Specification v0.4.1

Status: Draft / Review Candidate  
Version: 0.4.1  
Audience: language designers, compiler implementers, standard-library authors, review-tool authors  
Compatibility note: v0.4.1 is a tightening patch over v0.4. It does not replace the v0.3 semantic core.

---

# 0. Executive Summary

RSScript v0.4.1 is a **managed systems language** designed for the AI-era workflow:

```text
AI writes code.
Humans review code.
The language minimizes review cost.
```

The design target is a three-way balance:

```text
Easy by default.
Fast when local.
Reviewable by one canonical style.
```

RSScript is not "Rust with GC" and not "Python with types".

It is:

```text
a managed-first language
with an explicit local capability
and a review-first surface syntax.
```

v0.4.1 keeps the v0.3 semantic core:

```text
let       = managed binding
local     = local exclusive binding
with      = scoped resource management
fresh     = caller chooses managed/local at creation point
manage    = move local value into managed runtime
managed -> local does not exist
resource  = deterministic cleanup object
```

v0.4.1 tightens v0.4 by removing profile-based syntax variation.

There is no longer:

```text
profile: script
profile: performance
profile: review
```

There is only one canonical surface style.

The only file-level semantic modes are:

```rust
mode: managed
mode: uses-local
```

---

# 1. Why RSScript Exists

Most existing programming languages were designed for:

```text
human writes code
human reviews code
```

In AI-assisted development, this assumption breaks:

```text
AI code generation is cheap.
Human review is still expensive.
Review becomes the bottleneck.
```

Therefore, RSScript optimizes not only for writing code, but for **reviewing generated code at scale**.

This leads to several non-negotiable design choices:

```text
explicit effects
named arguments
no implicit conversions
no user-defined operator overloading
one canonical style
structured diagnostics
review-tool metadata
```

RSScript accepts slightly more verbosity in exchange for reduced ambiguity.

---

# 2. Design Principles

## 2.1 Ease by default

Simple programs should be written in managed style.

```rust
mode: managed

fn main() -> Unit {
    let image = Image.load(path: read "in.png")
    Image.resize(image: mut image, width: 800, height: 600)
    Image.save(image: read image, path: read "out.png")
}
```

The user does not need to understand memory ownership for ordinary code.

---

## 2.2 Fast when local

Performance-sensitive code can opt into local exclusive values.

```rust
mode: uses-local

fn process(path: read Path) -> fresh Image {
    local image = Image.load(path: read path)

    Image.resize(image: mut image, width: 800, height: 600)
    Image.normalize(image: mut image)

    return image
}
```

Local values enable:

```text
static mutation checking
escape analysis
stack or arena allocation
deterministic release of inline storage
clear managed boundary via manage
```

---

## 2.3 Reviewable by one canonical style

RSScript has one canonical surface syntax.

The same operation must not have multiple equivalent spellings.

Non-goal:

```rust
save(image: image, path: out)          // not canonical
save(image: read image, path: read out) // canonical
```

Canonical style:

```rust
Image.save(image: read image, path: read output)
```

---

## 2.4 No hidden behavior

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

If a conversion happens, it is visible.

```rust
let y: Int64 = Int64.from(value: x)
```

---

## 2.5 Public APIs are review contracts

A public API must expose the information a reviewer needs:

```text
argument names
argument types
data effects: read / mut / take
return freshness: fresh or managed
retention effects: retains(param)
guarantees: no_panic / noalloc / pure / no_block
unsafe/native boundaries
```

Public APIs must not rely on inference.

---

## 2.6 Diagnostics are part of the language

Compiler diagnostics are standardized.

They must be useful to both:

```text
human reviewers
AI repair agents
```

Each diagnostic has:

```text
stable error code
human summary
primary source span
causal chain
structured fixes
machine-readable JSON form
```

---

# 3. Version Delta: v0.4 -> v0.4.1

v0.4.1 makes the following normative changes:

| Area | v0.4 | v0.4.1 |
|---|---|---|
| Profiles | `script`, `performance`, `review` | removed |
| Surface syntax | profile-dependent | one canonical style |
| File declarations | `mode` + `profile` | only `mode` |
| Data effects | `read`, `mut`, `share`, `take` | `read`, `mut`, `take` |
| Retention | `share` parameter effect | `effects(retains(param))` |
| Runtime effects | additive: `io`, `allocates`, `may_panic` | reductive guarantees: `no_panic`, `noalloc`, `pure`, `no_block` |
| Naming | profile-dependent argument naming | named arguments required |
| Resource holding | future work | standard `ResourcePool<T: Resource>` |
| Handle fields | brief | dedicated semantic rules |
| Fresh analysis | brief | explicit preservation rules |

---

# 4. File Modes

Every RSScript source file has exactly one semantic mode.

## 4.1 `mode: managed`

Default mode.

```rust
mode: managed
```

Allowed:

```text
let bindings
class values
managed struct values
resource values through with
fresh functions
ordinary public APIs
```

Disallowed:

```text
local bindings
manage
take parameters
local closures
ResourcePool<T>
```

A `mode: managed` file cannot use local capability.

---

## 4.2 `mode: uses-local`

Required if the file uses local capability.

```rust
mode: uses-local
```

Allowed in addition to managed features:

```text
local bindings
manage
take parameters
local closures
ResourcePool<T: Resource>
local containers
```

A file must declare `mode: uses-local` if it contains any of:

```text
local x = ...
manage x
parameter: take T
local closure
ResourcePool<T>
```

---

## 4.3 Mode is not style

Mode describes semantic capability, not formatting style.

There is no profile-dependent syntax.

The same code style is used in all modes.

---

# 5. Type Kinds

RSScript v0.4.1 has three user-facing type declaration kinds:

```text
class
struct
resource
```

There is no `own struct`.

---

## 5.1 `class`

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
fields are managed or copy
cannot be local
cannot be fresh
cannot be resource
```

Example:

```rust
let user = User.new(id: user_id, name: read name)
```

`user` is a managed handle.

---

## 5.2 `struct`

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
fields may be inline or handle
```

A `struct` is the primary type for data.

Examples:

```rust
let image = Image.load(path: read path)       // managed
local image = Image.load(path: read path)     // local
```

---

## 5.3 `resource`

A `resource` is an object requiring deterministic cleanup.

```rust
resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}
```

Properties:

```text
must be used through with or approved resource containers
cannot be managed
cannot be local through ordinary local binding in v0.4.1
cannot be returned as an ordinary value
cannot be stored in class or struct fields
cannot be captured by managed closures
```

Primary usage:

```rust
with File.open(path: read path) as file {
    File.write(file: mut file, data: read data)
}
```

---

# 6. Field Model: Inline vs Handle

RSScript has an explicit field model.

Each field is either:

```text
inline
handle
```

This distinction is essential for local values, `fresh`, and `manage`.

---

## 6.1 Inline fields

A field is inline if it is stored inside its containing value.

Default inline fields:

```text
Copy fields
struct fields
```

Example:

```rust
struct Point {
    x: Float
    y: Float
}

struct Rect {
    origin: Point
    size: Point
}
```

If `Rect` is local, both `origin` and `size` are local inline subvalues.

---

## 6.2 Handle fields

A handle field stores a managed handle.

Fields of `class` type are always handles.

```rust
class User { ... }

struct Session {
    user: User          // class, therefore managed handle
}
```

A `struct` field can be explicitly made a handle:

```rust
struct Config {
    name: String
    rules: handle List<Rule>
}
```

Here:

```text
name  = inline field
rules = managed handle field
```

---

## 6.3 Why `handle T` exists

For `struct` types, the default field mode is inline.

```rust
struct Image {
    pixels: Buffer      // inline Buffer
}
```

Sometimes a struct should store a managed reference instead:

```rust
struct ImageView {
    pixels: handle Buffer
}
```

Difference:

```text
Buffer        = moves with the containing struct
handle Buffer = managed handle preserved across local/manage boundaries
```

---

## 6.4 Local structs may contain handle fields

A local struct can point to managed objects through handle fields.

```rust
local cfg = Config.load(path: read path)
```

If `Config` is:

```rust
struct Config {
    name: String
    rules: handle List<Rule>
    workspace: Buffer
}
```

then:

```text
cfg is local
cfg.name is inline
cfg.workspace is inline local
cfg.rules is a managed handle
```

The runtime must trace `cfg.rules` while `cfg` is alive.

---

## 6.5 `manage` and handle fields

When a local value is moved into managed runtime:

```rust
let shared = manage cfg
```

the runtime:

```text
recursively migrates inline local fields
preserves existing handle fields
does not deep-clone managed handles
marks the local binding as moved
```

For the `Config` example:

```text
cfg shell       -> migrated
cfg.name        -> migrated
cfg.workspace   -> migrated
cfg.rules       -> same managed handle
```

---

## 6.6 Resource fields

Resource fields are not allowed in ordinary `class` or `struct`.

Illegal:

```rust
struct Logger {
    file: File       // error: resource field
}
```

Use `with` or `ResourcePool<T: Resource>`.

---

# 7. Bindings

## 7.1 `let`

`let` creates a managed binding for non-Copy values.

```rust
let image = Image.load(path: read path)
```

For non-Copy structs:

```text
let x = ... means managed x
```

For Copy values:

```rust
let count = 0
```

the value is copied normally.

---

## 7.2 `local`

`local` creates a local exclusive binding.

```rust
local image = Image.load(path: read path)
```

A local binding:

```text
has exactly one local owner
can be passed as read
can be passed as mut with explicit mut
can be passed as take with explicit take
can be moved into managed runtime with manage
cannot be stored in managed objects
cannot be captured by managed closures
```

`local` is allowed only in `mode: uses-local`.

---

## 7.3 `with`

`with` introduces a scoped resource binding.

```rust
with File.open(path: read path) as file {
    File.write(file: mut file, data: read data)
}
```

The resource is dropped when the block exits.

`with` is allowed in both file modes.

---

# 8. Managed and Local Worlds

RSScript has one default world and one local capability.

```text
managed = default world
local   = explicit capability
```

---

## 8.1 Managed values

Managed values:

```text
may be shared
may be stored
may be cyclic
may be mutated dynamically
are traced by the runtime
do not require local mutation markers for safety
```

Even in canonical syntax, `read` and `mut` are review-visible effects, not pointer syntax.

---

## 8.2 Local values

Local values:

```text
are exclusive within their scope
are checked statically for mut/take use
may be optimized aggressively
cannot be retained by managed objects
```

Local values require explicit call-site effects:

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

---

## 8.3 Managed -> local does not exist

There is no conversion from managed to local.

Illegal:

```rust
let image = Image.load(path: read path)
local working = image       // error
```

Reason:

```text
managed values may have arbitrary aliases
extracting an exclusive local value would be unsafe or require deep clone
```

Correct pattern:

```rust
local image = Image.load(path: read path)
```

Choose local at creation time.

---

## 8.4 Deep clone is explicit and library-defined

RSScript does not provide a generic `to_local` or `owned_copy`.

If a type supports deep cloning into local form, it must expose an explicit API:

```rust
local image = Image.deep_clone_to_local(image: read managed_image)
```

The name must make copying cost obvious.

---

# 9. Data Effects

Data effects describe how a function uses a parameter.

RSScript v0.4.1 has exactly three data effects:

```text
read
mut
take
```

There is no `share` data effect.

Retention is expressed by `effects(retains(param))`.

---

## 9.1 `read`

A `read` parameter may be inspected but not mutated and not retained unless `retains(param)` is present.

Signature:

```rust
fn hash(data: read Bytes) -> UInt64
```

Call:

```rust
hash(data: read bytes)
```

---

## 9.2 `mut`

A `mut` parameter may be modified during the call.

Signature:

```rust
fn resize(image: mut Image, width: Int, height: Int) -> Unit
```

Call:

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

For managed arguments:

```text
mutation is dynamically shared
aliases observe the change
```

For local arguments:

```text
call site must use mut
compiler checks exclusive mutable access
```

---

## 9.3 `take`

A `take` parameter consumes a local value.

Signature:

```rust
fn consume(buffer: take Buffer) -> Unit
```

Call:

```rust
consume(buffer: take buffer)
```

After the call:

```rust
use(buffer)      // error: moved
```

`take` is allowed only in `mode: uses-local`.

A managed value cannot be passed to `take`.

---

## 9.4 Copy parameters

Copy parameters do not require data effects.

```rust
fn resize(image: mut Image, width: Int, height: Int) -> Unit
```

`width` and `height` are Copy.

At the call site:

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

---

# 10. Runtime Effects and Guarantees

v0.4.1 uses a **reductive effect model**.

Instead of declaring everything a function may do, functions declare guarantees about what they will not do.

Default assumption:

```text
a function may allocate
a function may panic
a function may block
a function may perform I/O
```

Source-level effects express constraints and special behavior.

---

## 10.1 Effect syntax

```rust
fn hash(data: read Bytes) -> UInt64
    effects(noalloc, no_panic, pure)
{
    ...
}
```

---

## 10.2 Standard guarantees

| Effect | Meaning |
|---|---|
| `no_panic` | function will not intentionally panic |
| `noalloc` | function performs no heap allocation |
| `no_block` | function does not block the current isolate |
| `pure` | no observable external side effects and no mutation of reachable managed state |
| `unsafe` | function contains or exposes unsafe behavior |
| `native` | function crosses native FFI |
| `retains(x)` | function may retain parameter `x` after returning |

---

## 10.3 `pure`

A `pure` function:

```text
does not mutate managed state
does not perform I/O
does not retain parameters
does not depend on mutable global state
```

A `pure` function may allocate unless also marked `noalloc`.

```rust
fn normalize_path(path: read String) -> fresh String
    effects(pure)
```

---

## 10.4 `no_panic`

`no_panic` is a guarantee.

If a `no_panic` function calls a function that may panic, it must handle or eliminate that panic.

```rust
fn index_of(data: read Bytes, byte: UInt8) -> Option<Int>
    effects(no_panic)
```

---

## 10.5 `noalloc`

`noalloc` forbids heap allocation, including managed allocation and local heap allocation.

```rust
fn checksum(data: read Bytes) -> UInt64
    effects(noalloc, no_panic, pure)
```

---

## 10.6 `retains(x)`

`retains(x)` means the function may keep a managed reference derived from parameter `x` after the call returns.

Example:

```rust
fn cache_put(cache: mut Cache, key: read String, value: read Image) -> Unit
    effects(retains(value))
{
    Cache.insert(cache: mut cache, key: read key, value: read value)
}
```

The parameter remains `read` because it is only inspected during the call. The retention behavior is post-call behavior and belongs in `effects`.

---

## 10.7 Passing local values to retaining APIs

A local value cannot be retained directly.

Illegal:

```rust
local image = Image.load(path: read path)

cache_put(
    cache: mut cache,
    key: read key,
    value: read image,       // error: retains(value) cannot retain local
)
```

Correct:

```rust
cache_put(
    cache: mut cache,
    key: read key,
    value: read (manage image),
)
```

`manage image` moves the local value into managed runtime.

---

## 10.8 `may_fail` is not an effect

Failure is represented by the return type.

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

---

## 10.9 `async` is not an effect

`async` is part of the function kind.

```rust
async fn fetch(url: read Url) -> Result<fresh Bytes, NetworkError>
```

---

# 11. Function Signatures

## 11.1 Public functions

Public functions must have explicit:

```text
parameter names
parameter types
parameter data effects for non-Copy parameters
return type
guarantee effects if any
```

Example:

```rust
pub fn resize(image: mut Image, width: Int, height: Int) -> Unit
    effects(no_panic)
```

---

## 11.2 Private functions

Private functions follow the same canonical syntax.

Limited local type inference is allowed for local variables, not for function signatures.

---

## 11.3 Return modes

For non-Copy returns:

```text
T         = managed T
fresh T   = fresh struct shell
Result<T,E> = managed T on success
Result<fresh T,E> = fresh struct shell on success
```

Example:

```rust
fn lookup(cache: read Cache, key: read String) -> Option<Image>
```

Returns a managed `Image` because it comes from cache.

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

Returns a fresh `Image`.

---

# 12. Named Arguments

RSScript uses named arguments as canonical call syntax.

## 12.1 Rule

All non-receiver arguments must be named.

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

Illegal:

```rust
Image.resize(mut image, 800, 600)
```

There are no profile-dependent exceptions.

---

## 12.2 Names prevent AI mistakes

Correct:

```rust
Bank.transfer(from: mut source, to: mut target, amount: money)
```

Wrong argument order is less likely because names are visible.

---

## 12.3 Constructors

Constructors use named fields.

```rust
let point = Point(x: 1.0, y: 2.0)
```

---

## 12.4 Namespaces and functions

RSScript permits namespaced functions:

```rust
Image.load(path: read path)
File.open(path: read path)
```

Dot syntax is namespace access, not method dispatch magic.

No implicit receiver conversion occurs.

---

# 13. Calls and Effects

## 13.1 Read call

```rust
Image.save(image: read image, path: read output)
```

## 13.2 Mutating call

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

## 13.3 Taking call

```rust
Buffer.consume(buffer: take buffer)
```

## 13.4 Managing local at call site

```rust
Cache.store(cache: mut cache, image: read (manage image))
```

---

# 14. `fresh`

`fresh T` means the returned top-level struct shell is newly created and has no aliases.

`fresh` is shallow.

It does not mean every internal handle is unique.

---

## 14.1 Valid fresh return types

`fresh` may be used only with `struct` types.

Legal:

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

Illegal:

```rust
fn current_user() -> fresh User     // error if User is class
```

Resources are not fresh values.

---

## 14.2 Caller selects mode

```rust
let image = Image.load(path: read path)?       // managed
local image = Image.load(path: read path)?     // local
```

---

## 14.3 Shallow freshness

Example:

```rust
struct Pair {
    left: handle Image
    right: handle Image
}

fn pair(left: read Image, right: read Image) -> fresh Pair {
    return Pair(left: read left, right: read right)
}
```

The `Pair` shell is fresh.

The images inside it are managed handles and may be shared.

---

# 15. Fresh-Preservation Analysis

A function declared as returning `fresh T` must pass compiler freshness checking.

The analysis is intra-procedural.

Inter-procedural facts are taken only from function signatures.

---

## 15.1 Fresh expression sources

An expression is fresh if it is one of:

```text
struct constructor expression creating a new shell
call to a function returning fresh T
clean local binding
composition of fresh fields into a fresh shell
```

---

## 15.2 Clean local binding

A local binding is clean if it has not been:

```text
managed with manage
stored into a managed object or container
captured by a managed closure
passed to a function that retains it
moved by take
returned previously
assigned into a handle field
```

---

## 15.3 Fresh analysis pseudocode

```text
is_fresh(expr):
    match expr:
        StructLiteral(fields):
            return all field rules are valid for shallow fresh shell

        Call(f, args):
            return f.return_mode == fresh

        LocalVar(x):
            return is_clean_local(x)

        ManagedVar(_):
            return false

        FieldAccess(base, field):
            if field is handle:
                return false
            if base is clean local and field is inline:
                return true subject to move rules
            return false

        ContainerLookup(_):
            return false

        GlobalLookup(_):
            return false
```

---

## 15.4 Branches

All return branches must return fresh values.

Illegal:

```rust
fn make(cond: Bool, cache: read Cache) -> fresh Image {
    if cond {
        return Image.new(width: 1, height: 1)
    } else {
        return Cache.get(cache: read cache, key: read "default")
    }
}
```

The second branch returns an existing managed value.

---

## 15.5 Closures and freshness

A local captured by a managed closure is not clean.

A local temporarily used by a noescape closure remains clean if not retained.

```rust
noescape_apply(callback: noescape Fn())

local image = Image.load(path: read path)

noescape_apply(callback: || {
    Image.inspect(image: read image)
})

return image      // still fresh if no retention occurred
```

---

## 15.6 Generics and freshness

A generic function returning `fresh T` must require `T: Struct`.

```rust
fn make_default<T: Struct>() -> fresh T
```

If freshness cannot be proven for all valid instantiations, the function is rejected.

---

# 16. `manage`

`manage` moves a local value into the managed runtime.

```rust
local image = Image.load(path: read path)
let shared = manage image
```

After `manage`:

```rust
Image.save(image: read image, path: read output) // error
```

---

## 16.1 Semantics

`manage x`:

```text
requires x to be local
requires no active mut/take use
recursively migrates inline local graph into managed heap
preserves handle fields
returns managed handle
marks x as moved
may allocate
may abort current isolate on allocation failure
```

---

## 16.2 Failure

If migration allocation fails:

```text
the current isolate aborts
no rollback is guaranteed
no broken state is exposed
```

This is intentional.

Transactional `manage` is not part of v0.4.1.

---

## 16.3 Cost

`manage` is not guaranteed O(1).

Cost is proportional to the local inline graph migrated.

Handle fields are not deep-cloned.

---

# 17. Resources and `with`

Resources require deterministic cleanup.

Primary usage:

```rust
with File.open(path: read path) as file {
    File.write(file: mut file, data: read data)
}
```

---

## 17.1 Drop points

A `with` resource is dropped on:

```text
normal block exit
return
break
continue
panic unwind, if implementation supports unwinding
```

---

## 17.2 Resource escape is forbidden

Inside a `with` block, the resource cannot be:

```text
returned
managed
taken out of the block
stored in a managed object
captured by a managed closure
```

Illegal:

```rust
with File.open(path: read path) as file {
    return file
}
```

Illegal:

```rust
with File.open(path: read path) as file {
    let x = manage file
}
```

---

## 17.3 ResourcePool

v0.4.1 includes a standard-library escape hatch for long-lived resources.

```rust
ResourcePool<T: Resource>
```

`ResourcePool` is a privileged standard-library type.

It may hold resource values internally.

User-defined `class` and `struct` types may not directly contain resources.

Example:

```rust
mode: uses-local

local pool = ResourcePool<DbConnection>.new(
    create: || DbConnection.open(url: read url),
    max_size: 16,
)

with ResourcePool.borrow(pool: mut pool) as conn {
    DbConnection.query(conn: mut conn, sql: read sql)
}
```

Rules:

```text
ResourcePool itself must be local
ResourcePool is allowed only in mode: uses-local
borrow returns a with-compatible resource lease
pool drop releases all held resources
resource values cannot escape the pool lease
```

---

# 18. Containers

## 18.1 Managed containers

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

To store a local value:

```rust
List.push(list: mut images, value: read (manage image))
```

---

## 18.2 Local containers

Local containers are advanced standard-library types.

```rust
mode: uses-local

local buffers = LocalVec<Buffer>.new()
```

Local containers may hold local struct values.

They cannot be managed directly unless explicitly converted with `manage`, which migrates their inline graph.

---

## 18.3 Resource containers

Only approved resource containers may store resources.

In v0.4.1:

```text
ResourcePool<T: Resource>
```

is the standard resource container.

---

# 19. Generics

## 19.1 Default bound

Generic type parameters default to `Managed`.

```rust
fn first<T>(items: read List<T>) -> Option<T>
```

means:

```rust
fn first<T: Managed>(items: read List<T>) -> Option<T>
```

`T: Managed` may be:

```text
Copy
class handle
managed struct handle
```

Resource types are not `Managed`.

---

## 19.2 Struct bound

Use `T: Struct` for fresh or local-capable values.

```rust
fn make_pair<T: Struct>(left: read T, right: read T) -> fresh Pair<T>
```

---

## 19.3 Resource bound

Resource generic APIs must be explicit.

```rust
ResourcePool<T: Resource>
```

Ordinary `List<T>` cannot be instantiated with a resource type.

---

## 19.4 Retention with generics

A function retaining a generic parameter must declare it:

```rust
fn store<T: Managed>(box: mut Box<T>, value: read T) -> Unit
    effects(retains(value))
```

---

# 20. Closures

RSScript has three closure categories.

```text
managed closure
local closure
noescape closure
```

---

## 20.1 Managed closure

A closure bound with `let` is managed.

```rust
let callback = || {
    Log.info(message: read "done")
}
```

Managed closures may capture:

```text
Copy values
managed values
```

They may not capture:

```text
local values
resources
with-bound resources
```

---

## 20.2 Local closure

A closure bound with `local` is local.

```rust
mode: uses-local

local buffer = Buffer.new(size: 1024)

local callback = move || {
    Buffer.clear(buffer: mut buffer)
}
```

Local closures may move-capture local values.

---

## 20.3 Noescape closure

A noescape closure cannot be stored or returned.

```rust
fn apply(callback: noescape Fn()) -> Unit
```

Noescape closures may temporarily use local values.

```rust
local image = Image.load(path: read path)

apply(callback: || {
    Image.inspect(image: read image)
})
```

---

# 21. Error Handling

RSScript uses explicit result types for recoverable errors.

```rust
Result<T, E>
Option<T>
```

`may_fail` is not a runtime effect.

Example:

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

Use of `?` is allowed only inside functions returning compatible `Result`.

---

## 21.1 Panic

Panic is unrecoverable by default.

Functions may panic unless marked `no_panic`.

```rust
fn parse(input: read String) -> Result<fresh Ast, ParseError>
    effects(no_panic)
```

---

## 21.2 Exhaustive matching

Pattern matches must be exhaustive.

Wildcard patterns in public code are discouraged and may be linted.

A match over an enum should name all variants unless explicitly marked:

```rust
match result {
    Ok(value) => ...
    Err(error) => ...
}
```

---

# 22. Forbidden Features

RSScript v0.4.1 does not support:

```text
implicit conversion
auto-deref
auto-ref
implicit From / Into chains
user-defined operator overloading
dynamic field creation
monkey patching
custom getter/setter magic
public API type inference
overloaded functions by argument type
macros that hide control flow
method dispatch that changes field-access semantics
borrow-returning public APIs
managed -> local demotion
generic owned_copy
own struct
surface &T / &mut T syntax
```

---

## 22.1 Operators

Operators are limited to built-in types.

Allowed:

```text
numeric + - * /
boolean && ||
comparison for built-in comparable types
```

Disallowed:

```rust
matrix_a + matrix_b
money + tax
```

Use named functions:

```rust
Matrix.add(left: read a, right: read b)
Money.add(left: read price, right: read tax)
```

---

# 23. Diagnostics Protocol

Compiler diagnostics must have both human-readable and machine-readable forms.

---

## 23.1 Human form example

```text
error[RS0401]: `image` was moved into the managed runtime by `manage image`

  image.rss:12:18
    let shared = manage image
                 ^^^^^^^^^^^^ moved here

After `manage`, the local binding no longer exists.

help: move this use before `manage image`
help: if you need a separate managed value, use an explicit deep clone API
```

---

## 23.2 JSON form

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
    },
    {
      "kind": "explicit_deep_clone",
      "title": "Use a type-specific deep clone API.",
      "applicability": "manual"
    }
  ]
}
```

---

## 23.3 Required diagnostic classes

Implementations must provide structured diagnostics for:

```text
use after manage
managed -> local attempt
missing named argument
missing read/mut/take effect
retaining local value
fresh function returning aliased value
resource escaping with
local captured by managed closure
take of handle field
implicit conversion attempt
operator overload attempt
mode violation
```

---

# 24. Formatter, Linter, Review Tool

RSScript tooling is part of the language experience.

---

## 24.1 Formatter

`rss fmt` must be deterministic.

There are no formatting style options in v0.4.1.

The formatter enforces:

```text
canonical named arguments
canonical effect placement
canonical indentation
canonical import ordering
canonical multi-line call formatting
```

---

## 24.2 Linter

`rss lint` enforces:

```text
public API explicitness
forbidden feature checks
wildcard match warnings
unnecessary handle field warnings
unused effects
missing guarantees where configured
```

---

## 24.3 Review tool

`rss review` reports semantic differences between revisions.

It should report:

```text
public API changes
parameter effect changes
return freshness changes
retention changes
guarantee changes
new unsafe/native usage
new local/manage boundary
resource lifetime changes
new inferred allocations or panics, when detectable
callers requiring re-review
```

Example:

```text
review summary:

  cache_put:
    effects: +retains(value)

  normalize:
    parameter image: read -> mut

  parse_config:
    return: Config -> fresh Config

  process_image:
    guarantee removed: noalloc
```

---

# 25. Isolate Model

RSScript v0.4.1 uses a single-isolate runtime model.

```text
each isolate owns one managed heap
managed values do not cross isolates
local values are isolate-local
resources are isolate-local
cross-isolate communication is future work
```

Future versions may add:

```text
Send
Share
channels
message passing
multi-isolate runtime
```

---

# 26. Examples

## 26.1 File write

```rust
mode: managed

fn write_text(path: read Path, text: read String) -> Result<Unit, IOError> {
    with File.open_write(path: read path) as file {
        File.write(file: mut file, data: read text)?
    }

    return Ok(Unit)
}
```

---

## 26.2 Image pipeline

```rust
mode: uses-local

fn make_thumbnail(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?

    Image.resize(image: mut image, width: 256, height: 256)
    Image.normalize(image: mut image)

    let shared = manage image

    Image.save(image: read shared, path: read output)?

    return Ok(Unit)
}
```

---

## 26.3 Cache retention

```rust
mode: managed

class ImageCache {
    entries: Map<String, Image>
}

fn cache_put(cache: mut ImageCache, key: read String, value: read Image) -> Unit
    effects(retains(value))
{
    Map.insert(map: mut cache.entries, key: read key, value: read value)
}
```

Call:

```rust
cache_put(cache: mut cache, key: read key, value: read image)
```

With local image:

```rust
cache_put(
    cache: mut cache,
    key: read key,
    value: read (manage image),
)
```

---

## 26.4 Config with handle fields

```rust
mode: uses-local

struct Config {
    name: String
    rules: handle List<Rule>
    workspace: Buffer
}

fn load_config(path: read Path) -> Result<fresh Config, ConfigError> {
    local workspace = Buffer.new(size: 4096)
    let rules = RuleLoader.load_rules(path: read path)?

    return Config(
        name: "default",
        rules: read rules,
        workspace: take workspace,
    )
}
```

---

## 26.5 Resource pool

```rust
mode: uses-local

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

# 27. Standard Library Guidelines

The standard library must follow canonical style.

## 27.1 Fresh by default for new values

If a function creates a new struct value, it should return `fresh T`.

```rust
Image.load(path: read Path) -> Result<fresh Image, ImageError>
Json.parse(text: read String) -> Result<fresh Json, JsonError>
```

If a function returns an existing shared object, it returns managed `T`.

```rust
Cache.get(cache: read Cache, key: read String) -> Option<Image>
```

---

## 27.2 Retention must be declared

Functions that store or retain parameters must declare `retains`.

```rust
List.push(list: mut List<T>, value: read T) -> Unit
    effects(retains(value))
```

---

## 27.3 Expensive clone APIs must be explicit

Avoid generic names like:

```text
to_local
owned_copy
localize
```

Prefer:

```text
deep_clone_to_local
clone_into_local
copy_pixels_to_local
```

---

## 27.4 Resource management

Resources should primarily be exposed through `with`.

Long-lived resources should use standard abstractions:

```text
ResourcePool<T: Resource>
```

not custom struct fields.

---

# 28. Non-goals of v0.4.1

RSScript v0.4.1 does not attempt to support:

```text
multiple surface styles
Rust-like lifetime syntax
surface & / &mut
full ownership polymorphism
GC -> local demotion
moving GC
thread-shared managed heap
general resource fields
generic resource containers beyond ResourcePool
operator-overloaded numeric DSLs
macro-heavy metaprogramming
implicit typeclass/trait dispatch
```

---

# 29. Implementation Notes

This section is informative, not normative.

A prototype implementation should prioritize:

```text
parser
mode checker
type checker
effect checker
fresh-preservation checker
local move checker
resource/with lowering
structured diagnostics
formatter
review diff tool
```

Code generation can come later.

The first prototype may be a typechecker-only compiler.

---

# 30. Reviewer Checklist

Reviewers should evaluate v0.4.1 by asking:

```text
1. Is there exactly one canonical surface syntax?
2. Are file modes semantic rather than stylistic?
3. Does every public API expose data effects?
4. Are retention effects clearly expressed with retains(param)?
5. Is the reductive effect system understandable?
6. Are handle fields sufficiently clear?
7. Is fresh-preservation implementable?
8. Are resources usable through with and ResourcePool?
9. Are diagnostics structured enough for AI repair tools?
10. Does the language still feel easy in managed mode?
11. Does local mode give a clear performance path?
12. Does the review tool have enough semantic metadata?
```

---

# 31. Final Model Summary

RSScript v0.4.1 can be summarized as:

```text
one canonical syntax
two file modes
three type kinds
three data effects
reductive runtime guarantees
fresh at creation point
manage as one-way boundary
resources through with
retention through effects(retains)
review tooling as part of the language
```

Or, shorter:

```text
Easy by default.
Fast when local.
Reviewable by one canonical style.
```
