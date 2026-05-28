# RSScript (Reviewable System Script) Language Specification v0.5

Status: Draft / Architecture Candidate  
Version: 0.5  
Audience: language designers, compiler implementers, standard-library authors, review-tool authors  
Compatibility note: v0.5 preserves the v0.4.x language core and reorganizes the implementation architecture around **RSScript frontend -> Rust source lowering -> rustc backend**.

---

# 0. Executive Summary

RSScript (**Reviewable System Script**) is a **constrained, review-first source
format for AI-generated systems code with Rust-grade execution**.

Its design target is:

```text
AI codegen target.
Semantic review protocol.
Rust lowering backend.
```

RSScript is:

```text
a source format for generated application-level systems code
with explicit semantic boundaries
machine-readable review artifacts
and Rust-backed execution
```

v0.5 keeps the v0.4.x semantic core:

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

v0.5 makes one major architecture decision:

```text
RSScript lowers to Rust source.
rustc is the backend.
Generated Rust is treated as typed implementation IR.
```

This is the intended implementation architecture until there is evidence that Rust source lowering blocks RSScript semantics.

RSScript's value is in its **semantic review protocol**:

```text
review-first syntax
effect visibility
managed/local boundaries
resource lifetime
freshness
retention
semantic review maps
semantic diffs
structured diagnostics
source-mapped backend diagnostics
```

These are independent of machine-code generation. Rust source lowering lets RSScript invest in the frontend and review model instead of duplicating rustc.

---

# 1. Why RSScript Exists

Most existing languages were designed for:

```text
human writes code
human reviews code
```

In AI-assisted development this assumption breaks:

```text
AI code generation is cheap.
Human review is still expensive.
Review becomes the bottleneck.
```

RSScript is designed for this workflow:

```text
AI writes code.
Humans review semantic changes.
Tools explain risk.
rustc executes the lowered implementation.
```

The language makes review-critical behavior explicit:

```text
mutation
retention
resource lifetime
local performance boundaries
managed/local transitions
freshness guarantees
native/unsafe boundaries
```

RSScript accepts more explicit code in exchange for less ambiguity and stronger
review artifacts.

The language surface is a means to that end. v0.5 should be judged by whether
it reliably produces reviewable contracts, diagnostics, maps, diffs, and
source-mapped Rust execution for AI-generated systems code.

---

# 2. Design Principles

## 2.0 Application Register by Default

RSScript separates the language surface used for ordinary application code from
the Rust surface used for library and native implementation work.

Application code should prefer:

```text
concrete data shapes
named calls
visible mutation
visible retention
managed-by-default values
explicit local hot paths
```

Library implementation work remains available through the Rust backend,
package contracts, and native wrapper boundaries. This keeps the ordinary
review surface smaller without giving up Rust as the systems implementation
substrate.

## 2.1 Ease by default

Ordinary code should be managed and approachable.

```rust
fn main() -> Result<Unit, ImageError> {
    let image = Image.load(path: read "in.png")?
    Image.resize(image: mut image, width: 800, height: 600)
    Image.save(image: read image, path: read "out.png")?
    return Ok(Unit)
}
```

Users should not need ownership or lifetime reasoning for ordinary application code.

---

## 2.2 Fast when local

Performance-sensitive code can opt into local exclusive values.

```rust
features: local

fn process(path: read Path) -> Result<fresh Image, ImageError> {
    local image = Image.load(path: read path)?

    Image.resize(image: mut image, width: 800, height: 600)
    Image.normalize(image: mut image)

    return Ok(image)
}
```

Local values enable:

```text
static local move checking
field-level mutation checking
escape analysis
stack or arena allocation
buffer reuse
clear managed boundary through manage
```

---

## 2.3 Reviewable by one canonical style

RSScript has one canonical surface syntax.

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

If a conversion happens, it is visible:

```rust
let y: Int64 = Int64.from(value: x)
```

---

## 2.5 Public APIs are review contracts

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

---

## 2.6 Diagnostics are part of the language

Diagnostics must serve both:

```text
human reviewers
AI repair agents
```

Each diagnostic has:

```text
stable code
human summary
primary source span
causal chain
structured fixes
machine-readable JSON form
```

---

## 2.7 Rust is a backend, not the language model

RSScript lowers to Rust source while keeping Rust syntax, lifetime parameters,
trait-bound complexity, and borrow-checker diagnostics behind the RSScript
review protocol.

Valid RSScript code should not require the user to understand generated Rust.

The compiler owns RSScript syntax, semantic checks, source-mapped diagnostics,
package contracts, and review metadata. Rust owns backend compilation after
lowering.

---

# 3. Version Delta: v0.4.5 -> v0.5

v0.5 makes the following normative or architectural changes:

| Area | v0.4.5 | v0.5 |
|---|---|---|
| Execution architecture | unspecified / future runtime possible | Rust source lowering is the primary backend |
| VM | possible future work | no custom VM in MVP |
| Backend | unspecified | rustc backend |
| Managed aliasing | managed dynamic semantics | explicitly runtime-mediated, not Rust borrow-checked |
| Rust diagnostics | not specified | must be source-mapped or treated as compiler bugs |
| Lowering | not specified | deterministic lowering shape contract |
| Runtime crate | implied | required target ABI/type surface before lowering |
| Roadmap | interpreter/runtime-oriented | frontend -> lowering -> Rust crate -> rustc |
| Self-hosting | future | self-hosted frontend still emits Rust initially |

No language-level feature is removed from v0.4.5.

---

# 4. Compilation Architecture

## 4.1 Pipeline

The v0.5 compiler pipeline is:

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

`rss run <file.rss>` is a convenience command over this pipeline. It lowers the file to a Rust package and invokes Cargo on that package. `rss run <package-directory>` follows the same pipeline for a package manifest, package source set, and package interface environment; packages with multiple source files use `src/main.rss` as the runnable entry source. By default the package is temporary; `rss run <file.rss> --out-dir <directory>` keeps the generated Rust package for inspection. It is not an interpreter and must not bypass frontend diagnostics, source mapping, or the runtime crate target ABI.

`rss verify-rust <file.rss>` performs the same lowering and checks the generated package with rustc. `rss verify-rust <package-directory>` verifies package lowering the same way. `rss verify-rust <file.rss> --out-dir <directory>` must keep that package, including `rsscript-source-map.json`, so backend diagnostics can be inspected against generated Rust.

The RSScript frontend is responsible for RSScript semantics.

rustc is responsible for:

```text
Rust type checking of generated code
optimization
machine code generation
linking
platform integration
```

rustc is not the primary source of RSScript user diagnostics.

---

## 4.2 Why Rust lowering is normative

RSScript's core value is review-first semantics, not backend technology.

RSScript should not spend early engineering effort reimplementing:

```text
code generation
LLVM integration
borrow checking
platform linker integration
package-level native build machinery
optimizer passes
```

Rust lowering gives RSScript:

```text
world-class backend
crates.io ecosystem
stable deployment story
native performance path
safe systems implementation substrate
```

RSScript remains a separate language because its source semantics, diagnostics, review tooling, and library philosophy are different from Rust.

---

## 4.3 Rust as typed IR

In v0.5, generated Rust should be treated as a **typed implementation IR**.

It is not:

```text
a public API
source users are expected to edit
source users are expected to debug routinely
RSScript's semantic definition
```

It is:

```text
a deterministic compiler output
a backend target
a snapshot-testable artifact
```

---

## 4.4 No custom VM in MVP

RSScript v0.5 does not define a custom bytecode VM.

Non-goals for the MVP:

```text
custom VM
custom GC implementation as primary runtime
LLVM backend
JIT
AOT backend independent of rustc
```

