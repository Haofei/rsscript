# RSScript (Reviewable System Script) Language Specification v0.5

Audience: language designers, compiler implementers, standard-library authors, review-tool authors
Architecture note: v0.5 uses **RSScript frontend -> Rust source lowering -> rustc backend**.

---

## Constitution

These articles are the highest authority in this document. They state what
RSScript is for and what governs every present and future design decision. They
are binding: where any chapter, example, feature, or convenience conflicts with
an article, the article wins. The numbered chapters are the detailed law; this
section is the constitution they answer to.

**Article I — Purpose: review is the bottleneck.**
RSScript exists for the AI-era cost inversion. Generating code is now cheap;
reviewing generated code is the bottleneck. Every design decision is justified
by whether it makes generated code cheaper and safer to review, not by
expressiveness, cleverness, or familiarity. (Detail: Chapter 1.)

**Article II — Constraint is the product.**
RSScript's value is in what it refuses to express as much as in what it
expresses. A smaller surface lowers two costs at once: the reviewer's first-read
cost, and the AI generator's option space. Removing a capability is a legitimate
and often preferred design act. Features are admitted by subtraction-bias, not
addition-bias.

**Article III — Review-critical behavior is explicit and in the signature.**
What mutates, what is retained, who owns a resource, what is fresh, and where
code crosses local/managed/native/unsafe must be visible in the signature and
machine-checked — never carried by comments, convention, or inference. If it
matters to a reviewer, it is in the type, and the compiler enforces it both at
the definition and at every call site. (Detail: sections 2.4, 2.5, Chapter 10.)

**Article IV — Feature admission rule: aggregate, do not interact.**
A candidate feature is admissible only if (1) it can be phrased as a reviewer
question and (2) it can be expressed with explicit, named, single-canonical
syntax with no implicit rule added to make it ergonomic. Features must be
coordinated projections of the one review model, so they compose instead of
producing combinations no one designed. Convenience bought with implicitness is
rejected even when convenient. (Detail: section 2.8.)

**Article V — Restraint is anchored to a product property, not an aesthetic.**
Features are rejected because they raise review cost or break the review model —
a measurable property (effect visibility, review-map unknown ratio) — not because
they are judged "inelegant" or "not simple." Aesthetic restraint loses to "but I
need X"; restraint anchored to reviewability does not. This article is the
deliberate antibody against feature accretion: the discipline is written down so
it can be enforced by rule under future pressure, not by mood or by a single
gatekeeper's taste.

**Article VI — Say no without welding the door shut: deferred, not excluded.**
When a capability is not admitted now, the spec records the binding constraints
that any future form must satisfy, rather than declaring a permanent non-goal —
unless the capability fails Article IV in principle, in which case it is excluded
outright. A deferral preserves an option; an exclusion closes one; the spec must
say which it means and never silently conflate them. (Detail: sections 14.6,
20.1, 21.1.)

**Article VII — Rust is a backend, not the language model.**
RSScript lowers to Rust and reuses its ecosystem, but Rust's lifetimes, trait
machinery, and backend representation never define RSScript semantics or leak
into the user surface. Valid RSScript code must not require understanding the
generated Rust. (Detail: sections 2.7, 4.)

---

## 0. Reading Guide and Normative Hierarchy

This document reorganizes the v0.5 draft around the semantic boundaries that the compiler must enforce before Rust lowering. The previous draft had many correct rules, but several were scattered across runtime, data-effect, resource, closure, review, and example chapters. This edition treats the following chapters as the primary semantic authority:

```text
Chapter 5   Expression modes and materialization
Chapter 5A  Statements and control flow
Chapter 8   Places, conflict roots, and same-call conflicts
Chapter 9   Call-like expressions, constructors, and variants
Chapter 10  Data effects, retention, and managed closure capture
Chapter 12  Resources, with, and ResourcePool
Chapter 17  Diagnostics and source mapping
```

The normative hierarchy is: the Constitution governs every chapter; among the
chapters, the semantic boundary chapters above are the primary authority; and if
an example or later explanatory section conflicts with those chapters, the
semantic boundary chapter wins. A chapter rule that conflicts with a
constitutional article is itself in error.

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
    let in_path = Path.from_string(value: read "in.png")
    let out_path = Path.from_string(value: read "out.png")
    let image = Image.load(path: read in_path)?
    Image.resize(image: mut image, width: 800, height: 600)
    Image.save(image: read image, path: read out_path)?
    return Ok(Unit)
}
```

A string literal is a `String`, not a `Path`. Constructing the `Path` is explicit
through `Path.from_string` — there is no implicit `String -> Path` conversion
(section 2.4). This is the canonical form; the bundled examples use it.

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

*Elaborates Constitution Article III. The article is the governing statement; this section adds the concrete forbidden list.*

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

The annotation is a checked binding contract. `let y: Int64 = x` is rejected
unless `x` already has type `Int64`; the explicit conversion call above is what
makes the binding valid.
Wrapper payloads are checked as part of the same contract:
`let r: Result<String, E> = Ok(42)` is rejected because the `Ok` payload is
`Int`, not `String`.
Nested wrapper payloads are checked recursively:
`Result<Option<String>, E>` rejects `Ok(Some(42))` because the inner `Some`
payload is `Int`, not `String`.
Generic arguments are also part of the contract: `let xs: List<String> =
List<Int>.new()` is rejected because the initializer has type `List<Int>`.

### 2.5 Public APIs are review contracts

*Elaborates Constitution Article III. The article is the governing statement; this section lists exactly what a public API must expose.*

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

*Elaborates Constitution Article VII.*

RSScript lowers to Rust source while keeping Rust lifetimes, trait-bound complexity, borrow-checker diagnostics, and backend representation details behind the RSScript review protocol.

Valid RSScript code should not require the user to understand generated Rust.

### 2.8 Feature admission rule

*Elaborates Constitution Articles IV and V. The articles are the governing statement; this section is the operative test and its first applications.*

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

`rss check <file.rss>` performs frontend parsing, name/type/effect checking,
freshness/local/resource checks, and review metadata generation without Rust
lowering, Cargo invocation, or native build execution. `rss check
<package-directory>` uses the package manifest, package source set, and interface
environment; for packages with multiple source files, `src/main.rss` is the
runnable entry source but `rss check` may check the package source set.

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

The backend is replaceable; RSScript is not a Rust dialect or a Rust derivative.
The language's semantic model lives entirely in the RSScript frontend (its
syntax, type/effect checking, conflict roots, freshness, resource and managed
rules). Lowering is a separate stage defined by a backend-agnostic shape
contract (section 4.3). The same contract could be satisfied against another
systems backend — for example Zig or C — without changing RSScript semantics.

Targeting Rust in v0.5 is an engineering decision, not an identity. rustc, LLVM,
Cargo, and the crate ecosystem supply a mature backend — codegen, optimization,
platform support, linking, libraries, and a type-checking backstop for generated
code — that would otherwise take years to build. Reusing it lets RSScript spend
its effort on the review protocol, which is the product. A future backend is a
preserved option, exercised only with a concrete forcing function (a platform
Rust serves poorly, a much smaller runtime, or a C-interop-dominated domain), not
a default; a second backend is a large, mostly duplicating surface and must clear
the feature admission rule before it is added.

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

Option return values must be written as `Some(value)` or `None`.
There is no bare-success shortcut for `Option<T>`.

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

## 5A. Statements and Control Flow

This chapter is numbered `5A` so it sits next to expression modes (Chapter 5)
without renumbering later chapters. It is a primary semantic-boundary chapter: it
defines where resources drop, where local-move and freshness state change, and
where source maps mark boundaries.

This chapter defines the v0.5 executable statement and control-flow surface. It
is normative for where resources drop, where freshness and local-move state
change, and where source maps mark boundaries; the Rust lowering must preserve
these semantics.

### Statements

The v0.5 statement forms are:

```text
let binding
local binding
with binding
expression statement
return
if / else
while
loop
for
break
continue
match (statement form, over Option/Result variants)
```

`if` and `while` conditions must have type `Bool`. RSScript has no truthy or
falsey coercions for strings, numbers, handles, resources, or containers.

Built-in operators have fixed operand types. Arithmetic operators require
numeric operands. Equality operators require matching known operand types.
Ordering operators require numeric operands. Logical `&&` and `||` require
`Bool` operands. RSScript has no implicit conversion or user-defined operator
overload resolution.

RSScript v0.5 has **no assignment statement** other than these initialization
bindings: there is no `x = y`, `obj.field = y`, or `list[i] = y`. All mutation is
expressed through explicit `mut` API calls (`Map.insert(map: mut m, ...)`), so
mutation always participates in call-like effect, conflict-root, and resource
checking. If assignment is added later it must itself become a call-like,
effect-checked construct; v0.5 deliberately omits it.

`?` is the failure-propagation operator.

```text
- `?` is allowed only inside a function whose return type is Result<_, E>;
  applying it elsewhere is a diagnostic (it requires a Result value).