An interpreter may exist for experimentation, but it is not the primary runnable MVP path.

---

# 5. Rust Lowering Shape Contract

## 5.1 Deterministic lowering

Every RSScript semantic construct must lower to a deterministic, documented Rust shape.

Example contract form:

```text
RSScript let binding     -> Rust managed handle binding
RSScript local binding   -> Rust owned/local runtime value
RSScript with block      -> Rust RAII guard / drop scope
RSScript manage expr     -> rss_rt::manage(...)
RSScript read arg        -> runtime read view or cloned handle according to value kind
RSScript mut arg         -> runtime mut operation according to value kind
RSScript take arg        -> Rust move of local value
RSScript fn main() -> Unit -> Rust binary harness calling generated library main
```

This contract makes generated Rust source:

```text
predictable
golden-testable
source-map-friendly
review-tool-friendly
```

---

## 5.2 One-to-one source mapping

The lowering must preserve a stable mapping from RSScript source spans to generated Rust spans.

Generated Rust should include internal span markers or equivalent metadata for:

```text
function definitions
let/local bindings
with blocks
manage expressions
function calls
named arguments
field paths
resource drops
native calls
```

Source mapping must be built into lowering from the first implementation.

It is not a post-processing task.

---

## 5.3 Golden tests

Implementations must maintain lowering golden tests.

For each representative RSScript input, test:

```text
RSScript source -> generated Rust shape
RSScript source -> source map entries
RSScript diagnostics -> RSScript spans
```

Generated Rust may change only when the lowering contract changes intentionally.

---

## 5.4 Generated Rust is private

Generated Rust is not the user-facing source format.

Users may inspect it for debugging, but RSScript compatibility is defined by RSScript source behavior, not generated Rust details.

---

# 6. Runtime Crate Target ABI

## 6.1 `rss_rt`

RSScript lowering targets a Rust runtime crate, referred to here as `rss_rt`.

The runtime crate provides the type surface used by generated Rust.

The type surface must be defined before lowering is implemented.

---

## 6.2 Runtime crate surface before implementation

The runtime crate should be split into:

```text
runtime type surface
runtime implementation
```

The type surface is a compile target for lowering and must exist first.

The implementation can be filled in incrementally.

Minimum surface:

```text
Managed<T>
Handle<T>
Local<T> or equivalent generated-owned form
Resource<T> / ResourceGuard<T>
Result / Error interop
String / Buffer / Bytes bridges
panic/trap conversion
source-span hooks
native function registry
```

The exact Rust names are implementation details, but the conceptual roles are normative.

---

## 6.3 Runtime ABI stability

Generated Rust depends on `rss_rt` as an internal ABI.

The ABI may change between compiler versions before stabilization, but each compiler release must pin a compatible `rss_rt` version.

```text
rssc 0.5.x -> rss_rt 0.5.x
```

---

## 6.4 Core library signatures vs runtime implementation

RSScript core APIs are defined by `.rssi` interface files.

Rust runtime/native implementations must conform to those signatures.

RSScript signatures are the public contract. Rust implementation details are not.

---

# 7. Managed Runtime Semantics

## 7.1 Managed values

Managed values are the default for non-Copy values created with `let`.

They:

```text
may be shared
may be stored
may be cyclic
may be mutated through mut APIs
are runtime-managed
```

Managed values are not Rust-owned values exposed to RSScript users.

---

## 7.2 Managed aliasing is runtime-mediated

In v0.5, managed value aliasing and mutation are mediated by the RSScript runtime, not by Rust's source-level borrow checker.

This is an explicit design decision.

Reason:

```text
RSScript managed semantics allow shared object graphs.
Rust ownership semantics do not directly model that surface language.
```

The v0.5 runtime target is intentionally simple: managed handles may be
implemented as reference-counted, lock-mediated Rust values such as
`Arc<RwLock<T>>`. This allows managed handles to be shared across Rust threads
when the lowered Rust types satisfy Rust's ordinary thread-safety rules.
RSScript v0.5 does not require a custom global tracing heap, moving collector,
or actor runtime to make that work.

Reference-counting means strong cycles are representable but not automatically
collected. Cyclic identity graphs should use `weak` fields at ownership
back-edges so the cycle is review-visible.

Therefore, generated Rust will usually represent managed values through runtime handles rather than direct Rust references.

---

## 7.3 What rustc checks and does not check

rustc may check generated Rust implementation correctness.

rustc is not expected to enforce RSScript's managed aliasing rules directly.

RSScript frontend must enforce:

```text
read / mut / take call-site correctness
retains(local) errors
resource escape errors
freshness errors
local move/use errors
handle field restrictions
partial local access restrictions
```

Managed dynamic mutation is a runtime model.

---

## 7.4 Semantic guarantee table

The table below is normative for the v0.5 implementation target. It prevents
the source language from promising more than the compiler/runtime can currently
prove.

Status meanings:

```text
static       checked by the RSScript frontend before Rust lowering
dynamic      checked by RSScript runtime hooks/handles at execution time
review-only  surfaced for human review, but not fully proven by the compiler
unsupported  rejected before Rust lowering or reserved for later versions
```

| Surface | v0.5 meaning | Current enforcement |
| --- | --- | --- |
| `read x` call-site effect | Callee may inspect `x`. It is not a snapshot guarantee. For managed handles this is a runtime read view; for plain lowered values it is a Rust shared borrow. | static argument/effect checking; dynamic managed read conflict reporting where `Managed<T>` read guards are used |
| `mut x` call-site effect | Callee may mutate `x`. For managed handles, mutation must go through the runtime handle and require dynamic exclusive write access. | static argument/effect checking; dynamic managed write conflict reporting where `Managed<T>` write guards are used |
| `take x` call-site effect | Callee consumes a local/owned value. It is not valid for managed handles or handle fields. | static checking for parameter effect, managed value take, handle-field take, and local move/use |
| managed sharing | Managed values may be shared, stored, and cyclic. Strong cycles are representable and must use `weak` review markers at back edges when collection matters. | dynamic `Arc<RwLock<T>>` handle semantics for `Managed<T>`; weak handles implemented; cycle collection unsupported |
| managed alias observes mutation | Aliases of the same managed handle observe mutation through the runtime handle. Ordering is the ordering of the generated Rust execution plus the runtime lock implementation; no stronger memory model is promised. | dynamic for `Managed<T>` aliases; not a compile-time proof |
| managed -> local | A managed value cannot be silently recovered as a local exclusive value, including through `read` or `mut` wrappers. | static |
| `manage x` | Moves a local value into a managed runtime handle; the local binding is no longer usable. | static move/use checking plus generated runtime handle creation |
| `effects(retains(x))` | Function may store a managed reference derived from `x` after return. Retaining a clean local value without `manage` is forbidden. | static for declared retained parameters and known builtin/core signatures |
| resource lifetime / `with` | Resource values must stay scoped to `with` and must not escape by return, managed storage, retention, or closure capture. | static for implemented escape shapes; `ResourcePool<T>` is the privileged long-lived resource container |
| `fresh T` | Returned top-level struct shell is newly created and unaliased; the guarantee is shallow and does not make handle fields unique. | static freshness analysis for supported constructors, fresh calls, branches/loops, handle-field restriction, and retained/managed-local pollution |
| local partial access | Disjoint local fields may be accessed independently until a handle field or indexed container boundary is reached. | static for supported path shapes |
| managed closure capture of local | Managed closures may not capture clean local values that could outlive the local region. | static for supported closure shapes; `noescape Fn()` is the supported temporary-callback escape hatch |
| `features: local` | Enables local ownership features (`local`, `manage`, `take`, `ResourcePool`, local closures). | static file-level gate |
| `features: native` | Marks a native/Rust boundary. Bodyless native declarations are allowed through binding metadata; executable native bodies are not part of v0.5. | static gate plus package/native binding checks; executable body unsupported |
| `features: unsafe` | Marks an explicit hazard boundary. It is separate from native and does not become a normal next layer. | review boundary and static feature gate for unsafe effects/native metadata; safe RSScript/runtime crates forbid Rust `unsafe` internally |
| `async fn` | Async signatures are visible to review and interface diffing. Executable async bodies are not part of the v0.5 runtime. | static feature gate; executable body unsupported |
| `effects(no_panic)` | Function promises not to intentionally panic through known RSScript calls. This is not a whole-program proof over arbitrary native/runtime internals. | static over resolved constructors/enum variants/functions with matching guarantees; native/runtime trust boundary is review-required |
| `effects(no_block)` | Function promises not to call known blocking APIs. This is not a scheduler or OS-level proof. | static over resolved constructors/enum variants/functions with matching guarantees; native/runtime trust boundary is review-required |
| `effects(noalloc)` | Function promises no obvious heap allocation through supported RSScript constructs and calls. It is not a global allocator trace. | static for constructors, `manage`, and resolved calls with matching guarantee |
| `effects(pure)` | Function promises no observable external side effects and no reachable managed mutation through known RSScript calls. | static for mut params, retention effects, and resolved calls with matching guarantee; native/runtime trust boundary is review-required |
| Rust lowering + source map | Generated Rust is typed implementation IR. Backend diagnostics should map back to RSScript spans when possible. | static lowering gate, source map emission, rustc JSON remap; unmappable diagnostics are reported explicitly |
| review map/diff | Review tools classify semantic risk; `unknown` must not be treated as safe. | implemented as review metadata/diff/map; still conservative and not a proof of behavioral equivalence |
| unsupported syntax | Unsupported source must not become generated Rust `todo!()` or silently skipped semantics. | static diagnostics for known unsupported constructs; parser completeness is still a hardening area |

Open hardening requirement: unknown top-level constructs, malformed
declarations, malformed `let`/`local` bindings, missing call argument values,
trailing expression tokens, and unclosed function/call delimiters are stable
diagnostics. Broader expression/body parse errors still need grammar-driven
diagnostics before v0.5 can be called semantically hard.

---

## 7.5 Runtime conflicts

RSScript managed mutation semantics are dynamically shared.

Aliases observe mutation.

Implementations may use runtime interior mutability, copy-on-write, cell-like storage, or another safe strategy.

If an implementation uses dynamic borrow guards internally, valid RSScript programs should not expose raw Rust borrow errors to the user.

Any runtime conflict must be reported as an RSScript runtime diagnostic with RSScript source location, not as a Rust panic pointing into generated code.

---

## 7.6 No Rust lifetime leakage

Generated Rust must not leak lifetime parameters, `RefCell`, `Rc`, `Arc`, `Mutex`, or other backend representation details into RSScript diagnostics unless explicitly marked as an internal compiler/runtime error.

RSScript users review RSScript semantics, not Rust lowering mechanics.

---

# 8. File Features

A file may declare advanced review-relevant capabilities.

If omitted, the file is managed-only and enables no advanced capability.

---

## 8.1 Omitted features

A file without a `features:` declaration is managed-only:

```rust
```

This lowers entry friction for ordinary scripts.

Each feature may appear at most once in a `features:` declaration. Duplicate feature names are diagnostics rather than silently ignored, because the header is a review capability boundary.

---

## 8.2 Default Managed File

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

---

## 8.3 `features: local`

Required if a file uses local capability.

```rust
features: local
```

Required for:

```text
local x = ...
manage x
parameter: take T
local closure
ResourcePool<T: Resource>
local containers
```

Features describe semantic capability, not style.

---

## 8.4 Review Boundary Features

The MVP recognizes these feature names as review-relevant capability gates:

```text
local
native
unsafe
async
```

`local` enables local ownership features.

`native`, `unsafe`, and `async` are boundary declarations. A file must declare the matching feature before it can contain `native` boundaries, `unsafe` effects, or `async fn` declarations.

`unsafe` is separate from `native`. A native wrapper may expose only safe RSScript
contracts, and an unsafe function may be implemented without crossing a native
wrapper boundary. When both appear, `features: native, unsafe` means the file
crosses a native boundary and also exposes behavior that requires unsafe review.

The safe RSScript surface has no specified undefined behavior. Managed aliasing,
mutation conflicts, resource-pool borrow conflicts, and runtime ownership
conflicts must become diagnostics or runtime errors, not unchecked memory
behavior. The reference compiler and runtime forbid Rust `unsafe` internally;
native wrappers and `effects(unsafe)` are explicit review boundaries outside this
safe surface.

The following feature names are reserved for future review-relevant capabilities:

```text
device
ffi
reflection
```

They are reserved because they may affect review risk, checker behavior, source mapping, or runtime boundaries.

Ordinary library areas are not features:

```text
Json
File
Map
HTTP
Image
Regex
```

Do not add a feature unless it:

```text
opens a capability managed-only files cannot use
changes review risk
requires compiler or checker handling
can be declared clearly at file scope
preserves canonical style
```

There is only one canonical surface style.

---

# 9. Type Kinds

RSScript has three user-facing type declaration kinds:

```text
class
struct
resource
```

There is no `own struct`.

---

## 9.1 `class`

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

---

## 9.2 `struct`

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

---

## 9.3 `resource`

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
must be used through with or approved resource containers
cannot be managed
cannot be returned as ordinary values
cannot be stored in ordinary class/struct fields
cannot be captured by managed closures
```

Primary usage:

```rust
with File.open(path: read path) as file {
    File.write(file: mut file, data: read data)
}
```

---

# 10. Field Model: Inline vs Handle

Every field is either:

```text
inline
handle
weak handle
```

---

## 10.1 Inline fields

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

## 10.2 Handle fields

A handle field stores a managed handle.

Fields of `class` type are always handles.

```rust
class User { ... }

struct Session {
    user: User
}
```

A `struct` field can be explicitly made a handle:

```rust
struct Config {
    name: String
    rules: handle List<Rule>
}
```

## 10.2.1 Weak handle fields

A weak field stores a non-owning managed handle.

```rust
class User { ... }

struct Session {
    owner: weak User
}
```

Properties:

```text
weak fields are handles
weak fields do not keep the target object alive
weak fields terminate local-inline paths
weak fields cannot be taken as inline local fields
weak fields are for managed class identity objects
```

In the MVP, `weak` fields may only target `class` types. This keeps weak references tied to the managed identity-object model and avoids implying weak ownership for inline value structs.

Generated Rust lowers a weak field to the runtime weak-handle surface, not to a direct Rust reference.

---

## 10.3 Local structs may contain handle fields

A local struct can point to managed objects through handle fields.

```rust
local cfg = Config.load(path: read path)?
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

Live local values may contain managed handles, but those handles remain explicit
runtime-managed references rather than hidden traced pointers.

---

## 10.4 `manage` and handle fields

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

---

## 10.5 Resource fields

Resource fields are not allowed in ordinary `class` or `struct`.

Illegal:

```rust
struct Logger {
    file: File
}
```

Use `with` or `ResourcePool<T: Resource>`.

---

# 11. Bindings

## 11.1 `let`

`let` creates a managed binding for non-Copy values.

```rust
let image = Image.load(path: read path)?
```

For Copy values, the value is copied normally.

---

## 11.2 `local`

`local` creates a local exclusive binding.