- on Ok(v) it evaluates to v and control continues.
- on Err(e) it is an early return of Err(e) from the enclosing function.
- on that early return, every active `with` resource in scope is dropped, in
  reverse order of acquisition, before the function returns — the same drop that
  a normal block exit, `return`, `break`, or `continue` performs (Chapter 12).
```

`?` performs **no implicit error conversion**. For `expr?` inside a function
returning `Result<U, E>`, `expr` must have type `Result<T, E>` with the *same*
error type `E`. A mismatched error type is a diagnostic, not a silent conversion
(§2.4 forbids implicit `From`/`Into` chains; this is the `?`-specific application
of that rule). Conversion must be written explicitly, for example with a
statement-form `match` that maps the error before returning:

```rust
match Config.parse(text: read text) {
    Ok(config) => {
        Config.apply(config: read config)
    }
    Err(e) => {
        return Err(AppError.from_config(error: read e))
    }
}
```

A future version may add an explicit `Result.map_err` API for this in canonical
call form (`Result.map_err(result: ..., mapper: ...)`); it would still be an
explicit, named call, never an implicit backend conversion. It is described here
in prose because the closure-parameter syntax it would need is not part of the
v0.5 surface.

`?` is the only implicit control transfer in RSScript, and it is visible in the
source as the `?` token.

This is the one implicit control transfer RSScript allows, and it lowers to
Rust's `?`, whose default behavior is exactly the `From` conversion Article III
forbids. The sound v0.5 rule has two obligations:

```text
1. RSScript lowering emits no `From`/`Into` conversions for error types. With
   that invariant, a mismatched error type has no RSScript-generated conversion
   path to silently pass through.
2. The frontend rejects a mismatched operand/return error type directly, so the
   rule is enforced at the source level, not left to backend rustc failure. The
   frontend reports this as `RS0013`.
```

The residual silent-conversion risk is a **native-provided `From`** at a native
boundary; that is a native-boundary review item, not ordinary safe-surface
behavior.

### `return`, `break`, `continue`

```text
- return exits the function; break/continue exit or re-enter the nearest loop.
- each drops the `with` resources whose scope it leaves, in reverse order.
- a local value moved by take/manage before one of these does not become live
  again on the path after it; local-move state is per-path (Chapter 11 freshness
  analysis is intra-procedural and path-sensitive at these boundaries).
```

### `for`

```text
for <var> in <iterable> { ... }
```

The iterable is read-iterated by the loop. The loop does not consume, mutate, or
retain the iterable unless an explicit iterator API says so ("consume" in
RSScript means a `take`, which `for` does not do). The loop variable is bound per
iteration; it is a Copy value or a managed read view of the element, never a
local exclusive value extracted from a managed container and never a resource
taken out of a pool. A loop body may open its own `with` resources, which drop at
the end of each iteration.

Resources, freshness, and local-move state observe loop back-edges: a value moved
inside the loop body is not usable on a later iteration, and a `with` resource
opened in the body is dropped before the next iteration.

### `while` and `loop`

```text
while <condition> { ... }
loop { ... }            // exited by break
```

`while` and `loop` follow the same back-edge rules as `for`: local-move and
freshness state are computed as a fixpoint over the loop body, so a value moved
(`take`/`manage`) inside the body is not live on any later iteration, and a value
the body depends on being unmoved must be unmoved on every back-edge. A `with`
resource opened in the body drops at the end of each iteration. `break` and
`continue` drop the `with` resources whose scope they leave.

### `match` (statement form)

```text
match <value> { <arm> => { ... } ... }
```

In v0.5, `match` is over the standard `Option<T>` and `Result<T, E>` variant
shapes only. The scrutinee must have type `Option<T>` or `Result<T, E>`.
Arm variants must match the scrutinee family: `Option<T>` arms may use
`Some`/`None`, and `Result<T, E>` arms may use `Ok`/`Err`. Mixing those variant
families is a diagnostic before lowering, not a Rust backend error. It must be
exhaustive: it covers `Some`/`None`, `Ok`/`Err`, or includes `_`; a
non-exhaustive match is a diagnostic before lowering. Arm rules:

```text
- a variant payload binding obeys the same data-effect, move, and resource rules
  as any other binding; a payload that is a resource cannot escape its arm.
- a `with` resource opened inside an arm drops at that arm's exit.
- local-move, freshness, and clean-local state from the arms are combined at the
  match exit by the conservative branch join below.