```rust
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
cannot be captured by managed closures
```

`local` is allowed only with `features: local`.

---

## 11.3 `with`

`with` introduces a scoped resource binding.

```rust
with File.open(path: read path) as file {
    File.write(file: mut file, data: read data)?
}
```

The resource is dropped when the block exits.

`with` is allowed in managed-only files and files with advanced features.

---

# 12. Managed and Local Worlds

RSScript has one default world and one local capability.

```text
managed = default world
local   = explicit capability
```

---

## 12.1 Managed values

Managed values:

```text
may be shared
may be stored
may be cyclic
may be mutated dynamically
are held through runtime-managed reference handles
```

Even in canonical syntax, `read` and `mut` are review-visible effects, not pointer syntax.

---

## 12.2 Local values

Local values:

```text
are exclusive within their scope
are checked statically for mut/take use
may be optimized aggressively
cannot be retained by managed objects
```

---

## 12.3 Managed -> local does not exist

Illegal:

```rust
let image = Image.load(path: read path)?
local working = image
```

Reason:

```text
managed values may have arbitrary aliases
extracting an exclusive local value would be unsafe or require deep clone
```

Correct:

```rust
local image = Image.load(path: read path)?
```

Choose local at creation time.

---

## 12.4 Deep clone is explicit and library-defined

RSScript does not provide a generic `to_local` or `owned_copy`.

If a type supports deep cloning into local form, it must expose an explicit API:

```rust
local image = Image.deep_clone_to_local(image: read managed_image)
```

The name must make copying cost obvious.

---

# 13. Data Effects

RSScript has exactly three data effects:

```text
read
mut
take
```

There is no `share` data effect.

Retention is expressed with `effects(retains(param))`.

---

## 13.1 `read`

A `read` parameter may be inspected.

```rust
fn hash(data: read Bytes) -> UInt64
hash(data: read bytes)
```

A `read` parameter may not be retained unless `retains(param)` is present.

---

## 13.2 `mut`

A `mut` parameter may be modified during the call.

```rust
fn resize(image: mut Image, width: Int, height: Int) -> Unit
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
compiler checks local exclusivity
```

---

## 13.3 `take`

A `take` parameter consumes a local value.

When `take` consumes a local-inline field path, that path is moved and cannot be
used again. Disjoint inline fields of the same local struct remain usable.

```rust
fn consume(buffer: take Buffer) -> Unit
consume(buffer: take buffer)
```

After the call, `buffer` is moved.

`take` is allowed only with `features: local`.

A managed value cannot be passed to `take`.

---

## 13.4 Copy parameters

Copy parameters do not require data effects.

```rust
fn resize(image: mut Image, width: Int, height: Int) -> Unit
```

`width` and `height` are Copy.

---

# 14. Field-Level Effects and Partial Local Access

Field-level access is intentionally conservative.

The goal is:

```text
allow obvious disjoint local field mutation
avoid full Rust-style borrow complexity
avoid container index analysis
```

---

## 14.1 Place paths

A place path is one of:

```text
base
base.field
base.field.field
base[index]
base.field[index]
base[index].field
```

Field paths contain only field access.

Index paths contain at least one index operation.

---

## 14.2 Local-inline field paths

A field path is local-inline if:

```text
base is a local binding
all accessed fields are inline fields
no handle field appears in the path
no index operation appears in the path
```

Example:

```rust
state.inner.cache
```

is local-inline if `state`, `inner`, and `cache` are inline local fields.

---

## 14.3 Disjoint nested field paths

Two local-inline field paths are disjoint if:

```text
1. they have the same local base
2. neither path is a prefix of the other
3. all fields in both paths are inline fields
4. after their longest common prefix, the next field component differs
```

Allowed:

```rust
Foo.run(
    x: mut state.inner.cache,
    y: mut state.inner.buffer,
)
```

Allowed:

```rust
Foo.run(
    x: mut state.parser.tokens,
    y: mut state.codegen.output,
)
```

---

## 14.4 Prefix conflicts

A whole prefix conflicts with a subpath.

Illegal:

```rust
Foo.run(
    x: mut state.inner,
    y: mut state.inner.cache,
)
```

Illegal:

```rust
Foo.run(
    x: read state.inner,
    y: mut state.inner.cache,
)
```

Reason:

```text
read state.inner may observe state.inner.cache
mut state.inner.cache modifies the same reachable subvalue
```

---

## 14.5 Handle fields terminate local-inline paths

A handle field is not an inline subvalue.

If a path reaches a handle field, local partial access stops there.

Example:

```rust
struct State {
    cache: handle Cache
}
```

The following cannot be approved by local field disjointness:

```rust
Foo.run(
    x: mut state.cache.entries,
    y: mut state.cache.stats,
)
```

`state.cache` is a managed handle.

---

## 14.6 Container elements are whole-container access

Any path containing an index operation is treated as access to the whole container for local alias checking.

Illegal in v0.5:

```rust
local buffers = LocalVec<Buffer>.new()

Foo.run(
    a: mut buffers[0],
    b: mut buffers[1],
)
```

Even literal indices are not special-cased.

RSScript v0.5 does not prove index inequality.

---

## 14.7 Explicit split APIs

Disjoint element access must be expressed through explicit library APIs.

Example:

```rust
LocalVec.with_two_mut(
    vec: mut buffers,
    first: 0,
    second: 1,
    f: noescape Fn(a: mut Buffer, b: mut Buffer) -> Unit,
)
```

or:

```rust
LocalVec.split_at_mut(
    vec: mut buffers,
    index: split,
    f: noescape Fn(left: mut LocalSlice<Buffer>, right: mut LocalSlice<Buffer>) -> Unit,
)
```

The language core does not perform index disjointness analysis.

---

# 15. Runtime Effects and Guarantees

RSScript uses a reductive effect model.

Default assumption:

```text
a function may allocate
a function may panic
a function may block
a function may perform I/O
```

Source effects express constraints and special behavior.

---

## 15.1 Effect syntax

```rust
fn hash(data: read Bytes) -> UInt64
    effects(noalloc, no_panic, pure)
{
    ...
}
```

---

## 15.2 Standard effects

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

## 15.3 `retains(x)`

`retains(x)` means the function may keep a managed reference derived from parameter `x` after returning.

```rust
fn cache_put(cache: mut Cache, key: read String, value: read Image) -> Unit
    effects(retains(value))
```

A local value cannot be passed directly to a retaining parameter. This includes
local-inline fields reached without crossing a `handle` or `weak` field.

Correct:

```rust
cache_put(cache: mut cache, key: read key, value: read (manage image))
```

---

## 15.4 Failure and async

`may_fail` is not an effect.

Failure is represented by return type:

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

`async` is function kind, not an effect.

`fresh` is also not an effect. Freshness is a return contract and must be written in the return type:

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

Invalid:

```rust
fn load(path: read Path) -> Result<Image, ImageError>
    effects(fresh)
```

---

## 15.5 Async execution model, future executable subset

In the current v0.5 implementation target, `async fn` is a review-visible
signature boundary and executable async bodies are unsupported. The first
executable async subset must stay narrow and review-first.

RSScript async must not expose Rust's `Future`, `Pin`, `Poll`, `Waker`, executor
internals, or lifetime-across-await machinery to RSScript users.

Async is:

```text
visible suspension boundary
visible task boundary
visible non-blocking app-layer call model
```

It is not a general Future type-system surface.

Any executable async function body, `await`, or `spawn` must require:

```rust
features: async
```

If local values are also used:

```rust
features: async, local
```

An async function:

```rust
async fn fetch_user(
    client: read HttpClient,
    id: read UserId,
) -> Result<fresh User, HttpError>
```