```

A payload binding has no `read`/`mut`/`take` syntax of its own, so its mode is
fixed by the **scrutinee's** materialization mode:

```text
- matching a managed/read Option/Result binds a non-Copy payload as a managed
  read value (a read view), usable within the arm but not consumed.
- matching a `fresh Result<fresh T, E>` (return-position analysis) may keep the
  payload fresh only within the arm.
- matching a local Option/Result is allowed only under features: local; the
  selected payload moves out under local move rules.
- a resource payload may be matched only inside an approved immediate resource
  context and cannot escape its arm.
- Copy payloads copy.
```

A *local* `Option`/`Result` is not produced by extracting from a managed value
(that is forbidden, §7.4). It arises only from a `local` binding of an
`Option<fresh T>` / `Result<fresh T, E>` value — for example
`local parsed = Json.try_parse(...)` where the result type is
`Result<fresh JsonValue, E>`. The `local` binding makes the variant value itself
local with a fresh payload (Chapter 5 materialization), and matching it moves the
fresh payload out under local move rules. Without such a `local` binding, a
matched `Option`/`Result` is managed and its payload binds as a managed read
value.

### Branch joins: `if`/`else` and `match`

At every branch join (the exit of an `if`/`else` or a `match`), local state is
joined **conservatively** — the join is the *intersection* of what holds on the
incoming paths, not the union:

```text
local-move:
    a local place is usable after the join only if it is live (not moved) on
    every reachable incoming path.
    Live  + Live   => Live       (usable)
    Moved + Moved  => Moved       (use after join rejected)
    Live  + Moved  => MaybeMoved  (use after join rejected)
    MaybeMoved + _ => MaybeMoved  (use after join rejected)

freshness / clean-local:
    a place is fresh (or clean-local) after the join only if it is fresh
    (clean-local) on every reachable incoming path.
```

So if a place is moved (`take`/`manage`) on *any* reachable arm and used after
the join, that use is rejected — moving on one arm is enough to poison the use,
because the runtime may have taken that arm. A path that exits the enclosing
scope before the join (e.g. `return`/`break` in an arm) is not an incoming path
to the join. Loops (`while`/`loop`/`for`) use the same conservative join as a
fixpoint over the back-edge.

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
the class value itself is always a managed identity object
fields may be inline, handle, or weak handle under the §6.5 rule
weak fields may break managed cycles
cannot be local
cannot be fresh
cannot be resource
```

"Always managed" describes the class value, not each field: a class is a managed
identity object, and its fields follow §6.5 (handle if marked `handle`/`weak` or
class-typed, otherwise inline within the managed class). A class may hold inline
non-Copy fields such as `entries: Map<String, Image>`.

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

Inline fields are stored inside their containing value. Whether a field is a
handle follows one uniform rule, the same in `struct` and `class` declarations:

```text
A field is a handle field iff it is marked `handle`/`weak`, or its declared
type is a `class`. Every other field — Copy fields, and non-Copy struct-typed,
String, Bytes, Buffer, or container fields — is inline by default.
```

The class/struct distinction is not about field handle-ness; it is about the
containing value. A `class` is itself a managed identity object (§6.2): the class
value is a managed handle, and its inline fields live inside that managed object.
A `struct` is a value object (§6.3). So a class may hold inline non-Copy fields
(for example `entries: Map<String, Image>`); those fields are not separate
handles, they are stored within the managed class. A field whose type is a class
is a separate handle in both kinds, because class values are always managed
handles. This is enforced by the checker (class-typed fields are treated as
handles for conflict-root analysis) and matches the Rust lowering.

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
must target a class type: a weak field whose type is not a class is a
  diagnostic (RS0902). v0.5 weak references break managed cycles only between
  class identities; a weak struct/container field is not permitted.
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

### 6.8 Copy types

`Copy` is a core distinction: Copy parameters do not require a data effect
(§10.5), Copy fields are inline (§6.5), and managed containers and closures may
hold Copy values freely. v0.5 therefore fixes the Copy set explicitly.

The Copy types in v0.5 are exactly the compiler-declared scalar primitives:

```text
Bool
Byte  Char
Int  Int8  Int16  Int32  Int64
UInt UInt8 UInt16 UInt32 UInt64
Float Float32 Float64
Unit
```

Two further types are **exempt from data-effect syntax** but are **not Copy** —
do not read this as "freely copyable":

```text
Fd       a descriptor handle. Fd is not a user-facing ordinary value in v0.5: it
         appears only inside native/resource implementations such as File, and is
         exempt from data effects only in trusted native/resource internals, not
         as a general public API type. Copying an Fd value is not a sanctioned
         operation; ownership of the underlying descriptor lives in the resource.
closure  closure-typed parameters do not use read/mut/take syntax, but closures
         are not Copy. Their escape and retention behavior is expressed by
         `noescape` or `effects(retains(callback))`, and managed-closure capture
         retention (§10.8) is unchanged.
```

The sized and unsized scalar names are **distinct types in v0.5, not aliases**:
`Int` is not an alias for `Int64`, `Byte` is not an alias for `UInt8`, and so on.
There is no implicit conversion between them (§2.4); width changes are explicit
through a `T.from` constructor (`let n: Int64 = Int64.from(value: x)`). Whether
any of these should later become aliases is deferred; v0.5 keeps them distinct so
no conversion is hidden.

How the checker knows it is "inside a trusted native/resource internal" for the
`Fd` exemption: `Fd` appears only in `native fn` declarations and `resource`
implementations (e.g. `File`); the exemption applies there, and `Fd` is not a
permitted public-API parameter type in ordinary managed/local code. A future
version may formalize this with a capability rather than a type convention.

Everything else is non-Copy: managed handles, weak handles, resources,
containers (`List`, `Map`, `Set`), `String`, `Bytes`, `Buffer`, generic type
parameters, and every user-defined `class`, `struct`, or `resource` — including a
struct all of whose fields are Copy.

User-defined types are non-Copy in v0.5 with no implicit derivation; a struct is
never silently Copy because its fields are. A future explicit `copy struct` or
`derives(copy)` is deferred, not excluded (Article VI), and would have to be
explicit per the no-hidden-behavior rule (§2.4).

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

### 8.6 Noescape closure captures participate in same-call conflicts

A closure literal passed as a `noescape` argument is invoked during the call (it
cannot escape it, §10.9). Its captured `read`/`mut`/`take`/`manage` uses of
places are therefore synthetic accesses of the enclosing call-like expression and
take part in the same-call conflict check of section 8.4, exactly as if they were
written as direct arguments. This includes a `manage` of a captured place (a
move, conflicting with any other use of that place) and a `mut` access such as
`ResourcePool.borrow(pool: mut pool)` inside the closure body (a synthetic `mut`
on `pool`).

```rust
apply(
    image: mut image,
    callback: || Image.save(image: read image, path: read output),
)
```

This is rejected: `image` is used as `mut` directly and as `read` through the
closure capture, an overlapping `mut` + `read` in one call. Without this rule a
`noescape` callback would be a back door that hides mutation or retention of a
captured place that also appears as a direct argument, violating Article III.

Capture roots are computed from the closure body; names bound inside the closure
(`let`/`local`/`with`) are not captures and do not participate. This applies to
`noescape` closures, whose body runs synchronously within the call; an escaping
managed closure is governed by retention analysis (section 10.8) instead.

### 8.7 Field splitting is a local-only capability

Treating two distinct inline field paths of the same base as disjoint (so they
may both be `mut`/`take` in one call) is sound only when the base is a locally
exclusive value. A managed object — a class, a managed container, or a managed
binding — is a single runtime value behind one write guard, so two mutable
accesses to its inline fields conflict even when the field paths differ.

```text
For a managed base, any mut/take access to an inline field has the managed object
base as its conflict root, unless the path first crosses an explicit handle/weak
boundary (which reaches a distinct managed object). For a local base, distinct
inline field paths may be disjoint per §8.3.
```

```rust
Foo.run(a: mut cache.entries, b: mut cache.stats)   // cache is a managed class
```

This is rejected (diagnostic RS0309): `entries` and `stats` are inline fields of
one managed object and share its write guard. If `entries` were a `handle Map`,
the root would stop at `cache.entries` (a distinct managed object) and the two
accesses could be disjoint.

Splittability keys off the **binding world**, not the declared type. A base is
field-splittable only when its value is *provably local-exclusive in this
function*:

```text
- a `local` binding: splittable (one exclusive owner).
- a `take` parameter: splittable (`take` requires a local value, §10.4, so the
  caller handed over an exclusive value).
- a `mut` parameter: NOT assumed splittable. A `mut` parameter — even of a value
  type like a struct — may be backed by a managed object at the call site (a
  managed `let` value passed `mut`), which the callee cannot see. So a `mut`
  parameter base is treated as a managed object base for field splitting.
- a managed `let` binding: not splittable.
```

The earlier "value type ⇒ splittable" reading was wrong: a struct is a value
type but a `mut` struct parameter can still be managed-backed. The v0.5 checker
therefore tracks field-splittable local exclusivity separately from ordinary
`mut` parameter access.

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

For struct/class constructors, the required form depends on both the field kind
and the initializer source. The full matrix:

```text
Copy field:
  any matching expression copies normally; no data effect.

inline non-Copy field:
  from a local place        requires take (moves the value into the shell)
  from a fresh expression   allowed; the fresh shell is moved into the shell
  from a literal            allowed when the literal is a fresh value of the
                            field type (a String literal initializes a String
                            field directly); otherwise use an explicit constructor
  from a managed value      rejected; there is no implicit clone. Use an explicit
                            clone/copy API, or store a handle field instead.

handle field:
  from a managed value      requires read
  from a local value        requires read (manage local) — manage first
  from a fresh expression   materializes managed and stores the handle

weak field:
  requires a weak-handle-producing expression (e.g. Weak.from)
```

`resource` fields are forbidden outside approved resource containers.

In the example below, `name: "default"` initializes an inline `String` field from
a String literal (a fresh value of the field type), `rules: read rules` is a
handle field from a managed value, and `workspace: take workspace` is an inline
non-Copy field from a local place.

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

`pure` is call-time observational purity, not Haskell-style referential
transparency over mutable identity and not a memoization guarantee. It means the
function performs no observable external side effects, does not mutate reachable
managed state, does not retain values, does not consume local values, does not
open or return resources, and calls only functions whose contracts are also
`pure` under these rules.

A `pure` function may read non-Copy managed inputs through `read` parameters,
including `String`, `Bytes`, `Buffer`, structs, and class handles. It may inspect
the current value reachable from those inputs for the duration of the call, but
must not mutate or retain that value. Repeated calls with the same managed handle
are not guaranteed to return the same result if another call mutates the handle
between those calls.

A `pure` function must not read ambient state such as time, randomness,
environment variables, filesystem, network, global mutable state, or native
state unless a future explicit trusted capability defines a stronger contract.
Pure over native functions is trusted metadata, not inferred proof. A native
function marked `pure` remains a native boundary and must-review unless package
policy explicitly trusts that boundary.

### 10.7 `retains(x)`

`retains(x)` means the function may keep a managed value derived from parameter `x` after returning.

```rust
fn cache_put(cache: mut Cache, key: read String, value: read Image) -> Unit
    effects(retains(key), retains(value))
```