means the function produces a logical result of that return type when awaited.
RSScript users do not write `Future<T>`.

Async calls must be consumed by `await` or `spawn`:

```rust
let user = await fetch_user(client: read client, id: read id)?
let task = spawn fetch_user(client: read client, id: read id)
let user = await task?
```

This is invalid:

```rust
let user = fetch_user(client: read client, id: read id)
```

Diagnostic:

```text
async call must be awaited or spawned
```

`await expr?` is parsed as:

```text
(await expr)?
```

`spawn` is an explicit task and retention boundary. It may retain non-Copy
arguments until task completion. It may capture managed values and Copy values.
It must not capture local values, local-inline fields, resources, or with-bound
resources. To pass a local value to a spawned task, source must first cross the
review-visible boundary:

```rust
let shared = manage local_value
let task = spawn work(value: read shared)
```

Implicit fire-and-forget is not part of the first executable async subset.
Detached tasks require an explicit API such as:

```rust
Task.detach(task: take task)
```

and should be elevated review risk because the task may outlive the caller.

Async bodies must not directly call sync functions unless those functions are
known constructors, enum variants, or declared `effects(no_block)`.

Local values must not be live across `await`. Resources must not cross `await`.
In particular, `await` inside an ordinary `with` resource scope is not allowed
in this subset. Future versions may introduce `async with`, but v0.5 does not.

Managed values may be used before and after `await`, and async callees may take
`mut` managed parameters. The caller must not hold a managed read/write runtime
guard across `await`.

Review tools should mark these as must-review:

```text
public async entry point
await native async call
spawn task
detached task
async call with mut parameter
async call with retains
async function calling unresolved or non-no_block sync function
```

Unknown async callees, incomplete native async metadata, and unmappable backend
async diagnostics must be classified as unknown, not safe.

---

# 16. Function Signatures

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

Private functions follow canonical syntax.

---

## 16.1 Return modes

For non-Copy returns:

```text
T                       = managed T
fresh T                 = fresh struct shell
Result<T, E>            = managed T on success
Result<fresh T, E>      = fresh struct shell on success
```

Example:

```rust
fn lookup(cache: read Cache, key: read String) -> Option<Image>
```

returns a managed `Image`.

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

returns a fresh `Image`.

---

## 16.2 Signature complexity budget

Public signatures should remain reviewable in one screen.

Linters should warn on:

```text
too many generic parameters
deeply nested types
long effect clauses
complex aliases hiding behavior
public signatures requiring implementation knowledge
```

RSScript rejects Rust-style lifetime parameters in user-facing signatures.

---

# 17. Named Arguments

All non-receiver arguments must be named.

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

Illegal:

```rust
Image.resize(mut image, 800, 600)
```

Constructors use named fields:

```rust
let point = Point(x: 1.0, y: 2.0)
```

Dot syntax is namespace access, not method dispatch magic.

---

# 18. Calls and Effects

## 18.1 Read call

```rust
Image.save(image: read image, path: read output)
```

## 18.2 Mutating call

```rust
Image.resize(image: mut image, width: 800, height: 600)
```

## 18.3 Taking call

```rust
Buffer.consume(buffer: take buffer)
```

## 18.4 Managing local at call site

```rust
Cache.store(cache: mut cache, image: read (manage image))
```

---

# 19. `fresh`

`fresh T` means the returned top-level struct shell is newly created and has no aliases.

`fresh` is shallow.

It does not mean every internal handle is unique.

---

## 19.1 Valid fresh return types

`fresh` may be used only with `struct` types.

Legal:

```rust
fn load(path: read Path) -> Result<fresh Image, ImageError>
```

Illegal:

```rust
fn current_user() -> fresh User
```

if `User` is a `class`.

Resources are not fresh values.

---

## 19.2 Caller selects ownership capability

```rust
let image = Image.load(path: read path)?
local image = Image.load(path: read path)?
```

---

## 19.3 Shallow freshness

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

The images inside are managed handles and may be shared.

---

# 20. Fresh-Preservation Analysis

A function declared as returning `fresh T` must pass compiler freshness checking.

The analysis is intra-procedural.

Inter-procedural facts are taken only from function signatures.

---

## 20.1 Fresh expression sources

An expression is fresh if it is one of:

```text
struct constructor expression creating a new shell
call to a function returning fresh T
clean local binding
composition of fresh fields into a fresh shell
```

---

## 20.2 Clean local binding

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

## 20.3 Fresh analysis pseudocode

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

## 20.4 Branches

All return branches must return fresh values.

---

## 20.5 Closures and freshness

A local captured by a managed closure is not clean.

A local temporarily used by a noescape closure remains clean if not retained.

---

## 20.6 Generics and freshness

A generic function returning `fresh T` must require `T: Struct`.

If freshness cannot be proven for all valid instantiations, the function is rejected.

---

# 21. `manage`

`manage` moves a local value into the managed runtime.

```rust
local image = Image.load(path: read path)?
let shared = manage image
```

After `manage`, the local binding is moved.

---

## 21.1 Semantics

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

## 21.2 Failure

If migration allocation fails:

```text
the current isolate aborts
no rollback is guaranteed
no broken state is exposed
```

---

## 21.3 Cost

`manage` is not guaranteed O(1).

Cost is proportional to the local inline graph migrated.

Handle fields are not deep-cloned.

---

# 22. Resources and `with`

Resources require deterministic cleanup.

```rust
with File.open(path: read path) as file {
    File.write(file: mut file, data: read data)?
}
```

---

## 22.1 Drop points

A `with` resource is dropped on:

```text
normal block exit
return
break
continue
panic unwind, if implementation supports unwinding
```

---

## 22.2 Resource escape is forbidden

Inside a `with` block, the resource cannot be:

```text
returned
returned through read/mut/take wrappers
managed
taken out of the block
stored in a managed object
captured by a managed closure
```

---

## 22.3 ResourcePool

`ResourcePool<T: Resource>` is the standard-library escape hatch for long-lived resources.

```rust
features: local

local pool = ResourcePool<DbConnection>.new(
    create: || DbConnection.open(url: read url),
    max_size: 16,
)

with ResourcePool.borrow(pool: mut pool) as conn {
    DbConnection.query(conn: mut conn, sql: read sql)?
}
```

Rules:

```text
ResourcePool itself must be local
ResourcePool is allowed only with features: local
borrow returns a with-compatible resource lease
pool drop releases all held resources
resource values cannot escape the pool lease
```

---

# 23. Containers

## 23.1 Managed containers

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

## 23.2 Local containers

Local containers are advanced standard-library types.

```rust
features: local

local buffers = LocalVec<Buffer>.new()
```

Local containers may hold local struct values.

Container elements do not participate in language-level partial access.

---

## 23.3 Resource containers

Only approved resource containers may store resources.

In v0.5:

```text
ResourcePool<T: Resource>
```

is the standard resource container.

---

# 24. Generics

## 24.1 Default bound

Generic type parameters default to `Managed`.

```rust
fn first<T>(items: read List<T>) -> Option<T>
```

means:

```rust
fn first<T: Managed>(items: read List<T>) -> Option<T>
```

Resource types are not `Managed`.

---

## 24.2 Struct bound

Use `T: Struct` for fresh or local-capable values.

```rust
fn make_pair<T: Struct>(left: read T, right: read T) -> fresh Pair<T>
```

---

## 24.3 Resource bound

Resource generic APIs must be explicit.

```rust
ResourcePool<T: Resource>
```

Ordinary `List<T>` cannot be instantiated with resource types.

---

## 24.4 Generic resources

Resource declarations may be generic if all type parameters have explicit bounds.