A function that stores both `key` and `value` declares both: `retains` names
every parameter the function keeps after returning. `retains(x)` may retain a
managed handle or managed value derived from `x`. It must not retain an active runtime read/write guard.

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
Registry.register(callback: cb)
Widget(on_click: cb)
```

A closure-typed argument is written by name (`callback: cb`), without a
`read`/`mut`/`take` wrapper: a closure's review-critical property is escape and
retention, not a data effect (§6.8). The canonical spelling is `callback: cb`.

If `image` is managed, the closure retains `image`. If `image` is local or a resource, the closure is rejected unless it is a `noescape` temporary.

### 10.9 Noescape and local closure escape hatches

A `noescape Fn(...)` parameter cannot store, return, or retain the closure. Noescape closures may temporarily use local values.

```rust
fn apply(callback: noescape Fn()) -> Unit
```

When the parameter has an explicit return contract, the closure literal is
checked at the call site:

```rust
fn build(callback: noescape Fn() -> Result<String, BuildError>) -> Unit
```

The callback's known return expression must match the declared `Fn` return type
before lowering. `callback: || Ok(42)` is rejected for the example above because
the `Ok` payload is `Int`, not `String`. This rule is generic: standard APIs
such as `ResourcePool<T>.new(create: noescape Fn() -> T, ...)` and
`ResourcePool<T>.try_new(create: noescape Fn() -> Result<T, E>, ...)` use the
same callback return contract rather than a ResourcePool-only type shortcut.
Nested callback wrapper payloads are checked recursively under the same rule:
`noescape Fn() -> Result<Option<String>, E>` rejects `callback: || Ok(Some(42))`
before Rust lowering.
Freshness is also part of the callback return contract. A
`noescape Fn() -> fresh T` or `noescape Fn() -> Result<fresh T, E>` callback may
return a constructor, known fresh call, or local value created inside the
callback; it must not return a captured managed or local value as fresh.

`Fn` may also declare positional parameter types:

```rust
fn map(callback: noescape Fn(Int) -> String) -> Unit
```

A closure passed to this parameter must have the same arity, for example
`callback: |value| String.from_int(value: value)`. The callback parameter names
are local to the closure; the contract supplies their positional types. A
callback with the wrong parameter count is a diagnostic before lowering.
Calls to a noescape callback must also match the same positional parameter
contract: inside `fn run(callback: noescape Fn(Int) -> Int)`, `callback("x")`
is rejected before lowering because the first argument is `String`, not `Int`.
The callback body uses the same positional types for expression checking; for
example `callback: |value| value == "x"` is rejected for `Fn(Int) -> Bool`
because the equality operands are `Int` and `String`.
Those positional types also apply to ordinary calls inside callback bodies; for
example `callback: |value| String.len(value: read value)` is rejected for
`Fn(Int) -> Int` because the `String.len` argument expects `String`, not `Int`.

v0.5 `noescape Fn` closures are **non-consuming**: a callee may call the closure
any number of times (for example `ResourcePool.new` calls its factory `max_size`
times), so the closure may `read` or `mut` a captured local but must not `take`
or `manage` a captured local — that would move it on the first call and leave it
gone on the next. Taking or managing a captured local in a `noescape` closure is
a diagnostic. A consuming `FnOnce`-style parameter is a future feature.
The same local/retention rule applies inside the callback body: a local value
created inside a `noescape` callback still cannot be passed to an
`effects(retains(...))` parameter unless it first crosses an explicit `manage`
boundary.

```rust
local seed = Buffer.new(size: 1024)
// rejected: `take seed` inside a closure the callee may call repeatedly
ResourcePool<Conn>.new(create: || Conn.from_seed(seed: take seed), max_size: 16)
```

The frontend reports this as `RS0804`; source that relies on a noescape closure
being called only once is non-conforming.

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
composition of fields that are each valid under the constructor field-effect rules (§9.3) into a fresh shell
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
stored into a handle field through a constructor or explicit mut API
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

In v0.5 these last two are concrete and closed: the only approved resource
container is `ResourcePool<T: Resource>`, and the only standard immediate resource
lease API is `ResourcePool.borrow`. There is no general mechanism for a package to
declare a new approved container or lease API in v0.5; any other container or
lease API is rejected by the v0.5 checker. The extension points are reserved for a
future version, which must define how approval is expressed in `.rssi` and how the
checker recognizes it.

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

`with File.open(...) as file` is valid only when the producer returns a bare
resource `R`. When the producer returns `Result<R, E>`, omitting `?` is a v0.5
diagnostic, not a compatibility warning. RSScript has no legacy source corpus, so
the checker keeps one canonical resource-producer spelling.

### 12.2 Drop points

Resource cleanup has two layers, and the deterministic guarantee does not depend
on the backend's panic strategy.

On every ordinary RSScript control-flow exit, a `with` resource is dropped
deterministically, in reverse order of acquisition:

```text
normal block exit
return
break
continue
? early return (Err propagation)
```

Abnormal termination is the second layer and carries no cleanup guarantee. If the
isolate aborts, the backend process aborts, or a runtime diagnostic terminates
execution, resource cleanup may or may not run depending on the runtime's
termination strategy; RSScript does not promise it. The deterministic-cleanup
guarantee is therefore scoped to ordinary control flow and is independent of
whether the backend unwinds or aborts on panic.

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

This is an exception to the default materialization rules in Chapter 5. Although
`ResourcePool.new` returns `fresh ResourcePool<T>`, a `ResourcePool<T>` may only
materialize in a `local` binding context. Materializing it into a `let` (managed)
binding, a managed container, or a managed field is rejected (diagnostic RS0705),
because a pool owns long-lived resources and must not be hidden behind a managed
binding. The general "let x = fresh_expr materializes as managed" rule does not
apply to `ResourcePool<T>`.

### 12.4 ResourcePool factory contract

This is a hard implementation boundary.

The v0.5 standard ResourcePool factory is eager and noescape.

Conceptual contract for the v0.5 constructor:

```rust
fn ResourcePool<T: Resource>.new(
    create: noescape Fn() -> T,
    max_size: Int,
) -> fresh ResourcePool<T>
```

`new` is the v0.5 constructor and requires an **infallible** factory: `create` must return a resource `T`, never `Result<T, E>`. Construction is eager and exact: the runtime calls `create` exactly `max_size` times, stores the `max_size` resources in the local pool, then discards the factory closure. In v0.5, `max_size` must be a positive `Int` literal; a non-positive literal is a diagnostic (RS0708), not a runtime condition — this keeps `new` infallible without needing a `Result` for a degenerate pool size. Because construction cannot fail, `new` returns the pool directly, not a `Result`. "Eager" and "exactly `max_size`" together remove any ambiguity with lazy replenishment: the pool never creates a resource after construction.

Implementation note (non-normative): a prototype runtime may defensively diagnose
invalid or empty pools, but this is not RSScript source semantics. A conforming
v0.5 frontend rejects non-positive `max_size` literals (RS0708) before lowering.

A fallible factory passed to `new` is rejected (diagnostic RS0707): hiding a creation failure inside `new` would violate no-hidden-behavior, since failure is represented by a return type (section 14.3). The closure literal is checked against the expected `noescape Fn() -> T` parameter contract, including its result type: the user need not annotate the closure's return type, but the checker takes the expected result `T` from the parameter and rejects a factory whose result is `Result<T, E>` (that is the RS0707 case). The `-> T` in the contract is the expected result the checker enforces, not mere documentation.

The canonical example below uses `DbConnection.open` as an *infallible* factory — it returns `DbConnection`, not `Result`, which is what makes it valid with `new`. This is a deliberate simplification: most real poolable resources (database connections, sockets, file handles) fail to create and need the fallible constructor below.

#### Fallible construction: `try_new`

The realistic case is a factory that can fail. `try_new` is part of the v0.5
executable MVP because resource allocation failure must stay explicit instead
of being hidden behind an infallible pool constructor.

```rust
fn ResourcePool<T: Resource>.try_new<E>(
    create: noescape Fn() -> Result<T, E>,
    max_size: Int,
) -> Result<fresh ResourcePool<T>, E>
```

`try_new` is eager like `new`, but because `create` can fail, construction can fail, so it returns a `Result` and the caller writes `?`. Binding semantics for v0.5 lowering and the reference runtime:

```text
1. eager: create is called up to max_size times at construction.
2. on the first create() returning Err, construction stops and returns that Err.
3. partial-construction cleanup: every resource already created before the
   failure must be dropped (its resource cleanup runs) before Err is returned;
   no resource leaks and no half-built pool is exposed.
4. the factory closure is discarded after construction, like new.
```

The constructor space has two axes — eager/lazy and infallible/fallible: `new` is eager+infallible, `try_new` is eager+fallible, and a lazy variant would be a distinct name (`lazy_new` / `retained_new`) with its own contract. A constructor must never silently change which cell it occupies behind the same `.rssi` signature.

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

Exhaustion and nesting, made precise for v0.5:

```text
- borrow does not return Result and must not block.
- a lease is tied to the pool that produced it. While a lease from a pool is
  active, the same pool is held `mut` for the lease's `with` scope, so any
  read/mut/take/manage use of the same pool root inside the lease body is
  rejected (this includes a nested borrow, but also Pool.stats(pool: read pool),
  Pool.reset(pool: mut pool), etc.). A future API may explicitly permit
  introspection or multi-borrow; until then, use one lease per pool at a time.
```

Exhaustion is not expected in ordinary v0.5 source: `max_size` is a positive
literal and nested same-pool borrow is rejected, so a single sequential lease per
pool cannot exhaust it. Borrowing from an exhausted pool is therefore a
**defensive** runtime diagnostic (with a source span, not a block and not a
silent failure) that covers runtime/native/compiler bugs, prototype
non-conformance, or future multi-borrow APIs — not a case ordinary RSScript code
is expected to handle. This is why `borrow` need not return a `Result`.

```rust
with ResourcePool.borrow(pool: mut pool) as a {
    with ResourcePool.borrow(pool: mut pool) as b {   // rejected in v0.5
        ...
    }
}
```

*v0.5 enforcement status: exhausted borrow is enforced at runtime (an empty pool
produces a resource-pool-empty diagnostic with a source span). The static active
lease rule is a frontend obligation: nested borrow and any other read/mut/take
or manage use of the same pool root inside the lease body are diagnostics before
lowering.*

---

## 13. Containers

### 13.1 Managed containers

`List`, `Map`, and `Set` are struct-like container values, not class handles. The
distinction is between binding and field, following the materialization rules
(Chapter 5) and the field rule (§6.5):

```text
- an ordinary container BINDING created by `let` materializes as managed, like
  any non-Copy struct: `let images = List<Image>.new()` is a managed binding.
- a container FIELD is inline by default (stored within its containing value),
  unless the field is marked `handle`. A class may therefore hold an inline
  container field such as `entries: Map<String, Image>`; it is not a separate
  managed handle, it lives within the managed class object.
```

So "managed" describes the default materialization of a container *binding*, not
an intrinsic class-like identity. A `let` container binding is managed:

```rust
let images = List<Image>.new()
```

Managed container bindings may store:

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

A function whose declared return type is not `Unit` must return explicitly on
every fallthrough path. A bare expression statement at the end of a function
body is still a statement, not an implicit function return. Falling through a
non-`Unit` function is a return type mismatch before Rust lowering.

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

`features: async` permits declaring review-visible `async fn` signatures only; it
does not permit executable async bodies.

In v0.5 executable code, a call to an `async fn` is always rejected before
lowering, because the only consumers of an async call — `await` and `spawn` — are
themselves unsupported in v0.5. There is therefore no valid v0.5 fix for an async
call other than removing it. The diagnostic "async call not consumed by await or
spawn" describes the contract of the future executable-async milestone, where a
consumer exists; in v0.5 it should be read as "async calls are not executable
yet," not as a fixable omission.

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

The bound lattice:

```text
Managed   = managed-capable: Copy primitives, class types, and struct types.
            This is the default and the broadest bound. Managed-capable values
            may go in managed bindings and managed containers (List<T>, Map, Set).
Struct    = struct types only (excludes class and excludes resource). A struct
            type is also Managed-capable, so T: Struct implies T: Managed.
Resource  = resource types only. Disjoint from Managed and from Struct: a
            resource is neither managed-capable nor a struct.
```

Consequences:

```text
- T: Struct implies T: Managed, so a T: Struct value may go in List<T>.
- fresh T and local T require T: Struct, because only struct shells are fresh or
  local; a plain T: Managed cannot be returned `fresh T` (a class cannot be
  fresh, §6.2), which is why Result<fresh T, E> on a generic T requires T: Struct.
- Resource generics (ResourcePool<T: Resource>) are a separate world; resource
  types never satisfy Managed or Struct and never enter ordinary containers.
```

### 14.6 Protocols are capability contracts

Terminology note: the capability-contract feature is named **`protocol`**,
not `interface` and not `trait`. The word "interface" in RSScript refers only to
`.rssi` semantic-contract files (the public signature surface). A `protocol` is a
language-level capability that a type can satisfy. The two are related — a
`protocol` is, in effect, a named bundle of `.rssi`-style effect-carrying method
contracts raised to the type level — but they are not the same thing, and the
shared word must not be reused for the language feature.

A `protocol` is an app-layer capability contract, not a general trait system.
The v0.5 MVP supports the static contract surface: protocol declarations,
protocol method signatures, protocol generic bounds, and explicit
`Protocol.method(...)` calls checked against those signatures. Dynamic dispatch
is a future extension described below.

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
protocol Writer {
    fn write(
        self: mut Self,
        message: read String,
    ) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn BufferWriter.write(
    self: mut BufferWriter,
    message: read String,
) -> Unit
    effects(retains(message))

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn write_line<W: Writer>(writer: mut W, message: read String) -> Unit
```

This covers "write code against a capability" and is fully review-resolvable.
The bound name must resolve to a declared protocol. A concrete type satisfies a
protocol only through an explicit `impl Protocol for Type` block. Each mapping
names the protocol method and the concrete function that implements it; the
checker validates parameter names, read/mut/take effects, `Self` substitution,
return type and freshness, `retains(...)`, boundary effects, and guarantee
effects exactly.

#### Dynamic dispatch (admitted, in a reviewable form)

RSScript admits protocol-typed dynamic dispatch (an open set of implementing
types chosen at runtime) as a future feature, not part of the v0.5 executable
MVP. The design decision is settled: dynamic dispatch is supported eventually,
because forbidding it makes users write timidly around capabilities that the
review model can in fact express safely. The constraints below are what make it
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
5. Review classification: a protocol-dynamic call classifies the region as
   `must_review` (single classification, per the §16.2 precedence) with a
   `protocol_dynamic_dispatch` reason, its effects bounded by the protocol
   contract. It is NOT `unknown` (section 16.5), because the effects are known
   even though the concrete type is not.