Legal:

```rust
resource NativeBuffer<T: Copy> {
    handle: NativeHandle

    drop {
        Native.free(handle: handle)
    }
}
```

Illegal:

```rust
resource BoxedResource<T> { ... } // missing bound
```

A generic resource remains a resource for all valid instantiations.

It cannot become managed.

---

## 24.5 Resource type parameters

A generic resource container must explicitly require resource parameters:

```rust
struct ResourcePool<T: Resource>
```

`T: Resource` cannot be stored in ordinary struct/class fields unless the container is an approved resource container.

---

## 24.6 Retention with generics

A function retaining a generic parameter must declare it:

```rust
fn store<T: Managed>(box: mut Box<T>, value: read T) -> Unit
    effects(retains(value))
```

---

## 24.7 Minimal interfaces, future capability contracts

RSScript should not expose Rust-style traits at the source level. Generated Rust
may use Rust traits as a lowering strategy, but RSScript diagnostics and public
contracts must speak in RSScript terms.

A future language-level `interface` feature should be minimal:

```text
interface = app-layer capability contract
```

It is for capabilities such as:

```text
Logger
Clock
Store
Cache
HttpClient
Queue
Reader
Writer
MetricsSink
ConfigSource
Repository
```

It is not for library-layer type-system machinery:

```text
associated types
blanket impls
specialization
trait objects
object safety rules
higher-ranked bounds
lifetime bounds
arbitrary where clauses
operator overloading
auto method resolution
default methods
```

An interface declares function signatures:

```rust
interface Logger {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(no_panic)
}
```

Async methods are allowed as review-visible contracts:

```rust
interface HttpClient {
    async fn send(
        self: read Self,
        request: read Request,
    ) -> Result<fresh Response, HttpError>
        effects(no_block)
}
```

The default `Self` bound is `Managed`:

```text
interface X
```

means:

```text
interface X<Self: Managed>
```

Allowed `Self` bounds are:

```text
Managed
Struct
Resource
Copy
```

Generic interfaces, inheritance, bound composition, associated types, and where
clauses are not part of this minimal model:

```rust
interface Store<T> { ... }          // not v0.5
interface A: B { ... }              // not v0.5
interface X<Self: A + B> { ... }    // not v0.5
```

Conformance is explicit:

```rust
impl Logger for ConsoleLogger {
    write = ConsoleLogger.write
}
```

The mapped concrete function must already exist:

```rust
fn ConsoleLogger.write(
    self: mut ConsoleLogger,
    message: read String,
) -> Unit
    effects(no_panic)
```

Contract matching is strict in the first interface subset:

```text
parameter names must match
parameter effects must match
parameter types must match after Self substitution
return type must match
freshness must match
async/sync kind must match
retains effects must match
native/unsafe effects must match
guarantees must match
```

Interface calls are explicit. There is no hidden method dispatch:

```rust
Logger.write(self: mut logger, message: read message)
```

This is intentionally not interface dispatch syntax:

```rust
logger.write(message: read message)
```

Generic functions may use explicit interface bounds:

```rust
fn save_log<L: Logger>(
    logger: mut L,
    message: read String,
) -> Unit {
    Logger.write(self: mut logger, message: read message)
    return Unit
}
```

First-class interface values are not part of this model:

```rust
let logger: Logger = ConsoleLogger.new() // not v0.5
let services = List<Logger>.new()        // not v0.5
```

If dynamic dispatch is added later, it must be a separate review-visible feature
such as `features: dyn_interface`, with explicit syntax.

`.rssi` public contracts may eventually declare interfaces and impl mappings.
Semantic diff and review map must treat interface changes as review-relevant,
including method additions/removals, effect changes, freshness changes,
async/sync changes, native/unsafe changes, and impl mapping changes.

The purpose of interfaces is app-layer composition with review-visible
capability boundaries, not maximum type-system expressiveness.

---

# 25. Closures

RSScript has three closure categories:

```text
managed closure
local closure
noescape closure
```

---

## 25.1 Managed closure

A closure bound with `let` is managed.

Managed closures may capture:

```text
Copy values
managed values
handle or weak field paths, because those fields are managed handles
```

They may not capture:

```text
local values
resources
with-bound resources
```

---

## 25.2 Local closure

A closure bound with `local` is local and may move-capture local values.

```rust
features: local

local buffer = Buffer.new(size: 1024)

local callback = move || {
    Buffer.clear(buffer: mut buffer)
}
```

---

## 25.3 Noescape closure

A noescape closure cannot be stored or returned.

```rust
fn apply(callback: noescape Fn()) -> Unit
```

Noescape closures may temporarily use local values.

---

# 26. Error Handling

RSScript uses explicit result types for recoverable errors.

```rust
Result<T, E>
Option<T>
```

`may_fail` is not a runtime effect.

Use of `?` is allowed only inside functions returning compatible `Result`.

---

## 26.1 Panic

Panic is unrecoverable by default.

Functions may panic unless marked `no_panic`.

---

## 26.2 Exhaustive matching

Pattern matches must be exhaustive.

Wildcard patterns in public code are discouraged and may be linted.

The v0.5 implementation surface starts with statement-form `match` over the
standard enum-like result shapes:

```text
Option<T>: Some(value), None
Result<T, E>: Ok(value), Err(error)
```

A match is accepted when it covers `Some` and `None`, covers `Ok` and `Err`, or
contains a wildcard `_` fallback. Other enum exhaustiveness is reserved for the
full enum/type-resolution pass.

---

# 27. Forbidden Features

RSScript v0.5 does not support:

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
custom VM as MVP execution target
user-facing Rust lifetime syntax
```

---

## 27.1 Operators

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

# 28. Standard Library Philosophy

RSScript standard libraries should follow:

```text
Managed at the surface.
Local in the engine.
Reviewable at the boundary.
```

---

## 28.1 User-facing APIs are managed-first

Simple public APIs should hide local scratch details.

```rust
let json = Json.parse(text: read body)?
```

---

## 28.2 Library internals are local-first

Library implementations should use local scratch buffers and `*_into` APIs where performance matters.

```rust
features: local

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError> {
    local scratch = JsonScratch.new(
        token_capacity: 4096,
        node_capacity: 2048,
    )

    JsonLexer.lex_into(text: read text, tokens: mut scratch.tokens)?
    JsonParser.parse_into(tokens: read scratch.tokens, nodes: mut scratch.nodes)?

    local value = JsonBuilder.build(nodes: read scratch.nodes)?
    return Ok(value)
}
```

---

## 28.3 Expert APIs use `*_into`

Hot-path APIs should expose local reuse explicitly.

```rust
Json.parse_into(
    text: read text,
    scratch: mut scratch,
    output: mut builder,
)?
```

---

## 28.4 Fresh by default for new values

If a function creates a new struct value, it should return `fresh T`.

If it returns an existing shared object, it should return managed `T`.

---

## 28.5 Retention must be declared

Functions that store or retain parameters must declare `retains`.

---

# 29. Core Library Model

RSScript core is signature-first.

## 29.1 `.rssi` interface files

Core APIs should be declared in `.rssi` interface files before implementations exist.

Example:

```rust
// core/json/json.rssi

struct JsonValue

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>