```

This is why dynamic dispatch passes the feature admission rule (section 2.8):
the concrete type is hidden, but the reviewer question — what does this call
mutate, retain, or own — is answered by the protocol's effect contract, and both
coercion and call stay explicit. Review-first does not require knowing the
concrete callee; it requires knowing the effects, and an effect-carrying protocol
provides exactly that.

### 14.7 `.rssi` interface files

`.rssi` files are compiler-frontend inputs that contain public semantic
contracts. They are not package-manager syntax, not generated Rust headers, and
not implementation files. A `.rssi` declaration uses the same review-critical
signature vocabulary as RSScript source: parameter names, `read`/`mut`/`take`,
return freshness, `effects(retains(...))`, guarantees, `native`, `unsafe`, and
resource contracts.

The compiler frontend owns `.rssi` parsing and canonical contract validation.
Package tooling may select which interface files are active for a package
feature set, but it must then ask the frontend to validate the effective
interface. Package tooling must not implement an independent semantic
normalizer and must not infer RSScript effects from Rust signatures.

Provisional v0.5 interface-only surface:

```text
features: <file features>       # same file-feature gate as source files
opaque struct <Name>            # representation hidden from dependents
opaque class <Name>             # representation hidden, managed identity kind
opaque resource <Name>          # representation hidden, resource kind
pub fn ...                      # bodyless public RSScript contract
native fn ... effects(native)   # bodyless native boundary contract
```

There is no package-level `namespace` shorthand in v0.5. Public contract symbols
use the same fully-qualified canonical names as source files:

```rust
opaque struct Json.JsonValue

pub fn Json.parse(text: read String) -> Result<fresh Json.JsonValue, Json.JsonError>
```

The compiler must reject namespace shorthands instead of normalizing them.
RSScript has no users yet, so there is no compatibility value in keeping alias
spellings that would increase review surface area.

Opaque interface types are distinct from empty ordinary declarations. Their kind
is explicit (`struct`, `class`, or `resource`) and the ordinary kind rules still
apply: classes are managed identity objects, structs are value objects eligible
for freshness/local handling, and resources obey `with`/`ResourcePool` escape
rules. Opaque resource types must use `opaque resource`, not `opaque struct`.

Package features declared in `rsspkg.toml` are package-selection features, not
RSScript file features. A package feature may cause additional `.rssi` files to
be selected by package tooling, but the resulting effective interface is still a
compiler-validated `.rssi` contract.

---

## 15. Native and Unsafe Boundaries

### 15.1 File features

A file without a `features:` declaration is managed-only.

Recognized v0.5 active capability gates:

```text
local
native
unsafe
async
```

Reserved v0.5 review markers:

```text
device
ffi
reflection
```

The reserved markers may be parsed and reported as review risk, but they do not
unlock syntax, lowering, runtime behavior, or package-manager semantics in v0.5.
Feature names are semantic capability gates or reserved review markers, not
library categories. `Json`, `HTTP`, `Image`, and `Regex` are not file features.

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

A native module declaration is shorthand for native declarations under one
namespace. Each method in the module is bodyless, is treated as `native`, and
gets `effects(native)` if the effect is omitted:

```rust
features: native

native module File {
    fn open(path: read Path) -> Result<File, IOError>
    fn open_write(path: read Path) -> Result<File, IOError>
}
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

#### 15.4.1 v0.5 unsafe enforcement

This is what the v0.5 checker enforces, with no contradiction:

```text
- Declaring an effects(unsafe) function requires features: unsafe.
- CALLING an effects(unsafe) function requires features: unsafe in the calling
  file, so a file cannot touch unsafe while looking feature-clean.
- A function that contains an unsafe call is classified must-review (§16.3), and
  so is the file (file-level unsafe feature is high risk).
- No per-call `unsafe` marker is accepted or required in v0.5. There is no
  "missing unsafe marker" diagnostic in v0.5.
```

A function that contains an unsafe call is **not** forced to declare
`effects(unsafe)` itself: establishing a safe, reviewed abstraction over unsafe
operations is the purpose of `unsafe`, the same way a safe function may contain a
Rust `unsafe` block. In v0.5 what keeps unsafe from hiding is the `features:
unsafe` file gate plus must-review classification; a function may still propagate
`effects(unsafe)` when its own contract is unsafe.

#### 15.4.2 Future per-call unsafe marker contract

A future version may add a per-call `unsafe` marker for call-site-line locality
(every unsafe crossing visible where it is read, like `read`/`mut`). It is
deferred, not excluded; v0.5 ships no unsafe code, so it is scheduled for when
unsafe usage makes line-level locality worth its cost. Its fixed contract:

```text
- A call to an effects(unsafe) function is written `unsafe Crypto.raw_copy(...)`.
- The marker requires features: unsafe.
- Marking a non-unsafe call is a diagnostic (the marker must be load-bearing).
- An unsafe call without the marker is a diagnostic.
- The marker is review metadata; it does not change the lowered call.
```

When this lands, §15.4.1 gains the marker rules; until then those rules are not
enforced and must not be, so v0.5 code never writes `unsafe` at a call site.

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

A region carries exactly one **classification** plus a list of **reasons**.
Classification is single-valued and chosen by this precedence (highest wins), so
a region that is both a public API and has an unresolved call is `unknown`, not
`must_review`:

```text
1. unknown            (cannot be classified; an unresolved call wins over all)
2. must_review        (a must-review fact is present, §16.3)
3. review_if_changed  (no must-review fact, but behavior others depend on)
4. low_semantic_risk  (none of the above)
```

`entry_point` is orthogonal: it is a marker on a region, not a point on this
precedence ladder. A region may be an entry point and also `must_review` or
`unknown`; the classification still follows the precedence above, and
`entry_point` is reported as a reason/marker.

Reasons are a list and never collapse: a region may report
`["public_api", "unresolved_call"]` with classification `unknown`. This keeps the
single displayed category deterministic while preserving why.

*v0.5 implementation status: the checker emits `must_review`, `low_semantic_risk`,
and `unknown` as classifications, reports `entry_point` as a marker, and folds
`review_if_changed` into `must_review`; `unknown` propagates to any region that
calls an `unknown` region. A future version may split out `review_if_changed`
under the same precedence.*

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
writes to managed state
writes through handle fields
fresh guarantee boundary
runtime guarantee boundary: no_panic/noalloc/no_block/pure
error handling boundary
removed guarantee
```

An unknown or unresolved external call is **not** a must-review fact. It is the
`unknown` classification itself (it wins the §16.2 precedence over `must_review`),
represented as `classification: unknown` with a reason such as
`["unresolved_external_call"]` — never as a `must_review` fact, so the two
categories do not collide.

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
call argument type mismatch
binding type annotation mismatch
return type mismatch
function fallthrough return type mismatch
control-flow type mismatch
match variant family mismatch
operator type mismatch
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
ResourcePool factory contract violation
ResourcePool max_size not a positive Int literal
ResourcePool active lease conflict
managed object field-split conflict
noescape callback return type mismatch
noescape callback parameter count mismatch
noescape callback call argument mismatch
noescape callback body call argument mismatch
noescape callback body operator type mismatch
noescape closure consuming a captured local
`?` operand error type does not match the function error type
Fd used outside native/resource internals
unknown type in signature or field
unknown field access on a resolved base type
unknown value binding
unknown protocol
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

#### 17.1.1 Diagnostic code registry

Diagnostic codes are `RSnnnn` and stable. They are allocated by range so codes
are not invented ad hoc; new codes join the range matching their concern. This is
the v0.5 allocation (it reflects the implemented codes, not an idealized scheme):

```text
RS00xx  signature / declaration / syntax / effect validity, match exhaustiveness,
        unsupported syntax, async-call-not-consumed
RS01xx  file-feature violations (local / native / unsafe / async gating, incl.
        declaring or calling an effects(unsafe) function without features: unsafe)
RS02xx  call arguments: named/missing/unknown/duplicate args, missing data
        effect, unresolved callee
RS03xx  local / move / same-call conflict; managed->local; manage/take operands
RS04xx  manage boundary (use-after-manage)
RS05xx  retention (local value retained by a retaining API)
RS06xx  freshness
RS07xx  resources, with, and ResourcePool
RS08xx  closures and closure capture (managed-capture, noescape, local closure)
RS09xx  weak and handle fields
RS10xx  forbidden constructs (operator overload, implicit conversion, surface
        references, removed forms)
RS11xx  backend / rustc diagnostic mapping
RS12xx  runtime diagnostics
RS13xx  package / interface contract
```

`RS13xx` covers compiler/frontend diagnostics over `.rssi` interface contracts
and source/interface semantic checking. Package-manager diagnostics use the
separate `PKGxxxx` namespace: manifests, dependency resolution, selected package
features, lockfiles, registry checksums, native binding metadata, native
conformance, and Cargo integration are PKG diagnostics. When a package tool calls
the compiler frontend, it surfaces frontend `RSxxxx` diagnostics unchanged rather
than translating them into PKG diagnostics.

There is no dedicated native/unsafe range: native and unsafe boundaries are gated
through RS01xx (feature) and reported as forbidden/native-binding issues in the
ranges above. A future per-call `unsafe` marker diagnostic (§15.4.2) would also
sit in RS01xx with the other feature/boundary codes.

Every diagnostic class listed in §17.1 is allocated a stable code within the
range matching its concern above (for example the new ResourcePool/`?` classes
sit in RS07xx and RS02xx respectively). Review-map *facts* that are not
diagnostics — such as a "removed guarantee" surfaced by `rss review --diff`
(§16.3) — are review metadata, not RS-coded diagnostics, and do not consume a
code. A complete class→code table is a conformance artifact to be generated from
the implemented code constants, so it cannot drift from the registry.

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

### 17.5 Semantic guarantee table

This table records the enforcement tier for the main v0.5 promises. The tier is
part of the specification contract: implementations must not present a
`review-only` or `unsupported` fact as a static safety guarantee.

| Promise / behavior | v0.5 tier | Enforcement source |
|---|---:|---|
| Named arguments and required `read` / `mut` / `take` data effects | static | frontend checker |
| Same-call conflict roots, including constructor and variant call-like forms | static | frontend checker |
| Local move/use-after-`take` and use-after-`manage` | static | frontend checker |
| Managed-to-local demotion rejection | static | frontend checker |
| Passing local values to retaining APIs, including nested wrappers | static | frontend checker |
| Managed closure capture of local values or resources | static | frontend checker |
| Managed closure capture of managed values as synthetic retention | review-only + static classification | frontend review metadata |
| Fresh return preservation for `fresh T`, `Result<fresh T, E>`, and `Option<fresh T>` | static | frontend checker |
| Resource escape, resource-in-container rejection, and `with` scope boundaries | static | frontend checker |
| Deterministic resource drop on ordinary control-flow exits | dynamic | generated Rust/runtime lowering contract |
| Resource cleanup after isolate abort or runtime termination | unsupported | no v0.5 guarantee |
| `ResourcePool<T>` local-only materialization, eager/noescape factory, and positive literal `max_size` | static | frontend checker |
| Exhausted `ResourcePool.borrow` | dynamic defensive diagnostic | runtime, for non-conforming or future multi-borrow cases |
| Weak field target kind and explicit upgrade requirement | static | frontend checker |
| Managed alias conflicts not visible from source roots | dynamic | runtime managed-access diagnostics |
| `no_panic`, `noalloc`, `no_block`, and `pure` over RSScript-known calls | static over known constructs; review-only over native/runtime internals | frontend checker + trusted signatures |
| Native wrapper semantic behavior beyond declared `.rssi` effects | review-only | package/native metadata, audits, and policy |
| Executable `async` bodies, `await`, and `spawn` | unsupported | frontend diagnostic before lowering |
| Rust build scripts, proc macros, native links, and transitive native facts | package review-only unless specifically scanned or checked | package metadata and policy |

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

Package features declared in `rsspkg.toml` are package selection features. They are not the same as RSScript file features declared with `features:`. If a package feature changes the public surface, package tooling selects a different effective `.rssi` interface and the compiler frontend validates that interface; the package manager still does not define language semantics.

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
    effects(retains(key), retains(value))
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

v0.5 follows a Rust-lowering roadmap, but the roadmap is now a hardening plan
rather than a list of not-yet-started components. The prototype already has the
front-end parser/checker path, deterministic formatting for the supported AST
surface, review metadata, Rust source lowering, source maps, rustc diagnostic
remapping, a small single-isolate runtime, and core `.rssi` interface loading.

Remaining v0.5 work should preserve this dependency order:

```text
spec invariant
  -> frontend checker fact
  -> lowering/source-map shape
  -> runtime behavior when static enforcement is impossible
  -> review metadata and dogfood coverage
  -> package metadata only after the underlying language fact is stable
```

Implementation priorities:

```text
0.5.x  close known static-checker gaps against Chapters 5, 5A, 8, 9, 10, 12, and 17
0.5.x  keep `.rssi` parsing and normalization compiler-owned
0.5.x  keep source maps complete for every user-originating lowered construct
0.5.x  preserve RSScript diagnostics before Rust lowering for unsupported syntax
0.5.x  expand dogfood programs that exercise review maps, package contracts, and diagnostics
0.5.x  keep package-manager features behind stable language facts and normalized interfaces
```

Do not add package-manager shortcuts, lowering placeholders, or compatibility
aliases that contradict the semantic model. Do not defer source mapping until
after lowering.

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