pub fn field_string(
    value: read JsonValue,
    name: read String,
) -> Result<String, JsonError>
```

The checker reads `.rssi` signatures.

The runtime may be implemented in Rust first.

---

## 29.2 Native implementation behind RSScript signatures

A Rust native implementation must conform to the `.rssi` signature.

RSScript signatures are the public API.

Native implementation details are not visible to RSScript users.

---

## 29.3 Core MVP packages

Minimum core signatures:

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

---

## 29.4 Package Manager Design

RSScript package management is specified separately from the core language syntax.

The package-manager direction is defined in:

```text
RSScript_Package_Manager_Design.md
```

See [RSScript Package Manager Design](RSScript_Package_Manager_Design.md).

That design is compatible with this v0.5 spec because it preserves the same implementation boundary:

```text
RSScript packages expose reviewable .rssi semantic contracts.
RSScript source lowers to Rust through the compiler pipeline.
Rust native wrappers are built by Cargo.
Cargo remains the Rust build and crate dependency substrate.
Package review metadata reports semantic risk instead of hiding it.
Unknown package risk is classified as unknown, not safe.
```

Package features declared in `rsspkg.toml` are package selection features. They are not the same as RSScript file features declared with `features:` at the top of a `.rss` or `.rssi` file.

If a package feature enables native code, unsafe code, build scripts, proc macros, linked libraries, or another advanced boundary, package review metadata must report that risk explicitly.

The current prototype implements a local package review subset:

```text
rss package check
rss package review
rss package lock
rss package review update
rss package tree
rss package publish --dry-run
rss package vendor
rss package metadata
rss package diff
```

`rss package check` validates a local package manifest, loads local path dependency `.rssi` contracts into the frontend environment, checks package `.rssi` public contracts against source implementations, rejects unresolved or conflicting local dependency graphs, runs interface/source frontend checks, regenerates package review metadata, compares the current semantic lock against `rsspkg.lock`, and scans enabled native Rust wrapper metadata for local consistency. When `[review] unknown_is_error = true`, any package review result with unknown risk makes package check fail even if the lock is current and there are no frontend errors.

Package review metadata treats `.rssi` files as the preferred public contract surface. It reports package feature names, public type/function/API counts, mutating APIs, retaining APIs, resource APIs, fresh-returning APIs, native APIs, unsafe APIs, and currently unknown APIs separately; it also emits per-export review classifications with reasons. If a `.rssi` contract has frontend errors, those diagnostics are reported as unknown contract exports and counted as unknown APIs, because the public semantic contract cannot be trusted. If a package has no `.rssi` surface, the prototype falls back to public source declarations for those counts and exports.

`rss check <package-directory>` is an alias for package check when the directory contains `rsspkg.toml`. Single-file `rss check <file.rss>` keeps the ordinary frontend diagnostic behavior.

`rss package review update` compares two `rsspkg.lock` files and classifies package version, source, checksum, public interface hash, review metadata hash, native wrapper hash, and feature-selection changes.

`rss package lock` records the root package plus recursively reachable local path dependencies. Registry and git dependencies remain unresolved until the resolver exists.

`rss package tree` prints the package dependency graph with review risk. The prototype expands local path dependencies recursively and classifies unresolved registry or git dependencies as `unknown`.

`rss package publish --dry-run` performs local pre-publish validation without uploading: package consistency, dependency graph review, semantic version shape, package review risk classification, native metadata, and reproducible archive hashing. Unknown package review risk blocks publish readiness; it is reported as unknown rather than treated as a safe-to-publish result.

`rss package vendor` copies local path dependencies into `vendor/<name>-<version>/` and writes `vendor/rss-vendor.json` for offline/reproducible review. Registry and git dependencies remain unresolved until the resolver exists.

`rss package metadata` writes `review/package-review.json` with schema `rss.review.package.v1`, using the same local review result as `rss package review`. Metadata generation still records unknown risk, but the command result is not ok when the review risk is unknown.

Package Rust lowering loads the package's own `.rssi` contracts plus dependency
interfaces as the call-resolution environment. If `[native.rust]` is enabled,
`native/bindings.rssbind.toml` may provide a minimal binding table:

```toml
[bindings]
"Native.echo" = "rss_json_native::echo"
```

These bindings map bodyless `native fn` contracts to Rust wrapper functions in
the generated package. The binding manifest is part of native review metadata
and native hashing. Package checks must reject bindings whose RSScript symbol is
not declared by a package `.rssi` native function, and bindings whose Rust target
does not live under the configured `[native.rust].crate`.

---

# 30. Native Core and FFI Boundary

v0.5 supports controlled native core boundaries.

It does not define general user FFI.

---

## 30.1 `native` effect

A native function must be declared with `effects(native)` or through a native module declaration.

```rust
native fn File.open(path: read Path) -> Result<File, IOError>
    effects(native)
```

`native fn` declarations are bodyless in v0.5. A function with an RSScript body
may be marked `effects(native)` only when that function's contract crosses a
native boundary through calls or package wrapper bindings; the `native fn`
keyword itself declares an external implementation.

---

## 30.2 Native safety obligations

Native implementations must preserve RSScript semantics:

```text
must not retain local values unless expressed through manage
must not fake fresh values
must not allow resource escape
must translate native panics/errors into RSScript diagnostics or Result errors
must preserve managed handle identity and weak-reference requirements
must preserve source location hooks where applicable
```

---

## 30.3 Native diagnostics

Native boundary failures must report RSScript-level diagnostics whenever possible.

Native runtime errors must not expose Rust implementation internals as primary user diagnostics.

---

# 31. Diagnostics Protocol

Compiler diagnostics must have both human-readable and machine-readable forms.

---

## 31.1 JSON form

Example:

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

---

## 31.2 Required diagnostic classes

Implementations must provide diagnostics for:

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
feature violation
unsupported syntax
unmappable rustc diagnostic
native boundary violation
```

---

# 32. Rust Diagnostic Mapping

## 32.1 Frontend-first diagnostics

The RSScript frontend should catch ordinary user errors before Rust lowering.

rustc diagnostics should usually indicate:

```text
compiler bug
runtime crate mismatch
missing native implementation
lowering bug
unexpected backend limitation
```

They should not be the normal user experience.

---

## 32.2 Source mapping is mandatory

Generated Rust must carry sufficient source mapping for every user-originating construct.

When rustc emits an error, the RSScript compiler must attempt to map it back to RSScript spans.

If mapping fails, report:

```text
internal compiler diagnostic with generated Rust reference
```

not raw Rust output as the primary diagnostic.

---

## 32.3 Diagnostic translation quality

A translated diagnostic must include:

```text
RSScript source span
RSScript explanation
backend diagnostic as secondary detail
suggested RSScript-level fix when possible
```

Raw rustc diagnostics may be attached for debugging under a verbose flag.

---

# 33. Semantic Review Tools

RSScript review tooling has two modes:

```text
rss review --diff
rss review --map
```

---

## 33.1 `rss review --diff`

Compares two checked RSScript programs.

Answers:

```text
What semantic behavior changed?
```

Reports:

```text
public API changes
parameter effect changes
return freshness changes
retention changes
guarantee changes
new native usage
new unsafe usage
new local/manage boundary
resource lifetime changes
callers requiring re-review
```

Example:

```text
review diff:

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

## 33.2 `rss review --map`

Performs absolute semantic review on a single file/module/directory.

Answers:

```text
Which parts of this code require human review?
```

Intended for AI-generated code with no meaningful previous version.

Example:

```sh
rss review --map generated_handler.rss
rss review --map --json src/agent/
```

---

## 33.3 Review map categories

A review map classifies code regions as:

```text
entry_point
must_review
review_if_changed
safe_to_skim
safe_to_skip
unknown
```

---

## 33.4 Entry points

Entry points are always review-visible.

Examples:

```text
main
public API
test function
registered handler
exported callback
configured command
agent loop
```

---

## 33.5 Must review

A function/region is must-review if it contains or exposes:

```text
public API surface
mut parameter
take parameter
manage operation
effects(retains(...))
with resource
ResourcePool
file features such as local/native/unsafe/async/device/ffi/reflection
native boundary
unsafe boundary
unknown external call
writes to managed state
writes through handle fields
fresh guarantee boundary
error handling boundary
removed guarantee
```

File-level features must also be reported as file-level review risk:

```text
local        elevated risk
async        elevated risk
native       high risk
unsafe       high risk
device       high risk
ffi          high risk
reflection   elevated risk
```

This file-level risk does not require every helper function in the file to be classified as must-review. Region classification still depends on the function's own semantic facts and propagated callee risk.

---

## 33.6 Safe to skip

A function may be classified safe-to-skip only if all of the following hold:

```text
private
not an entry point
no mut parameters
no take parameters
no retains effects
no with resources
no manage operation
no native or unsafe boundary
no unknown calls
no mutation of reachable managed state
no writes through handle fields
all callees are also safe or proven low-risk
```

Safe-to-skip does not mean logically correct.

It means no language-visible side-effect, resource, retention, native, or local/managed boundary risk.

---

## 33.7 Unknown

If the tool cannot classify a region, it must mark it unknown.

Unknown must not be classified as safe-to-skip.

An unresolved direct call makes the containing region unknown even when the
function is public or otherwise review-required; the public/API reason should be
retained as context, but the classification must remain unknown.

Early implementations may over-report review area.

They must not under-report risk.

---

## 33.8 Review map JSON

Example:

```json
{
  "kind": "review_map",
  "summary": {
    "total_functions": 2,
    "total_lines": 412,
    "must_review_lines": 41,
    "safe_to_skip_lines": 358,
    "unknown_lines": 0,
    "suggested_review_lines": 54,
    "review_ratio": 0.131,
    "must_review": { "functions": 1, "lines": 41 },
    "safe_to_skip": { "functions": 1, "lines": 358 },
    "unknown": { "functions": 0, "lines": 0 }
  },
  "files": [
    {
      "file": "handler.rss",
      "features": ["local", "native"],
      "risk": "high",
      "reasons": [
        "local capability enabled",
        "native boundary capability enabled"
      ],
      "regions": [
        {
          "function": "run_agent",
          "classification": "must_review",
          "line": 12,
          "line_count": 13,
          "reasons": ["entry point", "mut parameter agent"]
        },
        {
          "function": "pure_helper",
          "classification": "safe_to_skip",
          "line": 120,
          "line_count": 8,
          "reasons": ["private helper with no review-visible semantic risk"]
        }
      ]
    }
  ]
}
```

---

## 33.9 Limits

`rss review --map` does not prove:

```text
business logic correctness
algorithmic correctness
security policy correctness
prompt quality
tool selection correctness
branch condition correctness
```

It reduces review area for language-visible risks.

---

# 34. Formatter, Linter, and Check

## 34.1 Formatter

`rss fmt` must be deterministic.

There are no formatting style options in v0.5.

---

## 34.2 Linter

`rss lint` enforces:

```text
public API explicitness
forbidden feature checks
wildcard match warnings
unnecessary handle field warnings
unused effects
signature complexity budget
```

---

## 34.3 Check

`rss check` loads bundled core `.rssi` signatures by default, then runs:

```text
parser
name/type checks
effect checks
fresh checks
local move checks
resource checks
forbidden feature checks
diagnostic emission
```

---

# 35. Isolate Model

RSScript v0.5 uses a single-isolate model.

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

# 36. Examples

## 36.1 File write

```rust
fn write_text(path: read Path, text: read String) -> Result<Unit, IOError> {
    with File.open_write(path: read path) as file {
        File.write(file: mut file, data: read text)?
    }

    return Ok(Unit)
}
```

---

## 36.2 Image pipeline

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

---

## 36.3 Cache retention

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

---

## 36.4 Config with handle fields

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

---

## 36.5 Resource pool

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

# 37. Implementation Roadmap

v0.5 replaces the previous VM/interpreter-first roadmap with a Rust-lowering roadmap.

## 37.1 Milestones

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

---

## 37.2 Correct dependency order

The correct dependency graph is:

```text
runtime type surface
  -> lowering target shapes
  -> Rust source lowering + source maps
  -> rustc diagnostic mapping
  -> runtime implementation fill-in
  -> runnable MVP
```

Do not implement lowering before defining the runtime target surface.

Do not defer source mapping until after lowering.

---

## 37.3 Self-hosting path

Self-hosting does not require a custom VM.

A self-hosted RSScript compiler can still emit Rust source.

Stages:

```text
1. Rust bootstrap compiler checks/lowers RSScript
2. RSScript tools self-host: formatter, review, diagnostics
3. RSScript frontend self-host: lexer/parser/HIR/checker
4. rssc-stage1 compiles rssc.rss to Rust
5. rustc builds stage1
6. stage1 compiles rssc.rss again
7. stage1/stage2 outputs are compared
```

Rust remains the backend.

---

## 37.4 AI authoring micro-spec

RSScript has no meaningful public corpus today, so general-purpose language
models should not be expected to write correct RSScript from pretraining alone.

A future tooling milestone should provide a compact AI authoring micro-spec:

```text
purpose: teach an LLM to write RSScript through in-context learning
size: small enough to fit directly in prompts and agent system context
content: canonical syntax, core semantic rules, and focused examples
audience: code-generation agents, repair agents, and review assistants
```

This micro-spec is not a replacement for the normative language spec. It is a
promptable subset optimized for generation quality.

It should include:

```text
1. file header rules and `features:` examples
2. type declarations: class / struct / resource / handle / weak
3. function signatures with named parameters and read/mut/take
4. managed-default examples using only `let`
5. local hot-path examples with `local`, `take`, and `manage`
6. `with` resource examples and forbidden escape examples
7. `effects(retains(...))` examples and local-retention errors
8. `fresh` return examples, including shallow handle-field limits
9. native/unsafe/async boundary examples as review signals
10. package `.rssi` contract examples
11. review-map and diagnostic examples showing what the checker reports
12. anti-patterns that AI commonly writes and the RSScript rewrite
```

The examples should be short, canonical, and deliberately repetitive. The goal
is to make a model imitate the intended surface reliably before RSScript has
enough real-world code to appear in training data.

The compiler repository should keep this micro-spec executable where possible:
examples used for prompting should also be checked by `rss check`, and negative
examples should map to stable diagnostic codes.

---

# 38. Non-goals of v0.5

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
trait objects
Future / Pin / Poll / Waker source model
general user-defined FFI
GPU kernel language
agent runtime as language core
custom thread-shared tracing heap
moving GC
managed -> local demotion
operator-overloaded numeric DSLs
macro-heavy metaprogramming
```

---

# 39. Reviewer Checklist

Reviewers should evaluate v0.5 by asking:

```text
1. Is RSScript still managed-first?
2. Is local still an explicit capability, not the default world?
3. Are read/mut/take effects visible and canonical?
4. Is retention expressed through effects(retains(...))?
5. Are fresh and manage semantics clear?
6. Are partial local access rules implementable?
7. Are container element restrictions conservative enough?
8. Does Rust lowering preserve RSScript diagnostics?
9. Are runtime crate surfaces defined before lowering?
10. Are generated Rust diagnostics properly source-mapped?
11. Does rss review support both diff and map modes?
12. Is the spec free of domain-specific agent/GPU core pollution?
```

---

# 40. Final Model Summary

RSScript v0.5 can be summarized as:

```text
one canonical syntax
managed by default
local when performance matters
with for scoped resources
fresh at creation boundaries
manage as one-way local -> managed transition
read/mut/take for parameter behavior
retains for post-call retention
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
