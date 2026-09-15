# RSScript v0.7 Semantics Reference

Status: source-backed description of the language **as implemented today**.

## How to read this document

`docs/spec/RSScript_v0.7_Spec.md` is the normative statement of intent. This
document is the companion reference: for every rule it names the file in the
implementation that enforces it and, where one exists, the diagnostic code the
checker emits. It exists because the intended author of RSScript programs is a
code-generating model, and a model needs the *exact* accept/reject boundary, not
a summary.

Three conventions are used throughout.

* **Citations.** A rule is followed by the implementing file, e.g.
  (`crates/rsscript-semantics/src/signatures.rs`, `RS0002`). Paths are relative
  to the repository root.
* **Examples.** Every fenced `rsscript` block in this document is a complete,
  self-contained file that was run through
  `cargo run -q -p rsscript-cli --bin rss -- check <file>` while writing this
  reference. Blocks labelled **Accepted** exit 0; blocks labelled **Rejected**
  emit exactly the cited code. A few sections need a companion interface file;
  those are introduced by an `rsscript-interface` block and are checked with
  `rss check --interface <file>.rssi <file>.rss`. Two blocks are labelled
  `rsscript-lint` and are checked with `rss check --lint`.
* **"Unspecified".** Where behaviour could not be confirmed from the
  implementation or from a passing test, this document says *unspecified*
  rather than guessing. Those statements are collected in §12.

The implementation is the authority. The front end is roughly:

```
source text
  → lexer            crates/rsscript-syntax/src/lexer.rs
  → parser           crates/rsscript-syntax/src/parser/
  → AST desugars     crates/rsscript-syntax/src/desugar.rs
                     crates/rsscript-syntax/src/function_value_desugar.rs
                     crates/rsscript-syntax/src/async_await_hoist.rs
  → module isolation crates/rsscript-semantics/src/module_isolation.rs
  → HIR              crates/rsscript-semantics/src/hir/
  → checks           crates/rsscript-semantics/src/checks/
                     plus the standalone rule modules beside them
  → MIR / backends   (outside the scope of this document)
```

---

## 1. Lexical structure, files, and modules

### 1.1 Source characters and tokens

The lexer (`crates/rsscript-syntax/src/lexer.rs`) produces the token kinds
`Ident`, `Number`, `String`, `Char`, `InterpolatedString`, `MultilineString`,
`Keyword`, `Symbol`, `Unknown`, `Eof`.

| Lexical form | Rule |
| --- | --- |
| Identifier | starts with `_` or an ASCII letter; continues with `_` or ASCII alphanumerics. Non-ASCII identifiers are not accepted (`is_ident_start` / `is_ident_continue`). |
| Line comment | `//` to end of line. There is no block-comment form. |
| Integer literal | one or more ASCII digits. No sign, no `_` separators, no hex/octal/binary prefix. |
| Float literal | digits `.` digits. A trailing dot (`5.`) and a second dot (`1.2.3`) are deliberately *not* lexed as one number; the remainder is re-lexed and the parser reports it. |
| String literal | `"…"` with `\` escapes. An unterminated string becomes `Unknown('"')`, so it surfaces as unsupported syntax instead of silently swallowing the file. |
| Interpolated string | `$"… {expr} …"`; `{{` and `}}` are literal braces. |
| Multi-line string | `"""…"""`. |
| Char literal | `'c'`, one `\` escape honoured, terminated by `'`, a newline, or EOF. Exactly one Unicode scalar is required (`RS0038`). |

An invalid source character is kept as an `Unknown` token rather than being
mapped onto a valid operator; the parser then reports `RS0015`.

The reserved-word tables are generated into `docs/generated/keywords.md` and
`docs/generated/grammar.md` from `rsscript-syntax::lexer::KEYWORDS`. Reserved
words are: `as async break class continue drop effects else features fn for
fresh handle if in let local loop manage match mut pub read resource return
struct take weak while with`. `await` and `native` are contextual. `true`,
`false`, `Unit`, `None`, `Ok`, `Err`, `Some` are built-in constants and
constructors, not keywords.

Note the tables do not list every word the parser gives meaning to: `sum`,
`protocol`, `impl`, `type`, `const`, `opaque`, `derives`, `retains`, `noescape`,
`owned`, `captures`, `task_group`, `select`, `spawn`, `use`, `module` are all
recognised by the parser as ordinary identifiers in keyword position.

### 1.2 Literals and their static types

`crates/rsscript-semantics/src/hir/infer.rs` types literals as follows.

| Literal | Inferred type |
| --- | --- |
| `42` | `Int` (a decimal literal that does not fit in `i64` is `RS0033`) |
| `3.5` | `Float` (any numeric literal containing `.`) |
| `"text"`, `"""text"""` | `String` |
| `'x'` | `Char` |
| `true` / `false` | `Bool` |
| `Unit` | `Unit` |
| `[a, b]` | `List<T>` where `T` is the type of the *first* element; an empty list gets the placeholder element type `?`, and an unused binding of one is `RS0034` (§3.3) |
| `{ … }` object literal | `JsonLiteral` |
| `{ k: v }` map literal | `MapLiteral` |

Integer literals are always `Int`; there is no width or signedness inference,
and no literal suffix syntax. `Int` is 64-bit signed
(`crates/rsscript-semantics/src/literals.rs`).

**Accepted**

```rsscript
fn main() -> Unit {
    let n: Int = 42
    let f: Float = 3.5
    let s: String = "text"
    let c: Char = 'x'
    let b: Bool = true
    let u: Unit = Unit
    let list: List<Int> = [1, 2, 3]
    Output.write(message: s)
    return Unit
}
```

**Rejected — `RS0033`**

```rsscript
fn main() -> Unit {
    let n: Int = 99999999999999999999
    Output.write(message: Int.to_string(value: n))
    return Unit
}
```

**Rejected — `RS0038`**

```rsscript
fn main() -> Unit {
    let c: Char = 'ab'
    return Unit
}
```

### 1.3 `.rss` versus `.rssi`

`.rss` files are implementation files; `.rssi` files are interface files. The
distinction is enforced by file extension, not by a declaration
(`crates/rsscript-semantics/src/source_rules.rs`,
`declaration_surface_diagnostics`):

> A function without a body in a file whose path does not end in `.rssi` is
> `RS0015` ("bodyless source function"), *unless* its namespace names a declared
> `protocol` in the same program.

That is the single rule that separates the two file kinds. A bodyless function
in an `.rssi` file is an **external symbol**: the front end records its
signature and resolves calls against it, and binding metadata (outside the
language) maps the symbol to a provider.

Many fixtures under `crates/rsscript-sdk/tests/fixtures/` use bodyless `.rss`
declarations as a shorthand for "assume this API exists"; those fixtures
therefore emit `RS0015` in addition to the diagnostic they are testing. Do not
copy that shape into real source.

**Rejected — `RS0015`**

```rsscript
fn Image.load(path: Path) -> fresh Path
```

The `rss` CLI accepts `--interface <file.rssi>` (repeatable) so a single `.rss`
file can be checked against extra contracts, and `rss check <package-directory>`
for a whole package.

### 1.4 `module` and `use`

A file may declare at most one module identity, and it must precede every other
declaration. `use` declarations follow `module` and precede all other items.
Violations are `RS0015` with a specific label
(`source_rules.rs::module_use_layout_diagnostics`):

| Situation | Label |
| --- | --- |
| second `module` in one file | `duplicate module declaration` |
| `module` after a type/function/const | `misplaced module declaration` |
| `module` after a `use` | `misplaced module declaration` |
| `use` after a type/function/const | `misplaced use declaration` |
| two `use` declarations binding the same local name | `duplicate import name` |

`use` has three forms:

* `use a.b.name` — imports `name` from module `a.b`.
* `use a.b.name as local` — imports under a different local name.
* `use a.b.*` — glob: imports every bare-referenceable name of module `a.b`
  (free functions, constants, types, sums, and type aliases). A glob has no
  single local name and cannot carry an alias.

A `use` path must name a module the compilation unit declares — either a file
being checked or an interface supplied to the check. Otherwise it is `RS0018`
(`module_isolation.rs::unresolved_use_diagnostics`). Resolution is a renaming
pass with no fallback (§1.5), so an import of a module that exists nowhere binds
nothing at all, and without this check a typo in the path stays invisible until
the imported name is used — if it ever is.

Core and standard-package interfaces (`CORE_INTERFACES`,
`STANDARD_PACKAGE_INTERFACES`) declare no `module`: they are the root namespace
and are prelude-visible, so their names are reached without any `use`.

**Accepted** — the interface declaring `module host.fs` is supplied to the check

```rsscript
module app.report

use host.fs.read_all

pub fn title(name: String) -> fresh String {
    return String.concat(left: "Report: ", right: name)
}

fn main() -> Unit {
    Output.write(message: title(name: "q1"))
    return Unit
}
```

**Rejected — `RS0018`** — no file declares `core.text`

```rsscript
module app.report

use core.text.Formatter

pub fn title(name: String) -> fresh String {
    return String.concat(left: "Report: ", right: name)
}

fn main() -> Unit {
    Output.write(message: title(name: "q1"))
    return Unit
}
```

**Rejected — `RS0015`** (module after a declaration)

```rsscript
fn helper() -> Int {
    return 1
}

module app.late
```

**Rejected — `RS0015`** (duplicate import name; also `RS0018` twice, since
neither module exists)

```rsscript
use gadgets.thing
use widgets.thing
```

### 1.5 Module isolation: how names actually resolve

`crates/rsscript-semantics/src/module_isolation.rs` runs on the assembled
`Program` after parsing and merging and before HIR construction. It is a
**renaming pass**, not a scope-tree resolver, which is why the resolution order
is short and total.

1. The pass first runs the two AST desugars for *every* program, module-less
   ones included: `desugar_function_values` (see §4.8) and `hoist_async_awaits`
   (see §9.7).
2. If no file in the program declares `module`, the pass stops. A module-less
   program is the **root namespace** and its names are left completely
   untouched.
3. Otherwise each file is mapped to a module prefix. A dotted path is encoded
   injectively: each segment's own underscores are doubled, then segments are
   joined with a single underscore. So `a.b` → `a_b` and `a_b` → `a__b`, and the
   two can never collide. The separator between prefix and symbol is `__`, so
   `module a.b` + `fn name` lowers to the global symbol `a_b__name`.
4. Every reference is then rewritten by `resolve_bare` in this order:
   1. **the referencing file's own module** — if that module declares the name,
      it wins;
   2. **a `use` import visible in that file** — the local name maps to
      `(module prefix, real name)`, and the mangled symbol uses the *real* name
      so `use a.b.name as local` works;
   3. **otherwise unchanged** — a root symbol, a builtin, an external symbol, or
      an unresolved name.

   There is no outer-module or ancestor-module fallback, and no implicit
   sibling visibility. A name is either local to the module, imported, or global.

Additional facts of the pass:

* `main` is exempt. A function named `main` keeps its global symbol even inside
  a module, so the entry point is always reachable.
* Constants are upper-cased when mangled (`a_b__LIMIT`), matching the Rust
  backend's `SCREAMING_SNAKE_CASE` const lowering.
* A **bodyless** (external) function in a module keeps its source-level dotted
  identity `a.b.name` instead of the mangled symbol, so provider binding
  metadata sees a stable platform-neutral name.
* Sum-type variant names are global: they resolve through their sum type and
  need no import. A qualified `module.Variant` reference rewrites to the bare
  variant.
* `isolate_sources_with_interfaces` merges the source program and its interface
  programs into one module graph, isolates them together (so imports may cross
  the source/interface boundary), and then splits them back apart by file so HIR
  still knows which declarations are host contracts.

### 1.6 Visibility

`pub` marks a declaration public. Public-ness is recorded on functions, types,
sum types, type aliases, and constants (`rsscript-syntax/src/ast.rs`).

Within one checked program, `pub` has exactly one *semantic* consequence today:

> Positional (unnamed) arguments are allowed only for a **private user
> function** or receiver-call shorthand. A call to a `pub` function must name
> every argument (`checks/calls.rs`, `RS0201`).

It also governs what appears in a package's `.rssi` public contract, which is
checked against the implementation by `RS1301` at package granularity. The
checker does **not** reject a cross-module reference to a non-`pub` declaration:
module isolation rewrites the name regardless of `pub`. Cross-module privacy
enforcement is *unspecified* in the current front end.

### 1.7 Reserved names

Declaration leaf names beginning `__rss_` or `__rsscript_` are reserved for
compiler-generated symbols and are rejected (`RS0015`, "reserved declaration
name"). Ordinary double-underscore names that do not use those prefixes are
allowed (fixture `pass/dunder-names-allowed.rss`).

**Rejected — `RS0015`**

```rsscript
fn __rss_helper() -> Int {
    return 1
}
```

### 1.8 Constants

`const NAME: Type = <literal>` declares a program constant. The initializer must
be a literal — a number, a string (single- or multi-line), or `true`/`false`.
Calls and arithmetic in `const` position are `RS0015` ("unsupported const
initializer", `source_rules.rs::declaration_item_surface_diagnostics`).

`const Type.NAME: T = …` declares a **type-associated** constant. The desugar in
`crates/rsscript-syntax/src/desugar.rs` flattens both the declaration and every
`Type.NAME` reference to a single upper-cased identifier (`DEVICE_DEFAULT`), so
the rest of the pipeline sees an ordinary constant.

**Accepted**

```rsscript
const LIMIT: Int = 10

const Device.DEFAULT: String = "cpu"

fn main() -> Unit {
    Output.write(message: Int.to_string(value: LIMIT))
    Output.write(message: Device.DEFAULT)
    return Unit
}
```

**Rejected — `RS0015`**

```rsscript
fn compute() -> Int {
    return 1
}

const LIMIT: Int = compute()
```

### 1.9 Duplicate declarations

Top-level type, constructor, and function names must be unique
(`checks/declarations/duplicate_decls.rs`, `RS0005`). Duplicate *field* names
within a type are the same code.

**Rejected — `RS0005`**

```rsscript
fn helper() -> Int {
    return 1
}

fn helper() -> Int {
    return 2
}
```

### 1.10 Removed surface syntax

`crates/rsscript-semantics/src/source_rules.rs` scans the raw token stream for
three deliberately removed forms, before any resolution:

| Form | Code |
| --- | --- |
| `own struct` | `RS1003` |
| `&T` / `&mut T` in a type position | `RS1004` |
| `expr as Type` outside `use … as` and `with … as` | `RS1002` |

Two more surface rules are token-local as well: `effects(` after a signature is
`RS0015` ("removed effect clause"), and `native fn` / `unsafe fn` / `native
module` / `unsafe module` is `RS0015` ("removed implementation marker").
Implementation origin and host risk belong to package binding metadata, never to
source declarations.

**Rejected — `RS1004`**

```rsscript
fn borrow(value: &Int) -> Unit {
    return Unit
}
```

**Rejected — `RS1002`**

```rsscript
fn main() -> Unit {
    let n = 1
    let f = n as Float
    return Unit
}
```

**Rejected — `RS1003`**

```rsscript
own struct Point {
    x: Int
}
```

---

## 2. Types

### 2.1 The builtin type names

`BUILTIN_TYPE_NAMES` in `crates/rsscript-semantics/src/lib.rs` is the closed set
of names the front end knows without any declaration:

```
Unit Bool Byte Char
Int Int8 Int16 Int32 Int64
UInt UInt8 UInt16 UInt32 UInt64
Float Float32 Float64
String StringView Url Fd
Bytes BytesView Buffer BufferView Path
Result Option List Map Set
Dyn Fn Closure
FileError IOError HttpError JsonError CsvError NetworkError
```

Any other type name must be a declared source/interface type, a type alias, or a
generic parameter in scope; otherwise it is `RS0024`
(`crates/rsscript-semantics/src/external_types.rs`).

`Channel`, `Sender`, `Receiver`, `Stream`, `Pipeline`, `Task`,
`CancellationToken`, `CancellationSource`, `ChannelError`, `JsonValue` and the
rest are **not** builtins. They are declared by the prelude interface files
described in §10 and are therefore available in a single-file check, but they
resolve as ordinary declared types.

`builtin_generic_type_params` (`types.rs`) records the arity of the generic
builtins: `List<T>`, `Set<T>`, `Option<T>`, `Channel<T>`, `Sender<T>`,
`Receiver<T>`, `Stream<T>`, `Pipeline<T>`, `Dyn<P>`, `Map<K, V>`,
`Result<T, E>`, `FalliblePipeline<T, E>`.

**Rejected — `RS0024`**

```rsscript
fn take_it(value: Widget) -> Unit {
    return Unit
}
```

### 2.2 Which types are Copy

`crates/rsscript-semantics/src/value_properties.rs` owns the single source of
truth:

> A type is Copy iff its rendered name contains no `<` and, after stripping a
> leading `fresh `, is one of
> `Bool Byte Char Float Float32 Float64 Int Int8 Int16 Int32 Int64 UInt UInt8
> UInt16 UInt32 UInt64 Unit`.

Everything else — `String`, `Bytes`, `Path`, `List<…>`, `Map<…>`, every struct,
every class, every sum type, every resource, every `Dyn<P>` — is non-Copy.
`String` in particular is **not** Copy.

The Copy set has two direct language consequences: `retains(p)` on a Copy
parameter is `RS0007` (a Copy value has no managed retention boundary, §5.7),
and a Copy value may be snapshotted across an `await` while a non-Copy local may
not (§9.6).

A second, narrower predicate exists beside it: `is_cross_isolate_transferable`
accepts the Copy set plus `String` and `Bytes`. It is used only for
`Channel.message<T>` payloads (`RS0036`, §9.5).

### 2.3 Structs, classes, and resources

There are exactly three type-declaration kinds
(`rsscript-syntax/src/ast.rs::TypeKind`): `struct`, `class`, `resource`. There is
no fourth kind and no `own struct` (`RS1003`).

| | `struct` | `class` | `resource` |
| --- | --- | --- | --- |
| value model | value | managed identity object | linear / move-only RAII |
| may be bound with `local` | yes | **no** (`RS0306`) | only via `with` |
| may be `manage`d | yes | already managed | no (`RS0702`) |
| may be a `fresh` return type | yes | no (`RS0603`) | no (`RS0603`) |
| may be stored in a field | yes | yes | **no** (`RS0701`) |
| may be a generic argument | yes | yes | **no** (`RS0704`) |
| derives allowed | all nine | all nine | `Debug`, `Schema`, `ReviewSchema` only (`RS0212`) |
| user `drop { … }` body | no (`RS0015`) | no (`RS0015`) | yes |
| may live across `await` | if managed | yes | **no** (`RS0031`) |

A `class` is a managed identity object: it is created as a managed handle, so
`local c = SomeClass(...)` is rejected.

Fields are declared `name: Type`, and two qualifiers may precede the type:
`handle Type` (a managed reference field) and `weak Type` (a non-owning weak
handle to a class). A field may carry a default: `name: Type = <expr>`, in which
case the constructor call may omit it.

**Accepted**

```rsscript
struct Value {
    n: Int
}

class Identity {
    n: Int
}

fn bump_struct(value: mut Value) -> Unit {
    value.n = value.n + 1
    return Unit
}

fn Identity.bump(self: mut Identity) -> Unit {
    self.n = self.n + 1
    return Unit
}

fn main() -> Unit {
    local v = Value(n: 0)
    bump_struct(value: mut v)
    let c = Identity(n: 0)
    mut c.bump()
    return Unit
}
```

**Accepted** — field defaults

```rsscript
struct Options {
    retries: Int = 3,
    label: String = "default"
}

fn main() -> Unit {
    let o = Options(retries: 1)
    Output.write(message: o.label)
    return Unit
}
```

**Rejected — `RS0306`**

```rsscript
class Counter {
    total: Int
}

fn main() -> Unit {
    local c = Counter(total: 0)
    return Unit
}
```

An `opaque struct` / `opaque class` / `opaque resource` declaration hides its
representation and must have **no** fields and no `drop` body; anything else is
`RS0015`.

**Rejected — `RS0015`**

```rsscript
opaque struct Secret {
    value: Int
}
```

### 2.4 Sum types

`sum Name { Variant, Variant(field: Type, …) }` declares a nominal sum. Variant
payload fields are **named** in the declaration; positional declaration syntax
does not exist, and a positional *construction* is `RS0015`/`RS0201`.

Construction uses the bare variant name with named arguments:
`Circle(radius: 2)`. Payload-free variants are constructed by their bare name:
`Red`.

Patterns may bind payloads positionally (`Circle(r)`, `Rectangle(w, h)`) in
declared field order, or by name (`Circle { radius }`). See §6.4.

**Accepted**

```rsscript
sum Shape {
    Circle(radius: Int)
    Rectangle(width: Int, height: Int)
}

fn area(shape: Shape) -> Int {
    match read shape {
        Circle(r) => { return read r * read r * 3 }
        Rectangle(w, h) => { return read w * read h }
    }
}

fn main() -> Unit {
    let s = Circle(radius: 2)
    Output.write(message: Int.to_string(value: area(shape: s)))
    return Unit
}
```

### 2.5 `Option` and `Result`

`Option<T>` and `Result<T, E>` are builtin generic types with the builtin
constructors `Some`, `None`, `Ok`, `Err`. They are the only two sum-like
builtins with pattern support beyond scalar literals.

`None` is a value identifier, not a call: `None()` and `None(x)` are `RS0015`.
`Option(...)` and `Result(...)` used as if they were variants are likewise
`RS0015`.

### 2.6 Function types

A function type is spelled `Fn(P1, P2) -> R`. Parameters may carry data effects:
`Fn(read A, mut B, take C) -> R`. An omitted per-parameter effect follows exactly
the same default rule as a declaration parameter (§4.4). A `Fn(...)` with no
`-> R` has no return type (it is not implicitly `Unit` in the structural
representation; `function_return()` returns `None`).

Two qualifiers may precede `Fn` (`ast.rs::TypeRef`, placement enforced by
`source_rules.rs::type_ref_surface_diagnostics`):

| Qualifier | Meaning | Allowed position |
| --- | --- | --- |
| `noescape Fn(...)` | temporary callback; may be called or forwarded to another `noescape` parameter, never stored, returned, retained, or passed as an ordinary value | **direct function parameter only** |
| `owned Fn(...)` | owning/stored closure; escapes the call and may consume its captures | function parameter *and* storable positions: generic argument, struct field, binding, return type |

`noescape` or `owned` anywhere else is `RS0015`. A callback parameter taken by
value must be `owned`: `take Fn(...)` without `owned` is `RS0015` ("unsupported
by-value callback parameter", `by_value_callback_parameter_diagnostic`).

**Accepted**

```rsscript
struct Registry {
    hook: owned Fn(Int) -> Int
}

fn install(hook: take owned Fn(Int) -> Int) -> fresh Registry {
    return Registry(hook: take hook)
}
```

**Rejected — `RS0015`** (`noescape` in a field position)

```rsscript
struct Registry {
    hook: noescape Fn(Int) -> Int
}
```

**Rejected — `RS0015`** (`take Fn` without `owned`)

```rsscript
fn install(hook: take Fn(Int) -> Int) -> Unit {
    return Unit
}
```

### 2.7 `Dyn<P>`

`Dyn<P>` is the dynamic protocol-dispatch type. Its single argument names a
**protocol**, not a value type, and is validated separately
(`external_types.rs::external_binding_type_diagnostics`):

* `Dyn` with an arity other than 1 is `RS0024`.
* The argument must be a bare name — no type arguments of its own, no `Fn`
  shape, no `fresh`/`noescape`/`owned` qualifier — otherwise `RS0024`.
* The name must be a generic parameter in scope or a visible declared protocol;
  otherwise `RS0027`.

The argument is deliberately *not* re-checked as an ordinary type, so a protocol
name never produces a contradictory `RS0024`.

`Dyn.from<P, T>(value: take T) -> fresh Dyn<P>` (declared in
`stdlib/dyn/dyn.rssi`) constructs one, and requires that `T` satisfies `P`
(§7.4).

**Rejected — `RS0027`**

```rsscript
fn render_dynamic(value: read Dyn<Missing>) -> fresh String {
    return "x"
}
```

### 2.8 Type aliases

`type Name = Target` and `type Name<T> = Target<T>` declare aliases. Aliases are
expanded wherever a type is classified. A cycle is `RS0039`
(`crates/rsscript-semantics/src/type_aliases.rs`) — one diagnostic per member of
the cycle.

**Accepted**

```rsscript
type Score = Int

struct Row {
    score: Score
}

fn main() -> Unit {
    let row = Row(score: 3)
    Output.write(message: Int.to_string(value: row.score))
    return Unit
}
```

**Rejected — `RS0039`**

```rsscript
type A = B
type B = A
```

### 2.9 Tuples

Tuple literals and tuple types are surface sugar. The parser rewrites `(a, b)`
to `__Tuple2(item0: a, item1: b)` and `(T, U)` to `__Tuple2<T, U>`;
`desugar.rs::inject_tuple_structs` then injects a synthetic generic struct
`__TupleN<A, B, …>` with fields `item0 … itemN-1` for each arity actually used.
`expand_tuple_destructuring` turns `let (a, b) = e` into a temporary plus
`.itemN` projections.

Because the checker recognises a generic type variable only as a single
uppercase letter, tuple type parameters are `A`, `B`, `C`, …, capping tuple
arity at 26.

**Accepted**

```rsscript
fn pair() -> (Int, String) {
    return (1, "a")
}

fn main() -> Unit {
    let (n, s) = pair()
    Output.write(message: s)
    return Unit
}
```

### 2.10 Nominal versus structural typing

Named types are **nominal**. Two structurally identical structs with different
names are different types, protocol conformance is nominal (§7), and there is no
subtyping and no implicit conversion (`RS1002`).

Types are *represented* structurally, however: `ResolvedType`
(`crates/rsscript-semantics/src/types.rs`) is a `Named { name, arguments }` /
`Function { parameters, parameter_effects, return_type }` tree plus a
`TypeQualifiers { fresh, noescape, owned }` triple, and `TypeArena` interns it,
so `Map<String, Int>` written in two places shares one `TypeId`. Generic
instantiation is structural substitution over that tree
(`ResolvedType::substitute` / `collect_substitutions`).

Type *compatibility* is therefore name-and-argument equality after alias
expansion and substitution, with the known exceptions handled by
`crates/rsscript-semantics/src/type_compatibility.rs` (unresolved generic
placeholders are skipped rather than reported).

### 2.11 `Fd`

`Fd` is a builtin but is not an ordinary user-facing value type. It may appear
only in a native function signature or in resource internals such as a resource
field; anywhere else is `RS0023`
(`crates/rsscript-semantics/src/resource_types.rs::fd_surface_diagnostics`).

**Rejected — `RS0023`**

```rsscript
fn read_fd(handle: Fd) -> Unit {
    return Unit
}
```

### 2.12 Derives

`derives(...)` after a type name requests compiler-owned derives. The supported
set (`crates/rsscript-semantics/src/derives.rs`) is exactly:

```
Debug  Clone  Eq  Ord  Hash  JsonEncode  JsonDecode  Schema  ReviewSchema
```

Anything else is `RS0015`. On a `resource`, only `Debug`, `Schema`, and
`ReviewSchema` are allowed; a value derive is `RS0212`.

`crates/rsscript-semantics/src/derive_fields.rs` then checks that every field
supports the requested derive, recursing through `List`/`Option`/`Result` and
through `Map`/`Set` element types. A failure is `RS0211`. The main rules:

| Derive | Rejected field kinds |
| --- | --- |
| `Eq`, `Ord`, `Hash` | `Float*` fields; `handle`/`weak` fields (they lower to `Managed<T>`, which implements only `Clone`/`Debug`); fields whose own type does not derive the same trait |
| `Ord`, `Hash` | additionally `Map`/`Set` fields |
| `Eq` on a `Map`/`Set` field | requires `Eq + Hash` keys/elements |
| `JsonEncode`, `JsonDecode` | fields whose type does not derive the same |
| `JsonDecode` | additionally non-`Eq`/`Hash` `Map` keys and `Set` elements (e.g. `Float`) |
| any | a generic type parameter in a `Map`-key or `Set`-element position — the required `Hash` bound cannot be expressed |

A plain generic type parameter as an ordinary field is fine: the derive adds the
matching `T: Trait` bound.

**Accepted**

```rsscript
struct Point derives(Eq, Ord, Hash, Clone) {
    x: Int,
    y: Int
}

fn main() -> Unit {
    let mut set: Set<Point> = Set.new()
    Set.insert(set: mut set, value: Point(x: 1, y: 2))
    return Unit
}
```

**Rejected — `RS0211`**

```rsscript
struct Point derives(Ord) {
    x: Float
}
```

**Rejected — `RS0212`**

```rsscript
resource Handle derives(Clone) {
    fd: Int
}
```

### 2.13 Operators

`crates/rsscript-semantics/src/operators.rs` fixes the operand types of the
builtin operators. There is no operator overloading: a user declaration that
looks like one, or an operand that is a type name rather than a value, is
`RS1001`.

| Operator | Operands | Result |
| --- | --- | --- |
| `+ - * / %` | numeric, matching | same numeric type |
| `& \| ^ << >>` | `Int` | `Int` |
| `== !=` | matching known types | `Bool` |
| `< <= > >=` | numeric | `Bool` |
| `&& \|\|` | `Bool` | `Bool` |

A mismatch is `RS0210`.

**Rejected — `RS0210`**

```rsscript
fn main() -> Unit {
    if 1 == "1" {
        return Unit
    }
    return Unit
}
```

**Rejected — `RS1001`**

```rsscript
struct Point {
    x: Int
}

fn bad(right: Point) -> Unit {
    let sum = Point + right
    return Unit
}
```

### 2.14 Unknown fields and bindings

A field access must resolve against the RSScript type known for the base
expression (`RS0025`). A value identifier must resolve to a parameter, a local
binding, a `with`-bound resource, or a pattern binding (`RS0026`). Both are
reported by the front end rather than deferred to generated Rust.

**Rejected — `RS0025`**

```rsscript
struct Point {
    x: Int
}

fn main() -> Unit {
    let p = Point(x: 1)
    Output.write(message: Int.to_string(value: p.z))
    return Unit
}
```

**Rejected — `RS0026`**

```rsscript
fn main() -> Unit {
    Output.write(message: missing_name)
    return Unit
}
```
---

## 3. Type inference

### 3.1 What is never inferred

RSScript deliberately does not infer signatures. Two rules are applied to *every*
function, public or private (`crates/rsscript-semantics/src/signatures.rs`):

* A function must declare an explicit return type — `RS0002`. There is no
  implicit `Unit` return type; write `-> Unit`.
* Every parameter must declare an explicit type — `RS0003`.

Field types, sum-variant payload types, and `const` types are likewise written
out. Inference operates only *inside* a function body.

**Rejected — `RS0002`**

```rsscript
fn helper(value: Int) {
    return Unit
}
```

**Rejected — `RS0003`**

```rsscript
fn helper(value) -> Unit {
    return Unit
}
```

### 3.2 What is inferred

`crates/rsscript-semantics/src/hir/infer.rs` computes a type for an expression
from the expression alone plus the types of the bindings already in scope. It is
a single bottom-up pass, not a unification solver: there are no type variables,
no constraint set, and no backtracking. `infer_hir_expr_type` returns
`Option<ResolvedType>`, and `None` means "unknown", which makes the dependent
checks *skip* rather than report.

| Expression | Inferred type |
| --- | --- |
| identifier | its recorded binding type, else the sum type owning a variant of that name |
| literal | §1.2 |
| `f(args)` | the resolved signature's return type, with generic parameters substituted (§3.4) |
| `Some(x)` | `Option<typeof x>` |
| `Ok(x)` | `Result<typeof x, E>` — the error position is left as the placeholder `E` |
| `Err(x)` | `Result<T, typeof x>` — the ok position is left as the placeholder `T` |
| `Variant(...)` | the declared sum type owning `Variant` |
| `base.field` | the declared field type, with the base's generic arguments substituted |
| `base[i]` | *not inferred* (`None`) |
| `a == b`, `a < b`, `a && b` (comparison and logical) | `Bool` |
| `a + b`, `a << b` (arithmetic and bitwise) | the shared numeric operand type; *not inferred* if the operands are not one matching numeric type |
| `read e` / `mut e` / `take e` / `manage e` | the type of `e` |
| `e?` | the `Ok` type of `e` |
| `await e` | the `Task` payload of `e` if it has one, else the type of `e` |
| `spawn e` | `Task<typeof e>` |
| `match e { … }` | the value type of the *first* arm |
| closure | *not inferred* (`None`) — see §3.5 |

Two consequences are worth stating plainly because they surprise people:

* **A binary expression is typed only when its operands agree.** Comparison and
  logical operators always give `Bool`. The arithmetic and bitwise operators
  give their operand type, but only when both operands are known and are the
  *same* numeric type — there is no operator overloading and no `String +
  String` (§2.13). `let x = a + b` on a mismatched or non-numeric pair leaves
  `x` untyped, because `operators.rs` already reports that pair as
  `RS0210`/`RS1001` and a derived type would only add a second error.
* **An empty list literal has element type `?`.** `let xs = []` gives `xs` the
  type `List<?>`, and `?` makes the dependent checks skip. An *unused* one is
  `RS0034` (§3.3); a used one is trusted, exactly as a used bare `Ok(...)` is.
  Annotate it (`let xs: List<Int> = []`) when the element type matters.

Local bindings do not need annotations when the initializer's type is known:

**Accepted**

```rsscript
fn main() -> Unit {
    let n = 1
    let s = "text"
    let list = [1, 2, 3]
    let total = n + List.len(list: list)
    Output.write(message: Int.to_string(value: total))
    return Unit
}
```

### 3.3 When a binding annotation is required

Because `Ok`, `Err`, `None`, and an empty list literal `[]` each leave one
generic position open, a `let` bound to a bare one of them is only well-typed if
something later constrains the open position. If the binding is never used,
nothing can, and the program would not lower. The checker reports that in
RSScript instead of letting it surface as a backend "type annotations needed"
error: `RS0034` (`ownership.rs::uninferable_binding_type_diagnostic`, applied by
`checks/body/binding.rs::open_generic_initializer`). `Some(x)` and a non-empty
`[x, …]` are fully determined by their contents and are excluded.

**Rejected — `RS0034`**

```rsscript
fn main() -> Unit {
    let value = Ok(1)
    return Unit
}
```

**Rejected — `RS0034`**

```rsscript
fn main() -> Unit {
    let xs = []
    return Unit
}
```

**Accepted** — the annotation closes the open parameter

```rsscript
fn main() -> Unit {
    let value: Result<Int, String> = Ok(1)
    return Unit
}
```

An explicit annotation is also checked against the initializer: a mismatch is
`RS0207` (the same code as an argument mismatch, because it is the same
"initializer must match its declared type" rule).

**Rejected — `RS0207`**

```rsscript
fn main() -> Unit {
    let value: Int = "text"
    return Unit
}
```

### 3.4 Generic instantiation

A generic parameter list is `<T>`, `<T: Bound>`, with `Bound` one of
(`ast.rs::GenericBound`):

| Bound | Meaning |
| --- | --- |
| `T: Managed` | a managed value (class / handle) |
| `T: Struct` | a struct value |
| `T: Resource` | a resource value |
| `T: SomeProtocol` | a single nominal protocol bound |

Exactly one bound per parameter; a malformed or multi-bound parameter list is
`RS0015` ("malformed generic parameter declaration").

`infer_signature_substitutions` (`hir/infer.rs`) builds the substitution map at
each call site from four sources, in this order, first writer wins:

1. **Explicit callee type arguments** — `f<Int>(...)`, `Type.method<Int>(...)`,
   `receiver.method<Int>(...)`. Positionally matched to the signature's type
   parameters.
2. **Namespace type arguments** — for a qualified call `Type<Int>.method(...)`,
   the namespace's own type parameters supply the substitution.
3. **The receiver type** — for receiver-call shorthand, the inferred receiver
   type is structurally matched against the first parameter's declared type.
4. **Argument types** — each named (or, for a positional-allowed call,
   positional) argument's inferred type is structurally matched against its
   parameter's declared type, at any nesting depth.

`collect_substitutions` walks the declared type and the actual type in parallel:
a bare generic name binds to the actual type; matching named types of equal
arity recurse into their arguments; matching function types recurse into
parameters and return type. Nothing else binds.

If the signature has no type parameters, or nothing bound, the return type is
used as written.

There is **no monomorphisation in the front end**. Generic substitution produces
resolved types for checking only; `infer_call_type_arguments` hands the concrete
argument list to backend lowering as structured data so backends never have to
reconstruct an instantiation by parsing a callee spelling. Whether a backend
monomorphises is a backend decision and is outside this document.

Substitution is budgeted (`generic_constraints.rs::SubstitutionBudget`): an
exhausted budget surfaces as `RS0040`, not as a wrong answer.

**Accepted** — inference from an argument, and an explicit type argument

```rsscript
fn first_or<T: Struct>(values: read List<T>, fallback: take T) -> T {
    match List.first(list: values) {
        Some(value) => { return value }
        None => { return take fallback }
    }
}

struct Point {
    x: Int
}

fn main() -> Unit {
    local fallback = Point(x: 0)
    let values: List<Point> = [Point(x: 1)]
    let p = first_or<Point>(values: values, fallback: take fallback)
    Output.write(message: Int.to_string(value: p.x))
    return Unit
}
```

**Accepted** — a `fresh` generic return requires `T: Struct`

```rsscript
struct Box<T: Struct> {
    value: T
}

fn wrap<T: Struct>(value: take T) -> fresh Box<T> {
    return Box(value: take value)
}

struct Point {
    x: Int
}

fn main() -> Unit {
    local p = Point(x: 1)
    let boxed = wrap(value: take p)
    return Unit
}
```

**Rejected — `RS0603`** — `fresh T` without a `Struct` bound

```rsscript
fn wrap<T>(value: take T) -> fresh T {
    return take value
}
```

### 3.5 Protocol bound satisfaction

`generic_constraints.rs::type_satisfies_protocol_bound` decides whether a
concrete type satisfies a protocol bound at a resolved call site. In order:

1. `Dyn<P>` satisfies `P`.
2. Builtin structural facts:
   * `Ord` is satisfied by `Int`, `String`, `Bool`;
   * `Hashable` and `Eq` by `Int*`, `UInt*`, `Bool`, `Byte`, `Char`, `Unit`,
     `String`;
   * `Clone` by all of those plus `Float*`.
3. For `Hashable` / `Eq` / `Clone`, a `List`, `Option`, or `Result` satisfies the
   bound iff every type argument does (recursively).
4. A caller's own type parameter with a matching `T: P` bound satisfies `P`.
5. A visible `impl P for Type` declaration satisfies `P`.
6. A declared derive satisfies the mapped protocol: `Ord` ← `derives(Ord)`,
   `Hashable` ← `derives(Hash)`, `Eq` ← `derives(Eq)` **or** `derives(Ord)`,
   `Clone` ← `derives(Clone)`.

Nothing else. In particular structural method matching never satisfies a bound —
protocols are nominal.

Note the asymmetry in step 2: `Float` is `Clone` but neither `Ord` nor `Eq`, and
`Bool` is `Ord` but `Byte`/`Char` are not. A failure is `RS0032`, with guidance
specialised for `Hashable` and `Eq`.

**Rejected — `RS0032`** — `List.sort<T: Ord>` over a struct with no `Ord`

```rsscript
struct Point {
    x: Float
}

fn main() -> Unit {
    let mut list: List<Point> = [Point(x: 1.0)]
    List.sort(list: mut list)
    return Unit
}
```

**Rejected — `RS0032`** — a `Map` key that is not `Hashable`

```rsscript
struct Key {
    value: Float
}

fn main() -> Unit {
    let mut m: Map<Key, Int> = Map.new()
    Map.insert(map: mut m, key: Key(value: 1.0), value: 1)
    return Unit
}
```

---

## 4. Declarations and calls

### 4.1 Function declarations

```
[pub] [async] fn Name[.Method][<GenericParams>]([Params]) -> ReturnType
    [retains(param)]…
{ body }
```

The pieces, in the order the parser expects them:

* `pub` — visibility (§1.6).
* `async` — makes the function an async function; see §9.
* `Name` or `Type.Method` — a *free function* has no dot; a **qualified method**
  has exactly one dot. The namespace before the dot is a type name (or a
  protocol name for a protocol method).
* Generic parameters `<T>`, `<T: Bound>` (§3.4).
* Parameters `name: Type`, `name: read Type`, `name: mut Type`,
  `name: take Type`, optionally `name: Type = default`. A malformed parameter is
  `RS0015`.
* An explicit return type, optionally `fresh T` (§5.8).
* Zero or more `retains(param)` clauses (§5.7).
* A body — required in `.rss`, forbidden in `.rssi` (§1.3).
* An optional `#lower_name("rust_ident")` attribute immediately above the
  declaration pins the generated backend symbol. It must be a valid Rust
  identifier and must not collide with another declaration's lowered name
  (`RS0035`).

**Accepted**

```rsscript
#lower_name("rss_helper")
fn helper() -> Int {
    return 1
}

fn main() -> Unit {
    Output.write(message: Int.to_string(value: helper()))
    return Unit
}
```

**Rejected — `RS0035`**

```rsscript
#lower_name("not valid")
fn helper() -> Int {
    return 1
}
```

### 4.2 `Type.method` and the static-call convention

There is one call convention. A method is an ordinary function whose name is
`Type.method`, and the canonical call form is the qualified static call with the
receiver passed as a named argument:

```
Point.sum(self: p)
```

Receiver-call shorthand `[effect] receiver.method(args)` is sugar for exactly
that (`ast.rs::Callee::ReceiverCall`): it desugars to
`Type.method(self: <effect> receiver, args…)`, where the effect defaults to
`read` when no keyword is written. The method is resolved from the *inferred
receiver type*, so the receiver must have a known type and the resolved function
must declare a receiver as its first parameter — otherwise `RS0206`.

The receiver parameter does not have to be named `self`; the shorthand binds the
first parameter whatever its name. Naming it `self` is the convention and is the
only way to get the protocol receiver contract (§7.1).

**Accepted** — static call, receiver shorthand, and a static constructor-style
method side by side

```rsscript
struct Point {
    x: Int,
    y: Int
}

fn Point.origin() -> fresh Point {
    return Point(x: 0, y: 0)
}

fn Point.sum(self: read Point) -> Int {
    return self.x + self.y
}

fn main() -> Unit {
    let p = Point.origin()
    let total = Point.sum(self: p)
    let same = p.sum()
    Output.write(message: Int.to_string(value: total + same))
    return Unit
}
```

**Rejected — `RS0206`** — the receiver type has no such method

```rsscript
struct Alpha {
    value: Int
}

struct Beta {
    value: Int
}

fn Alpha.show(self: read Alpha) -> Int {
    return self.value
}

fn Beta.show(self: read Beta) -> Int {
    return self.value
}

fn run(thing: read List<Int>) -> Int {
    return thing.show()
}
```

### 4.3 `self`

`self` is a reserved parameter name (`signatures.rs`, `RS0028`):

* It may appear **only** as the *first* parameter of a **qualified** (`Type.m`)
  signature. A `self` parameter on a free function, or in a non-first position,
  is `RS0028`.
* A **protocol** method must declare `self: read Self`, `self: mut Self`, or
  `self: take Self` as its first parameter. A protocol method with a differently
  named or differently typed first parameter, or with no parameters at all, is
  `RS0028`.

**Rejected — `RS0028`**

```rsscript
protocol Render {
    fn render(value: read Self) -> fresh String
}
```

### 4.4 Labelled arguments and when the label may be omitted

Arguments are named: `f(name: value)`. The rules live in
`crates/rsscript-semantics/src/call_arguments.rs::call_argument_diagnostics`,
with the "may this call use positions" decision in `checks/calls.rs`.

An unnamed argument is `RS0201` **unless** one of three exemptions applies:

| Exemption | Condition |
| --- | --- |
| private helper call | the callee resolves to a **user function** that is **not** `pub`; then arguments bind positionally to parameters in declared order |
| receiver-call shorthand | `x.m(a, b)` — the receiver slot is supplied by the syntax, and the remaining arguments may be positional |
| constructor field shorthand | in a **constructor** call only, a bare *identifier* whose name matches a field name binds to that field (`Point(x)` means `Point(x: x)`) |

Everything else — public functions, core/stdlib functions, interface functions,
protocol calls, and any non-identifier expression in a constructor — must be
named. The constructor shorthand is identifier-only: `Point(compute())` is
`RS0201`.

The remaining shape rules:

| Rule | Code |
| --- | --- |
| a named argument that matches no parameter | `RS0203` |
| a parameter with no default and no supplied argument | `RS0204` |
| the same parameter bound twice | `RS0205` |

**Accepted** — a private helper takes positions; a `pub` one does not

```rsscript
fn add(left: Int, right: Int) -> Int {
    return left + right
}

pub fn scale(value: Int, factor: Int) -> Int {
    return value * factor
}

fn main() -> Unit {
    let a = add(left: 1, right: 2)
    let b = add(1, 2)
    let c = scale(value: 3, factor: 4)
    Output.write(message: Int.to_string(value: a + b + c))
    return Unit
}
```

**Rejected — `RS0201`**

```rsscript
pub fn scale(value: Int, factor: Int) -> Int {
    return value * factor
}

fn main() -> Unit {
    let c = scale(3, 4)
    return Unit
}
```

**Rejected — `RS0203`**

```rsscript
fn add(left: Int, right: Int) -> Int {
    return left + right
}

fn main() -> Unit {
    let a = add(left: 1, rigth: 2)
    return Unit
}
```

**Rejected — `RS0204`**

```rsscript
pub fn add(left: Int, right: Int) -> Int {
    return left + right
}

fn main() -> Unit {
    let a = add(left: 1)
    return Unit
}
```

**Rejected — `RS0205`**

```rsscript
pub fn add(left: Int, right: Int) -> Int {
    return left + right
}

fn main() -> Unit {
    let a = add(left: 1, left: 2, right: 3)
    return Unit
}
```

### 4.5 Default arguments

A parameter may declare a default value: `name: Type = <expr>`. A call may then
omit it, and HIR lowering fills the default in so every backend sees a complete
call. A parameter with a default is not `required`, so omitting it is not
`RS0204`.

**Accepted**

```rsscript
fn greet(name: String, prefix: String = "hi ") -> fresh String {
    return String.concat(left: prefix, right: name)
}

fn main() -> Unit {
    Output.write(message: greet(name: "bo"))
    return Unit
}
```

### 4.6 Call-site data effects: `read` is canonical by omission

The three data effects are `read`, `mut`, `take` (`ast.rs::DataEffect`). The
call-site rule is a single line in `call_argument_diagnostics`:

> A bare argument *is* `read`. If the parameter's effect is `read`, the
> annotation may be omitted or written explicitly. Otherwise the call site must
> spell the parameter's effect exactly.

Formally, with `E` the parameter's effective effect and `A` the annotation
written at the call site (`None` for a bare argument):

| `E` | `A = None` | `A = read` | `A = mut` | `A = take` |
| --- | --- | --- | --- | --- |
| `read` | ok | ok | `RS0202` | `RS0202` |
| `mut` | `RS0202` | `RS0202` | ok | `RS0202` |
| `take` | `RS0202` | `RS0202` | `RS0202` | ok |

The *parameter's* effective effect is computed by `Param::effective_effect`
(`ast.rs`): the written annotation if there is one, otherwise the type's default
data effect. `TypeRef::default_data_effect` returns `read` for every type
**except** `noescape` types, `owned` types, `Closure`, `Fd`, and the legacy
`share` spelling, which have no default and therefore no call-site effect
requirement.

So `fn count(bag: Bag)` and `fn count(bag: read Bag)` are the same signature, and
both accept `count(bag: b)` and `count(bag: read b)`.

The same default applies inside a function type: `Fn(A)` and `Fn(read A)` are the
same (`TypeRef::effective_fn_param_effect`).

Receiver-call shorthand has its own presentation of the rule
(`receiver_call_effect_diagnostics`): the receiver's supplied effect is `read`
when no keyword is written, and a mismatch with the declared receiver parameter
is `RS0202` with a machine-applicable fix that rewrites `x.m(...)` to
`mut x.m(...)`.

**Accepted**

```rsscript
struct Bag {
    items: List<Int>
}

fn count(bag: Bag) -> Int {
    return List.len(list: bag.items)
}

fn count_explicit(bag: read Bag) -> Int {
    return List.len(list: bag.items)
}

fn main() -> Unit {
    let bag = Bag(items: [])
    let a = count(bag: bag)
    let b = count_explicit(bag: bag)
    let c = count(bag: read bag)
    return Unit
}
```

**Rejected — `RS0202`**

```rsscript
struct Bag {
    items: List<Int>
}

fn Bag.push(bag: mut Bag, value: Int) -> Unit {
    List.push(list: mut bag.items, value: value)
    return Unit
}

fn main() -> Unit {
    local bag = Bag(items: [])
    Bag.push(bag: bag, value: 1)
    return Unit
}
```

### 4.7 Return rules

* The declared return type is checked against every `return <expr>` whose type
  is known — `RS0208` (`call_arguments.rs::return_type_mismatch_diagnostic`).
* `Result` and `Option` return *constructors* are checked against the declared
  success / error / `Some` payload type, so
  `-> Result<Int, String>` catches `return Ok("text")`.
* **Falling off the end** of a function whose return type is not `Unit` is also
  `RS0208` ("return in `f` has type `Unit`"). `control_flow.rs` computes
  fall-through structurally: a block may fall through iff *every* statement may.
  `return`, `break`, and `continue` cannot fall through; an `if` with **both**
  branches can fall through iff either branch can; a non-empty `match` or
  `select` can fall through iff any arm can; a `with` can fall through iff its
  body can. Everything else — including `loop` and `while` — is treated as
  falling through, so an infinite `loop { }` with no trailing `return` still
  produces `RS0208`.
* A function whose return type mentions one of its own generic parameters is
  exempt from the fall-through check.
* An explicit bare `return` in a non-`Unit` function is `RS0208`.

**Rejected — `RS0208`** (wrong type)

```rsscript
fn helper() -> Int {
    return "text"
}
```

**Rejected — `RS0208`** (fall-through)

```rsscript
fn helper() -> Int {
    let x = 1
}
```

### 4.8 Named functions as values

RSScript has no first-class function-pointer value. Instead,
`crates/rsscript-syntax/src/function_value_desugar.rs` rewrites a **bare
argument that names a top-level free function** into an equivalent forwarding
closure:

```
List.filter(list: read xs, predicate: is_pos)
// becomes
List.filter(list: read xs, predicate: (x) { return is_pos(x: read x) })
```

The forwarding closure's parameters and per-argument effects are copied from the
named function's declaration. Only *free* functions participate — not
`Type.method` names, and not `main`. The pass runs before module isolation, so
the synthesized call is mangled like any other reference, and it runs for every
program, module-less ones included.

### 4.9 `main`

`main` is the executable entry point. Three facts are enforced or observable:

* Module isolation exempts `main` from mangling, so it keeps a global symbol
  even inside a `module`.
* The register VM looks up the function named `main` to start a program
  (`crates/rsscript-vm/src/reg_vm/executable.rs`).
* `main` is excluded from the free-function-value desugar, so it can never be
  passed as a callback.

The checker imposes **no** signature constraint on `main`: `fn main() -> Unit`,
`fn main() -> Int`, `fn main() -> Result<Unit, String>`, and
`fn main(args: List<String>) -> Unit` all check clean, and a file with **no**
`main` at all also checks clean (`rss check` is a library-friendly check).
Which of those the runner accepts, and how `args` is supplied, is a runner
concern and is *unspecified* by the language front end. Note that `Arguments.*`
(`stdlib/arguments/arguments.rssi`) takes an explicit `args: read List<String>`
parameter precisely so that argument access is never ambient.

All four were checked; the most surprising one is that a file with no entry
point at all is accepted:

**Accepted** — no `main` at all

```rsscript
fn helper() -> Int {
    return 1
}
```

### 4.10 Argument types

When both sides are known, an argument's type must match its parameter's type —
`RS0207`. When either side is unknown (§3.2), the check is skipped rather than
guessed.

**Rejected — `RS0207`**

```rsscript
fn helper(value: Int) -> Unit {
    return Unit
}

fn main() -> Unit {
    helper(value: "text")
    return Unit
}
```

### 4.11 Signature lints

Two warnings are emitted only under `rss check --lint`:

* `RSL001` — a public signature exceeding the review budget. The current budget
  warns above **6** parameters; generic parameters, effect clauses, and nested
  type shapes are counted the same way.
* `RSL002` — a repeated `retains(x)` clause. Repeating a retention declaration
  does not change the contract.

**Warning — `RSL001`**

```rsscript-lint
pub fn wide(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int, j: Int) -> Int {
    return a + b + c + d + e + f + g + h + i + j
}
```

**Warning — `RSL002`**

```rsscript-lint
fn keep(value: String) -> Unit
    retains(value) retains(value)
{
    return Unit
}
```

---

## 5. Ownership and data effects

### 5.1 The two storage modes

Every value in a function body is in one of two modes.

* **Managed** — `let name = expr`. The value lives in the managed runtime and
  may be aliased. Classes are always managed. This is the default.
* **Local exclusive** — `local name = expr`. The value is an exclusive local
  graph. It may be mutated, consumed with `take`, and promoted to managed with
  `manage`, but it may not be aliased or retained.

`let mut name = expr` additionally makes the binding **reassignable** (§6.6);
mutability of the binding is orthogonal to managed-vs-local.

The three transitions:

| Operation | Effect |
| --- | --- |
| `manage local_value` | moves the local graph into managed storage; the local binding is dead afterwards |
| `take local_value` | consumes the local value into a `take` parameter; the binding is dead afterwards |
| managed → local | **not allowed** (`RS0301`) |

**Accepted**

```rsscript
struct Bag {
    items: List<Int>
}

fn make() -> fresh Bag {
    return Bag(items: [])
}

fn count(bag: read Bag) -> Int {
    return List.len(list: bag.items)
}

fn main() -> Unit {
    local bag = make()
    let shared = manage bag
    Output.write(message: Int.to_string(value: count(bag: shared)))
    return Unit
}
```

**Rejected — `RS0301`**

```rsscript
struct Bag {
    items: List<Int>
}

fn main() -> Unit {
    let bag = Bag(items: [])
    local other = bag
    return Unit
}
```

`manage` requires a local binding that has not already become managed
(`RS0307`), and `take` requires a local value — a managed value may have
aliases and so cannot be consumed (`RS0308`).

**Rejected — `RS0307`**

```rsscript
fn main() -> Unit {
    let shared = manage 5
    return Unit
}
```

**Rejected — `RS0308`**

```rsscript
struct Bag {
    items: List<Int>
}

fn consume(bag: take Bag) -> Int {
    return List.len(list: bag.items)
}

fn main() -> Unit {
    let bag = Bag(items: [])
    let n = consume(bag: take bag)
    return Unit
}
```

### 5.2 The three data effects

| Effect | Callee may | Caller sees |
| --- | --- | --- |
| `read` | observe | nothing changes |
| `mut` | observe and mutate | writes propagate back |
| `take` | consume | the argument is dead after the call |

These are ownership semantics, not host effects. `read` is the default for every
ordinary type (§4.6).

**Accepted** — all three, with `local` supplying the exclusive value that `take`
requires

```rsscript
struct Bag {
    items: List<Int>
}

fn count(bag: read Bag) -> Int {
    return List.len(list: bag.items)
}

fn push(bag: mut Bag, value: Int) -> Unit {
    List.push(list: mut bag.items, value: value)
    return Unit
}

fn consume(bag: take Bag) -> Int {
    return List.len(list: bag.items)
}

fn main() -> Unit {
    local bag = Bag(items: [])
    push(bag: mut bag, value: 1)
    let n = count(bag: bag)
    let m = consume(bag: take bag)
    Output.write(message: Int.to_string(value: n + m))
    return Unit
}
```

### 5.3 Use after move

`crates/rsscript-semantics/src/moved_use_flow.rs` computes flow-sensitive
use-after-move facts over checked HIR, and `ownership.rs::moved_use_diagnostic`
renders them as `RS0401`. The check is path-sensitive: a use is an error if it
is reachable on **any** path from the move. The fixture set covers moves inside
`if` branches, loops (including `break`), `match` expression arms, short-circuit
operands, and inline `manage` in an argument position.

Two moves produce the fact: `manage x` and `take x` (including `take x.field`
for an inline field).

**Rejected — `RS0401`**

```rsscript
struct Bag {
    items: List<Int>
}

fn count(bag: read Bag) -> Int {
    return List.len(list: bag.items)
}

fn main() -> Unit {
    local bag = Bag(items: [])
    let shared = manage bag
    let n = count(bag: bag)
    return Unit
}
```

### 5.4 Place conflicts

A single call may not access overlapping places when any of the accesses
mutates or moves. `crates/rsscript-semantics/src/place.rs` classifies the
conflict and produces one of five codes.

| Code | Conflict | Fixture |
| --- | --- | --- |
| `RS0302` | a whole local base (or whole local prefix) mixed with a `mut`/`take` access to one of its fields | `fail/field-whole-base-conflict.rss` |
| `RS0303` | two local field paths where one is the same as, or a prefix of, the other, and either access mutates or takes | `fail/field-prefix-conflict.rss` |
| `RS0304` | an indexed local path against another access to the same local base, when mutation or take is involved — v0.7 does not prove indexed element disjointness | `fail/field-indexed-container-conflict.rss` |
| `RS0305` | a call that moves a local base with `manage`/`take` while another argument accesses one of that base's fields | `fail/field-move-base-take-conflict.rss` |
| `RS0309` | two fields of the same **managed** object split across one call | `fail/managed-field-split-conflict.rss` |

(Fixture paths are relative to `crates/rsscript-sdk/tests/fixtures/`. The two
examples below cover the local and the managed ends of the range; `RS0303`,
`RS0304`, and `RS0305` follow the same shape with a deeper path, an index, and a
`take` respectively.)

Disjoint *inline* fields of a local base are fine (fixture
`pass/field-disjoint-local-inline.rss`); it is the managed case that `RS0309`
closes, because a managed object's fields are reached through one handle.

**Rejected — `RS0302`**

```rsscript
struct Pair {
    left: List<Int>,
    right: List<Int>
}

fn combine(whole: read Pair, part: mut List<Int>) -> Unit {
    return Unit
}

fn main() -> Unit {
    local pair = Pair(left: [], right: [])
    combine(whole: pair, part: mut pair.left)
    return Unit
}
```

**Rejected — `RS0309`**

```rsscript
class Cache {
    a: Buffer
    b: Buffer
}

fn touch(x: mut Buffer, y: mut Buffer) -> Unit {
    return Unit
}

fn run(cache: mut Cache) -> Unit {
    touch(x: mut cache.a, y: mut cache.b)
    return Unit
}
```

### 5.5 `handle` and `weak` fields

A `handle T` field is a managed reference. It cannot be consumed with `take` as
if it were an inline local field — `RS0901`
(`crates/rsscript-semantics/src/take_handle_fields.rs`).

**Rejected — `RS0901`**

```rsscript
struct Rule {
    name: String
}

struct Config {
    rules: handle List<Rule>
}

fn consume(list: take List<Rule>) -> Unit {
    return Unit
}

fn bad_take(config: mut Config) -> Unit {
    consume(list: take config.rules)
    return Unit
}
```

A `weak T` field breaks managed class cycles. Three rules
(`crates/rsscript-semantics/src/resource_types.rs`,
`crates/rsscript-semantics/src/weak_fields.rs`):

| Rule | Code |
| --- | --- |
| the field's type must be a **class** (`fail/weak-non-class-field.rss`) | `RS0902` |
| the field must be initialized from an explicit weak-handle expression such as `Weak.from(value: target)` | `RS0904` |
| the field must be upgraded with `Weak.upgrade(value: field)` before the target value is used | `RS0903` |

`Weak.upgrade` returns `Option<T>`, so a `match` (or `let … else`) is the natural
consumer.

**Accepted**

```rsscript
class User {
    name: String
}

struct Session {
    owner: weak User
}

fn make_session() -> fresh Session {
    let user = User(name: "Ada")
    return Session(owner: Weak.from(value: user))
}

fn main() -> Unit {
    let session = make_session()
    return Unit
}
```

**Accepted** — upgrading before use

```rsscript
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn log(user: read User) -> Unit {
    return Unit
}

fn good_read(session: read Session) -> Unit {
    match Weak.upgrade(value: session.owner) {
        Some(user) => { log(user: user) }
        None => { return Unit }
    }
    return Unit
}
```

**Rejected — `RS0903`**

```rsscript
class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn log(user: read User) -> Unit {
    return Unit
}

fn bad_read(session: read Session) -> Unit {
    log(user: session.owner)
    return Unit
}
```

**Rejected — `RS0904`**

```rsscript
class User {
    name: String
}

struct Session {
    owner: weak User
}

fn make_session() -> fresh Session {
    let user = User(name: "Ada")
    return Session(owner: user)
}
```

### 5.6 `for`-loop element views

A `for` loop variable bound to a non-Copy struct element of a list is a **read
view** into the list, not an owned element. It may be passed as `read`, but
`mut`, `take`, and `manage` are `RS0310` — iteration does not grant exclusive
ownership of elements.

**Rejected — `RS0310`**

```rsscript
struct Row {
    value: Int
}

fn bump(row: mut Row) -> Unit {
    return Unit
}

fn main() -> Unit {
    let rows: List<Row> = [Row(value: 1)]
    for row in rows {
        bump(row: mut row)
    }
    return Unit
}
```

### 5.7 `retains(param)` and escape checking

`retains(param)` on a function declaration states that the call may store the
named argument beyond the call. It is the only source-level declaration effect
in the language, and it exists precisely because it changes escape checking.

Declaration rules (`signatures.rs::collect_retains_diagnostics`):

| Rule | Code |
| --- | --- |
| the retained name must be a parameter of the same function | `RS0007` |
| the parameter must not be Copy (§2.2) — a Copy value has no managed retention boundary | `RS0007` |
| the parameter must not be a `noescape` callback | `RS0802` |
| a repeated `retains(x)` is a lint | `RSL002` |

Call-site rule (`ownership.rs::retained_local_diagnostic`, `RS0501`):

> A **clean local** value may not be passed to a retained parameter, in any
> effect position. Passing it would let local ownership escape.

The same code covers a local value reaching a retained parameter through a field
projection or a wrapper call, and a local captured by a closure that is itself
passed to a retained parameter.

**Accepted**

```rsscript
class Cache {
    entries: Map<String, String>
}

fn Cache.put(self: mut Cache, key: String, value: String) -> Unit
    retains(key) retains(value)
{
    Map.insert(map: mut self.entries, key: key, value: value)
    return Unit
}

fn main() -> Unit {
    let cache = Cache(entries: Map.new())
    mut cache.put(key: "a", value: "b")
    return Unit
}
```

**Rejected — `RS0007`** (unknown parameter)

```rsscript
fn keep(value: String) -> Unit
    retains(other)
{
    return Unit
}
```

**Rejected — `RS0007`** (Copy parameter)

```rsscript
fn keep(value: Int) -> Unit
    retains(value)
{
    return Unit
}
```

**Rejected — `RS0501`**

```rsscript
class Cache {
    entries: Map<String, String>
}

fn Cache.put(self: mut Cache, key: String, value: String) -> Unit
    retains(key) retains(value)
{
    Map.insert(map: mut self.entries, key: key, value: value)
    return Unit
}

fn main() -> Unit {
    let cache = Cache(entries: Map.new())
    local key = String.concat(left: "a", right: "b")
    mut cache.put(key: key, value: "b")
    return Unit
}
```

### 5.8 `fresh`

`fresh T` on a return type states that the returned value does not alias
caller-visible mutable state. It is a **proof obligation on the callee**, checked
by `crates/rsscript-semantics/src/fresh_return_flow.rs` and
`checks/body/fresh.rs`.

`fresh` is only valid for **struct** types. `fresh` on a class or a resource is
`RS0603`, and a generic `fresh T` requires `T: Struct` (the single exception is
`Self` bounded by `Managed`).

The proof accepts, per `fresh_return_not_clean_diagnostic` and
`freshness_unknown_diagnostic`:

* a literal;
* a struct constructor call;
* a call to a function that itself returns `fresh`;
* a **clean local** value — a `local` binding that has not escaped through
  `manage`, `take`, a retained parameter, or a closure capture;
* a clean inline *field* of such a local.

Anything else is either `RS0601` (proved not clean — e.g. returning a `read`,
`mut`, or `take` parameter, a managed value, or a handle field) or the warning
`RS0602` (could not be proved either way). `RS0601` and `RS0602` are
flow-sensitive: the fixtures cover a local that becomes managed inside a loop, a
local retained on one branch, and payloads projected out of `Option`/`Result`
by `match`.

A direct `fresh` expression may materialize as a managed temporary for a `read`
argument, but `mut` and `take` require an explicit `local` binding first —
`RS0604`.

**Accepted**

```rsscript
struct Point {
    x: Int,
    y: Int
}

fn origin() -> fresh Point {
    return Point(x: 0, y: 0)
}

fn main() -> Unit {
    let p = origin()
    Output.write(message: Int.to_string(value: p.x))
    return Unit
}
```

**Rejected — `RS0601`**

```rsscript
struct Point {
    x: Int
}

fn echo(p: read Point) -> fresh Point {
    return p
}
```

**Rejected — `RS0603`**

```rsscript
class Counter {
    total: Int
}

fn make() -> fresh Counter {
    return Counter(total: 0)
}
```

**Rejected — `RS0604`**

```rsscript
struct Point {
    x: Int
}

fn origin() -> fresh Point {
    return Point(x: 0)
}

fn bump(point: mut Point) -> Unit {
    return Unit
}

fn main() -> Unit {
    bump(point: mut origin())
    return Unit
}
```

**Warning — `RS0602`** — the checker cannot prove or disprove freshness of a
payload projected out of an `Option`

```rsscript
struct Point {
    x: Int
}

fn pick(values: read List<Point>) -> fresh Point {
    match List.first(list: values) {
        Some(p) => { return p }
        None => { return Point(x: 0) }
    }
}
```

### 5.9 Closures and capture

There are two closure syntaxes:

```
|params| { body }            // or |params| expr — implicit capture
fn(params) captures(read a, mut b) { body }   // explicit capture list
```

A closure's **kind** comes from how it is bound or passed, not from its syntax:

| Kind | How it arises | May be |
| --- | --- | --- |
| managed closure | `let f = \|…\| { … }`, or passed to an ordinary (non-`noescape`) callback parameter | stored, returned, retained |
| local closure | `local f = \|…\| { … }` | called directly; passed to a `noescape Fn()` parameter |
| noescape callback | a parameter declared `noescape Fn(...)` | called directly; forwarded to another resolved `noescape Fn()` parameter |

The four capture/escape rules
(`checks/body/closure_captures.rs`, `checks/calls/closure_contracts.rs`,
`closure_escape.rs`, `retained_closure_flow.rs`):

| Rule | Code |
| --- | --- |
| a **managed** closure may not capture a clean **local** value — it may outlive it | `RS0801` |
| a `noescape` callback may not be returned, stored, retained, or passed as an ordinary managed value; forwarding is allowed only to another `noescape Fn()` parameter | `RS0802` |
| a `local` closure may not be returned, stored in a managed binding, or passed as an ordinary managed callback (`fail/local-closure-managed-store.rss`) | `RS0803` |
| a `noescape` closure is **non-consuming**: the callee may call it several times, so it may read or mutate a captured local but must not `take` or `manage` it | `RS0804` |

The explicit `fn(...) captures(...)` form adds a fourth code, `RS0805`, with
three shapes:

* the body uses a name that is not declared in `captures(...)` — "closure uses
  `x` without declaring it in captures";
* `captures(...)` declares a name the body never uses;
* the declared capture effect is weaker than the body's actual use. Access
  strength ranks `read < mut < take`; a declared capture is valid when its rank
  is at least the body's actual-usage rank.

A closure's own body-local bindings (`let`, `local`, `with`, `for`, match-arm
patterns) are excluded from its capture set. Nested closures introduce their own
scope.

**Accepted** — a `noescape` parameter lets a callback observe a local

```rsscript
struct Bag {
    items: List<Int>
}

fn make() -> fresh Bag {
    return Bag(items: [])
}

fn report(bag: read Bag) -> Unit {
    Output.write(message: Int.to_string(value: List.len(list: bag.items)))
    return Unit
}

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn main() -> Unit {
    local bag = make()
    apply(callback: || {
        report(bag: bag)
    })
    return Unit
}
```

**Accepted** — explicit captures

```rsscript
fn run() -> Int {
    let offset = 2
    local add = fn(value) captures(read offset) {
        return value + offset
    }
    return add(40)
}

fn main() -> Unit {
    Output.write(message: Int.to_string(value: run()))
    return Unit
}
```

**Rejected — `RS0801`**

```rsscript
struct Bag {
    items: List<Int>
}

fn make() -> fresh Bag {
    return Bag(items: [])
}

fn main() -> Unit {
    local bag = make()
    let callback = || {
        List.len(list: bag.items)
    }
    return Unit
}
```

**Rejected — `RS0802`**

```rsscript
fn store(callback: noescape Fn()) -> Unit {
    let kept = callback
    return Unit
}
```

**Rejected — `RS0804`**

```rsscript
struct Bag {
    items: List<Int>
}

fn make() -> fresh Bag {
    return Bag(items: [])
}

fn consume(bag: take Bag) -> Unit {
    return Unit
}

fn apply(callback: noescape Fn()) -> Unit {
    callback()
    return Unit
}

fn main() -> Unit {
    local bag = make()
    apply(callback: || {
        consume(bag: take bag)
    })
    return Unit
}
```

**Rejected — `RS0805`**

```rsscript
fn run() -> Int {
    let offset = 2
    local add = fn(value) captures() {
        return value + offset
    }
    return add(40)
}
```

### 5.10 Callback contracts

`checks/calls/closure_contracts.rs` additionally checks a `noescape Fn(...)`
argument's *shape* against the parameter's function type before lowering, all
under `RS0207`:

* the callback's arity must match;
* the arguments at each call of the callback must match its parameter types;
* the argument types of calls **inside** the callback body must match;
* the callback's known return expression must match the declared return type,
  including through `if`/`match` branches and nested calls.

This is why a `noescape Fn()` (no declared return) rejects a body whose last
expression produces a value: the expected return is `Unit`.
---

## 6. Control flow

### 6.1 What `control_flow.rs` is responsible for

`crates/rsscript-semantics/src/control_flow.rs` owns the diagnostics that depend
on *resolved HIR types at a control-flow construct*. It does **not** do liveness,
reachability, or definite-assignment analysis; those live in the flow modules
(`local_flow_*.rs`, `moved_use_flow.rs`) and are about ownership, not control
flow. Concretely `control_flow.rs` produces:

* function fall-through and bare-`return` mismatches (`RS0208`, §4.7);
* `if`/`while` conditions that are not `Bool` (`RS0209`);
* `for` iterables that are not `List<T>` (or `Stream<T>` for `await for`)
  (`RS0209`);
* `match` scrutinees outside the supported set (`RS0209`);
* `match` literal-pattern type mismatches and variant-family mismatches
  (`RS0209`);
* `match` expression arms that disagree on a value type (`RS0209`);
* non-exhaustive `match` (`RS0021`, computed in
  `analyzer/exhaustiveness.rs`);
* variant pattern arity mismatches (`RS0037`);
* the structured-pattern effect rules (`RS0202`) described in §6.5.

### 6.2 `if`

`if cond { … } else { … }`. The condition must be `Bool`; there is no truthiness.
`if` is also an **expression**: the parser records `Expr::Match` with
`from_if_expression: true`, so an `if`/`else` used for a value shares the match
expression's typing and lowering.

**Accepted**

```rsscript
fn pick(flag: Bool) -> Int {
    let value = if flag { 1 } else { 2 }
    return value
}

fn main() -> Unit {
    Output.write(message: Int.to_string(value: pick(flag: true)))
    return Unit
}
```

**Rejected — `RS0209`**

```rsscript
fn main() -> Unit {
    let n = 1
    if n {
        return Unit
    }
    return Unit
}
```

### 6.3 Loops

Three loop forms (`ast.rs`):

| Form | Notes |
| --- | --- |
| `loop { … }` | `LoopStmt` with no condition |
| `while cond { … }` | `LoopStmt` with a `Bool` condition |
| `for name in iterable { … }` | `ForStmt`; the iterable must be `List<T>` |
| `await for name in stream { … }` | `ForStmt` with `is_async`; the iterable must be `Stream<T>` |

`break` and `continue` are statements. Each targets the innermost enclosing
`loop`/`while`/`for` *of the same control-flow region*; a closure body starts a
new region, so a `break` written inside a closure does not reach a loop around
the closure. With no such enclosing loop the statement is `RS0016`
(`control_flow.rs::loop_control_flow_diagnostics`). Note that a `loop` is
treated as *possibly falling through* by the return check (§4.7), so an
unconditional `loop { }` at the end of a non-`Unit` function still produces
`RS0208`.

The `for` element binding is a read view for non-Copy struct elements (§5.6).

**Accepted**

```rsscript
fn total(values: read List<Int>) -> Int {
    let mut sum = 0
    for value in values {
        sum = sum + value
    }
    return sum
}

fn main() -> Unit {
    Output.write(message: Int.to_string(value: total(values: [1, 2, 3])))
    return Unit
}
```

**Accepted** — `while`, `continue`, `loop`, `break`

```rsscript
fn main() -> Unit {
    let mut i = 0
    while i < 3 {
        i = i + 1
        if i == 2 {
            continue
        }
    }
    loop {
        break
    }
    return Unit
}
```

**Rejected — `RS0209`**

```rsscript
fn main() -> Unit {
    let n = 1
    for x in n {
        Output.write(message: "x")
    }
    return Unit
}
```

**Rejected — `RS0016`** (twice: once for `break`, once for `continue`)

```rsscript
fn main() -> Unit {
    let n = 1
    if n > 0 {
        break
    }
    continue
    return Unit
}
```

### 6.4 Pattern forms

`ast.rs::MatchPattern` has six forms.

| Pattern | Syntax | Notes |
| --- | --- | --- |
| binding | `name` | irrefutable; binds the whole scrutinee |
| wildcard | `_` | irrefutable |
| variant | `None`, `Some(v)`, `Ok(e)`, `Circle(r)`, `Rectangle(w, h)` | positional sub-patterns bind declared fields in declared order |
| struct | `Point { x, y }`, `Point { x, .. }`, `Point { x: inner_pattern }` | named fields; `..` allows omitting the rest |
| literal | `0`, `"text"`, `'c'`, `true` | `Int`, `String`, `Char`, `Bool` only |
| list | `[]`, `[a, b]`, `[first, ..rest]`, `[..init, last]`, `[a, ..mid, z]` | a rest may be ignored (`..`) or bound (`..name`) |

Arms may carry a guard: `pattern if cond => { … }`. A guarded arm does **not**
count toward exhaustiveness, and a mutating effect inside a guard is rejected
(`match_guard_mutation_diagnostic`).

Variant pattern arity is checked: a bare `V` matches a payload-free variant, but
once a parenthesised payload is written its arity must equal the variant's
declared field count — `RS0037`. Positional binding exists only for declared
sum-variant fields; it does not reintroduce anonymous positional records.

**Accepted** — guards

```rsscript
fn classify(value: Option<Int>) -> String {
    match value {
        Some(n) if n > 0 => { return "pos" }
        Some(n) => { return "nonpos" }
        None => { return "none" }
    }
}

fn main() -> Unit {
    Output.write(message: classify(value: Some(1)))
    return Unit
}
```

**Accepted** — scalar literal dispatch

```rsscript
fn name(code: Int) -> String {
    match code {
        0 => { return "zero" }
        1 => { return "one" }
        _ => { return "other" }
    }
}

fn main() -> Unit {
    Output.write(message: name(code: 1))
    return Unit
}
```

**Accepted** — list patterns

```rsscript
fn head(values: read List<Int>) -> Int {
    match read values {
        [] => { return 0 }
        [first, ..rest] => { return read first }
    }
}

fn main() -> Unit {
    Output.write(message: Int.to_string(value: head(values: [1, 2])))
    return Unit
}
```

**Rejected — `RS0037`**

```rsscript
sum Shape {
    Rectangle(width: Int, height: Int)
}

fn area(shape: Shape) -> Int {
    match read shape {
        Rectangle(w) => { return read w }
    }
}
```

### 6.5 Scrutinee effects on structured patterns

A `match` may carry a scrutinee effect: `match read x { … }`,
`match mut x { … }`, `match take x { … }`. The rule
(`control_flow.rs::structured_match_effect_diagnostic`, `RS0202`):

> A **structured** pattern — one that projects a field place, i.e. a struct
> pattern, a positional variant pattern with bindings, or a list pattern —
> requires an explicit scrutinee effect. Literal patterns, bare variant names,
> plain bindings, and wildcards do not project a place and are outside the rule.

Additional per-field rules in the same family:

| Rule | Diagnostic |
| --- | --- |
| a field pattern requesting `mut`/`take` on a **managed class** value | `managed_pattern_field_effect_diagnostic` |
| a field effect stronger than the scrutinee effect | `weakened_pattern_field_effect_diagnostic` |
| the same field projected twice when either projection mutates or takes | `conflicting_pattern_field_effect_diagnostic` |
| the same field listed twice | `duplicate_pattern_field_diagnostic` |
| a field name not declared by the type | `unknown_pattern_field_diagnostic` |
| declared fields omitted without `..` | `omitted_pattern_fields_diagnostic` |

Note the asymmetry that catches people out: `match value { Some(n) => … }` on an
`Option<Int>` is fine without an effect, but `match point { Point { x } => … }`
is `RS0202`.

**Rejected — `RS0202`**

```rsscript
struct Point {
    x: Int,
    y: Int
}

fn describe(point: read Point) -> Int {
    match point {
        Point { x, y } => { return x + y }
    }
}
```

### 6.6 Supported scrutinee types

`match_scrutinee_diagnostic` accepts, after alias expansion:

* `Option<T>`, `Result<T, E>`, `List<T>`;
* the scalars `Int`, `String`, `Char`, `Bool` (literal dispatch);
* any declared sum, struct, or class type.

Everything else — including `Float` — is `RS0209`. A literal pattern whose
literal type differs from the scrutinee type is also `RS0209`, and a variant
name outside the scrutinee's variant family is `RS0209`
(`match_variant_family_diagnostic`).

**Rejected — `RS0209`**

```rsscript
sum Shape {
    Circle(radius: Int)
}

fn main() -> Unit {
    let s = Circle(radius: 1)
    match read s {
        Square(x) => { return Unit }
    }
}
```

### 6.7 Exhaustiveness

`crates/rsscript-semantics/src/analyzer/exhaustiveness.rs` decides coverage.
`RS0021` is emitted when it cannot prove the arms cover the scrutinee type.
Guarded arms are excluded from the calculation.

The algorithm, per scrutinee type:

| Scrutinee | Covered when |
| --- | --- |
| any | some arm is a wildcard or a plain binding |
| `Bool` | both `true` and `false` literal patterns appear |
| `Option<T>` | a `None` pattern appears, **and** either a `Some` with an irrefutable payload (`Some(x)` with no sub-pattern list, i.e. `bindings.is_empty()`) or the collected `Some` sub-patterns cover `T` |
| `Result<T, E>` | the same, for `Ok` over `T` and `Err` over `E` |
| `List<T>` | some rest pattern caps the open tail, and every shorter length below that rest pattern's minimum is covered by a fixed-length pattern |
| declared sum | every variant is covered, each by patterns that cover its fields |
| declared struct / class | the field patterns cover the product of the fields' finite witness domains |
| anything else | the builtin `Some`/`None`, `Ok`/`Err` rule |

For structs, sums with payloads, and nested patterns the checker builds a
**witness product** over the fields' finite domains (`Bool` → two witnesses,
`Option` → `Some`/`None`, and so on) and requires every witness row to be
matched. The product is capped at 512 rows; beyond that the match is treated as
*not* provably exhaustive, so a very wide product needs an explicit `_`.

Because the `List` rule needs a rest pattern to close the tail, a `match` over a
list with only fixed-length arms is never exhaustive.

**Rejected — `RS0021`**

```rsscript
fn main() -> Unit {
    let value: Option<Int> = Some(1)
    match value {
        Some(n) => { Output.write(message: "some") }
    }
    return Unit
}
```

**Rejected — `RS0021`**

```rsscript
sum Shape {
    Circle(radius: Int)
    Rectangle(width: Int, height: Int)
}

fn area(shape: Shape) -> Int {
    match read shape {
        Circle(r) => { return read r }
    }
}
```

### 6.8 `match` as an expression

A `match` in value position produces a value. The expression's type is taken
from the **first arm that produces a value**, and every other arm must agree —
`RS0209` ("match arm has type `X`, expected `Y` from the first produced arm").

**Accepted**

```rsscript
fn describe(value: Option<Int>) -> String {
    let label = match value {
        Some(n) => { "some" }
        None => { "none" }
    }
    return label
}

fn main() -> Unit {
    Output.write(message: describe(value: None))
    return Unit
}
```

### 6.9 `let … else`

`let <pattern> = <expr> else { … }` binds when the pattern matches and runs the
else block otherwise. The else block must diverge (return, break, continue).

**Accepted**

```rsscript
fn first(values: read List<Int>) -> Int {
    let Some(value) = List.first(list: values) else {
        return 0
    }
    return value
}

fn main() -> Unit {
    Output.write(message: Int.to_string(value: first(values: [7])))
    return Unit
}
```

### 6.10 The `?` operator

`expr?` propagates a failure out of the enclosing function. Two independent
checks apply (`crates/rsscript-semantics/src/try_checks.rs`):

* **Operand** — the operand's type must be `Result<…>` or `Option<…>`. Applying
  `?` to anything else whose type is known is `RS0013`. An operand whose type is
  unknown is skipped.
* **Propagation target** — the enclosing function must return `Result<…>` or
  `Option<…>`, so the failure case has somewhere to go. `?` in a function with
  any other concrete return type is `RS0013`.
* **Error type** — inside a function returning `Result<T, E>`, every `?` on a
  `Result<_, F>` must have `F` compatible with `E`; otherwise `RS0013`.

There is no `From`-style error conversion: the error types must match.

The propagation-target check is deliberately conservative
(`try_checks.rs::TryContext::from_return_type`). It classifies the declared
return type with type aliases expanded, and derives no obligation at all when
the return type is a bare type parameter of the function, still carries an
unresolved generic placeholder, or is a bare `Result`/`Option` with no
arguments. A `?` inside a **closure body** is also never reported this way: the
closure has its own return contract, which this check does not model.

**Accepted**

```rsscript
fn parse(text: String) -> Result<Int, String> {
    return Ok(1)
}

fn double(text: String) -> Result<Int, String> {
    let value = parse(text: text)?
    return Ok(value * 2)
}
```

**Rejected — `RS0013`** — the operand is not a `Result`/`Option`

```rsscript
fn load(value: Int) -> Int {
    return value
}

fn wrap(value: Int) -> Result<Int, String> {
    let loaded = load(value: value)?
    return Ok(loaded)
}
```

**Rejected — `RS0013`** — the enclosing function cannot propagate the failure

```rsscript
fn parse(text: String) -> Result<Int, String> {
    return Ok(1)
}

fn double(text: String) -> Int {
    let value = parse(text: text)?
    return value * 2
}
```

### 6.11 Assignment

`x = e`, `obj.field = e`, and `list[i] = e` are the controlled assignment forms
(`crates/rsscript-semantics/src/assignment.rs`).

| Rule | Code |
| --- | --- |
| the root must be a `let mut` local — a plain `let`/`local` binding is immutable, a parameter is not a reassignable local, and an unknown name has no place | `RS0311` |
| the left side must be a place expression (local, field, or index), not a call result | `RS0311` |
| index assignment is executable only for `List`; other indexed targets (e.g. `Map`) are deferred — use `Map.insert` | `RS0312` |
| when both sides are known, the value type must match the place type | `RS0313` |

Mutating a `mut` parameter is done through a call or a field update, not by
rebinding the parameter.

**Accepted**

```rsscript
struct Row {
    values: List<Int>
}

fn main() -> Unit {
    let mut row = Row(values: [1, 2])
    row.values[0] = 5
    row.values = [3]
    return Unit
}
```

**Rejected — `RS0311`**

```rsscript
fn main() -> Unit {
    let n = 1
    n = 2
    return Unit
}
```

**Rejected — `RS0312`**

```rsscript
fn main() -> Unit {
    let mut m: Map<String, Int> = Map.new()
    m["a"] = 1
    return Unit
}
```

**Rejected — `RS0313`**

```rsscript
fn main() -> Unit {
    let mut n = 1
    n = "text"
    return Unit
}
```

### 6.12 Definite assignment

A `let` with a type annotation and no initializer is a **deferred declaration**:
the binding exists but holds no value. Reading it before anything assigns it is
`RS0017` (`control_flow.rs::definite_assignment_diagnostics`).

The analysis is deliberately weaker than full definite assignment, and the exact
rule is:

* The body is walked in source order. Within one statement, the reads in its
  expressions are seen before that statement's own assignment takes effect — so
  `x = x + 1` on a deferred `x` is a read of an unassigned binding.
* **Any** assignment to the name earlier in that walk marks it assigned from
  then on, including an assignment inside one arm of an `if`, one `match` arm,
  or a loop body that may run zero times. The analysis is therefore *optimistic
  about paths*.
* A later `let` of the same name with an initializer also marks it assigned.
* Closure bodies are not walked: a closure runs at a time the check does not
  model.
* `let … else` bindings are not deferred declarations. They lower to a `let`
  with no value in HIR, so the deferred set is read off the *syntax* tree to
  keep the two apart exactly.

The consequence is that every read `RS0017` reports is unassigned on *every*
path: branch merging produces no false positives, at the cost of missing reads
that are unassigned on only some paths.

**Rejected — `RS0017`**

```rsscript
fn main() -> Unit {
    let x: Int
    Output.write(message: Int.to_string(value: x))
    return Unit
}
```

**Accepted** — one assigning path is enough

```rsscript
fn main() -> Unit {
    let mut x: Int
    let c = true
    if c {
        x = 1
    } else {
        x = 2
    }
    Output.write(message: Int.to_string(value: x))
    return Unit
}
```

---

## 7. Protocols

### 7.1 Declaring a protocol

```
protocol Name {
    fn method(self: read Self, …) -> ReturnType
    …
}
```

A protocol declaration contributes a `ProtocolDecl` (just a name and span) plus
one bodyless `Protocol.method` function per method. That is why a protocol
method is the one exception to the "no bodyless functions in `.rss`" rule
(§1.3), and why a protocol method with a body is `RS0015` (fixture
`fail/protocol-default-method-body.rss`) — there are no default method bodies.

Every protocol method must declare `self: read Self`, `self: mut Self`, or
`self: take Self` as its first parameter (`RS0028`, §4.3).

Generic protocols and generic protocol implementations are reserved: a `<` two
tokens after `protocol` or `impl` is `RS0015` ("generic protocol declaration").
Use a function generic with a protocol bound instead.

### 7.2 Declaring conformance

```
impl Protocol for Type {
    method = Type.method
    …
}
```

An `impl` block maps each protocol method name to a concrete function name. The
checker (`checks/declarations/signatures.rs`,
`crates/rsscript-semantics/src/protocol_bounds.rs`) verifies:

* the protocol is declared and visible — otherwise `RS0027`;
* the implementing type is declared — otherwise `RS0024`;
* every protocol method is mapped, and every mapping names a declared function;
* each mapped function's signature matches the protocol method's signature
  (`protocol_signature_mismatch`).

Conformance is **nominal**. A type with a method of the right name and shape does
not conform; there must be an `impl`, a matching generic bound, or a derive
(§3.5).

### 7.3 Static dispatch

A protocol method is called as `Protocol.method(self: value, …)`. The receiver's
type must be proven to satisfy the protocol by §3.5 — through an explicit
generic bound `<T: Protocol>`, a visible `impl`, a derive, or `Dyn<Protocol>`.
Otherwise `RS0032`.

**Accepted**

```rsscript
protocol Render {
    fn render(self: read Self) -> fresh String
}

struct Label {
    text: String
}

fn Label.render(self: read Label) -> fresh String {
    return String.concat(left: self.text, right: "!")
}

impl Render for Label {
    render = Label.render
}

fn render_static<T: Render>(value: read T) -> fresh String {
    return Render.render(self: value)
}

fn render_dynamic(value: read Dyn<Render>) -> fresh String {
    return Render.render(self: value)
}

fn main() -> Unit {
    let label = Label(text: "hi")
    Output.write(message: render_static(value: label))
    return Unit
}
```

**Rejected — `RS0032`** — no `impl`, no bound

```rsscript
protocol Render {
    fn render(self: read Self) -> fresh String
}

struct Label {
    text: String
}

fn show(value: read Label) -> fresh String {
    return Render.render(self: value)
}
```

### 7.4 Dynamic dispatch with `Dyn<P>`

`Dyn<P>` is the dynamic protocol value. A `Dyn<P>` satisfies `P` (step 1 of
§3.5), so `Protocol.method(self: dyn_value)` type-checks. `Dyn<P>` carries no
permission or authority; it is purely a dispatch boundary.

Construction is `Dyn.from<P, T>(value: take T) -> fresh Dyn<P>`
(`stdlib/dyn/dyn.rssi`).

**Accepted**

```rsscript
protocol Render {
    fn render(self: read Self) -> fresh String
}

struct Label {
    text: String
}

fn Label.render(self: read Label) -> fresh String {
    return String.concat(left: self.text, right: "!")
}

impl Render for Label {
    render = Label.render
}

fn show(value: read Dyn<Render>) -> fresh String {
    return Render.render(self: value)
}

fn main() -> Unit {
    local label = Label(text: "hi")
    let boxed = Dyn.from<Render, Label>(value: take label)
    Output.write(message: show(value: boxed))
    return Unit
}
```

`Dyn.from<P, T>` requires that `T` satisfies `P` — either through a visible
`impl P for T` or through a declared protocol bound on a type parameter
(`fn box_any<T: Render>(value: take T) -> fresh Dyn<Render>` is accepted). The
check is `checks/calls/generic_constraints.rs::check_dyn_from_call`, and the
diagnostic is `generic_constraints.rs::dyn_from_diagnostic` (`RS0032`). The
following program — identical except that the `impl Render for Label` block is
deleted — is **rejected**:

**Rejected — `RS0032`**

```rsscript
protocol Render {
    fn render(self: read Self) -> fresh String
}

struct Label {
    text: String
}

fn show(value: read Dyn<Render>) -> fresh String {
    return Render.render(self: value)
}

fn main() -> Unit {
    local label = Label(text: "hi")
    let boxed = Dyn.from<Render, Label>(value: take label)
    Output.write(message: show(value: boxed))
    return Unit
}
```

### 7.5 Generic bounds versus `Dyn`

| | `fn f<T: P>(x: read T)` | `fn f(x: read Dyn<P>)` |
| --- | --- | --- |
| dispatch | static, resolved per instantiation | dynamic |
| conformance proof | at the call site, by §3.5 | when the `Dyn` is constructed, by §3.5 (`RS0032`) |
| number of bounds | exactly one per type parameter | one protocol per `Dyn` |
| value shape | the concrete value | an explicit boundary value |

---

## 8. Resources

### 8.1 `resource` types

A `resource` is a move-only RAII value with a linear lifetime. It may declare a
`drop { … }` body, which is the only place a user-observable destructor exists —
a `drop` body on a `class` or `struct` is `RS0015` ("unsupported managed drop").

The restrictions, collected:

| Rule | Code |
| --- | --- |
| a resource may not be stored in an ordinary class or struct field | `RS0701` |
| a resource may not be an ordinary generic argument (`List<File>`, `Option<File>`, …) | `RS0704` |
| a resource generic *parameter* must declare an explicit bound | `RS0704` |
| only `Debug`, `Schema`, `ReviewSchema` derives | `RS0212` |
| `fresh Resource` is invalid | `RS0603` |
| a resource may not live across an `await` | `RS0031` |

A generic container intended to hold resources must declare the bound
explicitly: `resource Pool<T: Resource> { … }`.

### 8.2 Resources can only be produced by external functions

This is the single most surprising rule in the language, and it is worth stating
directly:

> A resource-producing expression must be consumed by `with`. A `return` of a
> resource constructor is a resource producer escaping its context, so a
> function *body* cannot construct and return a resource. In practice this means
> a resource is always produced by a bodyless function declared in an `.rssi`
> interface.

`crates/rsscript-semantics/src/resource_producers.rs` classifies an expression as
a producer when it is a call whose type is a resource
(`ResourceProducerKind::Resource`) or a call returning `Result<Resource, E>`
(`ResourceProducerKind::ResultResource`). Anything but a `with` head — a `let`
binding, a `return`, an argument position — is `RS0702`.

The examples in this section therefore use a companion interface file. They were
checked with
`rss check --interface fs.rssi <file>.rss`:

```rsscript-interface
resource File {
    fd: Int
}

struct IOError {
    message: String
}

pub fn File.open(path: String) -> File

pub fn File.open_result(path: String) -> Result<File, IOError>

pub fn File.stat(file: read File) -> Int

pub fn File.write(file: mut File, data: String) -> Unit
```

### 8.3 `with … as x`

`with <producer> as name { body }` opens a bounded resource scope. The resource
is bound to `name` for the body and is cleaned up on **every** exit from the
block.

When the producer returns `Result<Resource, E>`, the `?` must be written
explicitly at the `with` head — `RS0706`. Omitting it means the scope would bind
a `Result`, not a resource.

`with` blocks nest, and the inner scope closes first. Cleanup order is
lexical/LIFO: the innermost `with` scope is released first, then outward.
Per `docs/spec/RSScript_Execution_Spec_v0.1.md`, normal return, `?`
propagation, provider error, deadline, and cancellation all converge on the same
resource-slot cleanup path, and a provider-owned resource is released according
to its declared cleanup contract.

**Accepted**

```rsscript
fn copy(path: String) -> Unit {
    with File.open(path: path) as file {
        File.write(file: mut file, data: "x")
    }
    return Unit
}
```

**Accepted** — `Result` producer with the required `?`

```rsscript
fn copy(path: String) -> Result<Unit, IOError> {
    with File.open_result(path: path)? as file {
        File.write(file: mut file, data: "x")
    }
    return Ok(Unit)
}
```

**Rejected — `RS0706`**

```rsscript
fn copy(path: String) -> Result<Unit, IOError> {
    with File.open_result(path: path) as file {
        let n = File.stat(file: file)
    }
    return Ok(Unit)
}
```

### 8.4 Escape rules

`crates/rsscript-semantics/src/resource_flow.rs` computes escape facts over HIR.
A resource introduced by `with` must not leave the block. `RS0702` covers all of:

* `return`ing the resource;
* binding it into a managed binding (`let shared = file`);
* `manage file`;
* passing it to a `retains(...)` parameter;
* capturing it in a managed closure (directly or through a wrapper);
* passing it with `take` to something that outlives the scope;
* a resource producer evaluated outside a `with` head at all.

**Rejected — `RS0702`** — `manage` inside the scope

```rsscript
fn leak(path: String) -> Unit {
    with File.open(path: path) as file {
        let shared = manage file
    }
    return Unit
}
```

**Rejected — `RS0702`** — a producer that is never given to `with`

```rsscript
fn leak(path: String) -> Unit {
    let file = File.open(path: path)
    return Unit
}
```

**Rejected — `RS0701`**

```rsscript
struct Holder {
    file: File
}
```

**Rejected — `RS0704`**

```rsscript
fn keep(files: List<File>) -> Unit {
    return Unit
}
```

### 8.5 No resource across `await`

A resource may not be live across a suspension point — `RS0031`. This is the
resource half of the await-liveness rule described in §9.6.

**Rejected — `RS0031`**

```rsscript
struct NetworkError {
    message: String
}

async fn fetch() -> Result<Int, NetworkError> {
    return Ok(1)
}

async fn run(path: String) -> Result<Int, NetworkError> {
    with File.open(path: path) as file {
        let value = await fetch()?
        return Ok(value)
    }
    return Ok(0)
}
```

### 8.6 Transfer

A resource moves. The only transfers the front end permits are:

* into the `with` binding from its producer;
* by `take` into a parameter *that does not outlive the scope* — a `take`
  parameter on a function called inside the `with` body is fine, and the fixture
  set exercises it, but `take` into anything that stores the value is `RS0702`;
* by `mut` / `read` into a call, which does not transfer ownership at all.

There is no way to hand a resource back to a caller from inside a `with` block,
and no way to store one. That is the intended shape: a resource's lifetime is
exactly one lexical scope.

---

## 9. Asynchronous control flow

### 9.1 `async fn` and `await`

`async fn` marks a function asynchronous. `await` is a contextual keyword and is
an explicit suspension boundary.

| Rule | Code | Source |
| --- | --- | --- |
| `await` outside an async context | `RS0029` | `await_placement.rs::collect_expression` |
| `await` whose operand is not a resolved async call (or an outstanding `async let` handle) | `RS0030` | `await_operand_diagnostic` |
| an async call evaluated without `await` | `RS0022` | `async_call_consumption_diagnostic` |

The "async context" is `function_is_async` **or** a block that contains an
`async let` — a `task_group` body. That is why a *synchronous* function may
contain a `task_group` whose body awaits.

RSScript exposes no `Future` or `Task` value in source. `await` consumes a
direct async call or a task-group handle; there is nothing else to await.
`spawn` is reserved but not executable (`RS0015`).

**Accepted**

```rsscript
struct NetworkError {
    message: String
}

async fn fetch(id: Int) -> Result<String, NetworkError> {
    return Ok("user")
}

async fn load(id: Int) -> Result<String, NetworkError> {
    let value = await fetch(id: id)?
    return Ok(value)
}
```

**Rejected — `RS0029`**

```rsscript
struct NetworkError {
    message: String
}

async fn fetch(id: Int) -> Result<String, NetworkError> {
    return Ok("user")
}

fn load(id: Int) -> Result<String, NetworkError> {
    let value = await fetch(id: id)?
    return Ok(value)
}
```

**Rejected — `RS0030`**

```rsscript
fn helper() -> Int {
    return 1
}

async fn run() -> Int {
    let value = await helper()
    return value
}
```

**Rejected — `RS0022`**

```rsscript
struct NetworkError {
    message: String
}

async fn fetch(id: Int) -> Result<String, NetworkError> {
    return Ok("user")
}

async fn load(id: Int) -> Result<String, NetworkError> {
    let value = fetch(id: id)
    return Ok("x")
}
```

### 9.2 Where an `await` may appear (`RS0411`)

A user `async fn` lowers to an explicit `Pending` chain. The supported shapes
(`await_placement.rs::async_function_lowering_diagnostics`) are:

* a statement-boundary `await`: `let x = await f()`, `return await f()`, a bare
  `await f()` statement, an assignment RHS;
* an `await` inside an `if`, `loop`, `match`, or `with` **body**, which creates
  its own explicit async boundary;
* a `select` arm operation;
* an `await for` body;
* a `task_group` body.

Anything else — an `await` embedded in an ordinary expression — would need full
async expression lowering. Most such cases are rescued by the hoisting desugar
(§9.7); what remains rejected is exactly what hoisting cannot preserve.

**Rejected — `RS0411`** — the right operand of a short-circuit `||`

```rsscript
struct TimerError {
    message: String
}

async fn ready() -> Result<Bool, TimerError> {
    return Ok(true)
}

async fn bad() -> Result<Bool, TimerError> {
    let value = false || await ready()?
    return Ok(value)
}
```

### 9.3 `task_group` and `async let`

```
task_group {
    async let handle = async_call(args)
    …
    let value = await handle?
}
```

`crates/rsscript-semantics/src/task_groups.rs` enforces the structured-concurrency
contract. Every violation is `RS0015` with a specific label:

| Rule | Label |
| --- | --- |
| `async let` must be a **direct child** of `task_group { … }` | `nested async let` |
| a named `async let` handle must be consumed by `await` inside the same group | `unawaited async let` |
| that `await` must be a direct task-group statement, not nested in a sub-block | `nested async let await` |
| the `await` must come **after** the matching `async let` | `async let await before declaration` |
| a handle may be awaited **at most once** | `async let awaited more than once` |

All five were exercised while writing this section; the example below shows the
most common one.

Nested `task_group` and `select` bodies are independent structured boundaries
and are deliberately not traversed by this rule.

`async let _ = call(...)` creates a **scoped background child**: it has no name,
so no `await` obligation, and the group drains it before the scope can return.
No child silently outlives its parent.

**Accepted**

```rsscript
struct NetworkError {
    message: String
}

async fn fetch_user(id: Int) -> Result<String, NetworkError> {
    return Ok("user")
}

async fn fetch_profile(id: Int) -> Result<String, NetworkError> {
    return Ok("profile")
}

fn load(id: Int) -> Result<String, NetworkError> {
    task_group {
        async let user = fetch_user(id: id)
        async let profile = fetch_profile(id: id)
        let u = await user?
        let p = await profile?
    }
    return Ok("done")
}
```

**Accepted** — background child

```rsscript
struct NetworkError {
    message: String
}

async fn work(id: Int) -> Result<Int, NetworkError> {
    return Ok(id)
}

fn run() -> Result<Int, NetworkError> {
    task_group {
        async let _ = work(id: 1)
    }
    return Ok(0)
}
```

**Rejected — `RS0015`** (unawaited handle)

```rsscript
struct NetworkError {
    message: String
}

async fn work(id: Int) -> Result<Int, NetworkError> {
    return Ok(id)
}

fn run() -> Result<Int, NetworkError> {
    task_group {
        async let handle = work(id: 1)
    }
    return Ok(0)
}
```

### 9.4 `select`

```
select {
    binding = await <operation> => { body }
    binding = await <operation> => { body }
}
```

`select` waits on several async operations and runs the body of whichever
becomes ready first. `_` may be used as the binding to ignore the result.

Each arm's *operation* is checked in an async context regardless of the enclosing
function, while each arm's *body* is ordinary code in the enclosing context
(`await_placement.rs::collect_statement`). A `select` body is an independent
structured boundary for the `async let` rules.

Scheduler behaviour (`crates/rsscript-vm/src/reg_vm/scheduler.rs`): once a winner
is chosen, every losing arm's task is **cancelled** and reaped
(`cancel_select_losers`). Cancellation drains a cancelled task's lexical resource
scopes, including when the task is parked.

**Accepted**

```rsscript
fn pipeline() -> Result<Int, ChannelError> {
    let mut left = Channel.bounded<Int>(capacity: 1)?
    let mut right = Channel.bounded<Int>(capacity: 1)?
    let left_rx = Channel.receiver<Int>(channel: mut left)?
    let right_rx = Channel.receiver<Int>(channel: mut right)?
    task_group {
        select {
            first = await Receiver.recv<Int>(receiver: left_rx) => {
                Output.write(message: "left")
            }
            second = await Receiver.recv<Int>(receiver: right_rx) => {
                Output.write(message: "right")
            }
        }
    }
    return Ok(0)
}
```

### 9.5 Channels and streams

`packages/async/interface/channel.rssi` declares a bounded MPSC channel with
explicit endpoints, explicit waiting, and explicit ownership transfer:

```
struct ChannelError
struct Channel<T>
struct Sender<T>
struct Receiver<T>

pub fn Channel.bounded<T>(capacity: Int)          -> Result<fresh Channel<T>, ChannelError>
pub fn Channel.message<T>(capacity: Int)          -> Result<fresh Channel<T>, ChannelError>
pub fn Channel.sender<T>(channel: Channel<T>)     -> fresh Sender<T>
pub fn Channel.receiver<T>(channel: mut Channel<T>) -> Result<fresh Receiver<T>, ChannelError>
pub async fn Sender.send<T>(sender: Sender<T>, value: take T) -> Result<Unit, ChannelError>
pub async fn Sender.send_cancellable<T>(sender: Sender<T>, value: take T, token: CancellationToken) -> Result<Unit, ChannelError>
pub fn Sender.close<T>(sender: mut Sender<T>)     -> Unit
pub async fn Receiver.recv<T>(receiver: Receiver<T>) -> Result<Option<T>, ChannelError>
pub async fn Receiver.recv_cancellable<T>(receiver: Receiver<T>, token: CancellationToken) -> Result<Option<T>, ChannelError>
pub fn Receiver.close<T>(receiver: mut Receiver<T>) -> Unit
pub fn ChannelError.message(error: ChannelError)  -> fresh String
```

Values **move into** the channel: `Sender.send` takes `value: take T`, so the
sent value must be a local exclusive value (`RS0308` otherwise). `Receiver.recv`
returns `Result<Option<T>, ChannelError>`, where `None` means the channel is
closed.

`Channel.message<T>` is the same runtime channel with a compile-time payload
contract: `T` must be **cross-isolate transferable**, i.e. a Copy scalar,
`String`, or `Bytes` (`value_properties.rs::is_cross_isolate_transferable`).
Anything else is `RS0036` (`checks/calls.rs::check_message_channel_payload`). A
still-generic element type is skipped rather than rejected. Use
`Channel.bounded` for an in-isolate channel with arbitrary payloads.

`packages/async/interface/stream.rssi` declares a pull-based async sequence:

```
struct Stream<T>

pub fn Receiver.into_stream<T>(receiver: take Receiver<T>) -> fresh Stream<T>
pub fn Stream.from_list<T>(items: take List<T>)            -> fresh Stream<T>
pub async fn Stream.next<T>(stream: Stream<T>)             -> Result<Option<T>, ChannelError>
pub fn Stream.collect_list<T>(stream: Stream<T>)           -> Result<fresh List<T>, ChannelError>
```

`await for x in stream { … }` lowers to repeated `Stream.next` awaits and ends
when `next` yields `None`.

**Accepted** — a channel round trip

```rsscript
fn pipeline() -> Result<Int, ChannelError> {
    let mut channel = Channel.bounded<Int>(capacity: 4)?
    let sender = Channel.sender<Int>(channel: channel)
    let receiver = Channel.receiver<Int>(channel: mut channel)?
    task_group {
        local value = 1
        async let produced = Sender.send<Int>(sender: sender, value: take value)
        let sent = await produced?
        async let received = Receiver.recv<Int>(receiver: receiver)
        let got = await received?
    }
    return Ok(0)
}
```

**Accepted** — `await for` over a stream

```rsscript
fn consume(items: take List<Int>) -> Result<Int, ChannelError> {
    let stream = Stream.from_list<Int>(items: take items)
    task_group {
        await for value in stream {
            Output.write(message: Int.to_string(value: value))
        }
    }
    return Ok(0)
}
```

**Rejected — `RS0036`**

```rsscript
fn pipeline() -> Result<Int, ChannelError> {
    let mut channel = Channel.message<List<Int>>(capacity: 4)?
    return Ok(0)
}
```

### 9.6 What may cross an `await`

An async frame may suspend with managed handles and Copy snapshots live. It may
**not** suspend with a local exclusive value, a resource, or a runtime guard
live — `RS0031` (`await_placement.rs::await_live_value_diagnostics`). A local
*may* be moved with `take` into the awaited operation when the signature permits
it; what is forbidden is keeping it live *across* the boundary.

A `retains(param)` parameter is an escape even when the call is async, so a
local or resource cannot be smuggled past a suspension boundary that way.

**Rejected — `RS0031`**

```rsscript
struct Bag {
    items: List<Int>
}

struct NetworkError {
    message: String
}

fn make() -> fresh Bag {
    return Bag(items: [])
}

fn use_bag(bag: read Bag) -> Unit {
    return Unit
}

async fn fetch() -> Result<Int, NetworkError> {
    return Ok(1)
}

async fn run() -> Result<Int, NetworkError> {
    local bag = make()
    let value = await fetch()?
    use_bag(bag: bag)
    return Ok(value)
}
```

### 9.7 The await-hoisting desugar

`crates/rsscript-syntax/src/async_await_hoist.rs` runs on every program, before
module isolation and HIR construction. For each `async fn` body it rewrites
nested `await` expressions into A-normal form:

```
f(x: await g())
// becomes
let __rss_await_0 = await g()
f(x: __rss_await_0)
```

Details that matter:

* The unit lifted whole is `await op` **or** `await op?` (a `Try` wrapping an
  `Await`), so the `?` travels with the await.
* Hoisting is inner-to-outer: awaits inside the awaited operand are hoisted
  first.
* A *root* position (`let`/`return`/expression statement/assignment RHS) already
  holds a linear await, so only its **operand** is searched for nested awaits.
* Control-flow statements own their own blocks and are recursed into; the
  condition/scrutinee is left to the async boundary lowering.
* The temporaries are named `__rss_await_N` — which is exactly why user
  declarations may not use the `__rss_` prefix (§1.7).

Three positions are deliberately **not** hoisted, because hoisting would change
evaluation order or semantics, and therefore stay rejected as `RS0411`:

* the right operand of a short-circuit `&&` / `||`;
* `match` / `if` arms reached through an *expression* (arm **bodies** are
  hoisted; the scrutinee is not);
* closure bodies.

**Accepted** — hoisting rescues a nested await in an argument position

```rsscript
struct NetworkError {
    message: String
}

async fn fetch(id: Int) -> Result<Int, NetworkError> {
    return Ok(id)
}

fn double(value: Int) -> Int {
    return value * 2
}

async fn run(id: Int) -> Result<Int, NetworkError> {
    let value = double(value: await fetch(id: id)?)
    return Ok(value)
}
```

### 9.8 Cancellation

`packages/async/interface/cancellation.rssi` splits cancellation into a cancel
capability and a read-only observation token:

```
struct CancellationSource
struct CancellationToken

pub fn CancellationSource.new()                      -> fresh CancellationSource
pub fn CancellationSource.token(source: CancellationSource) -> fresh CancellationToken
pub fn CancellationSource.cancel(source: mut CancellationSource) -> Unit
pub fn CancellationToken.is_cancelled(token: CancellationToken)  -> Bool
```

`Task.cancellation_token()` (`packages/async/interface/task.rssi`) returns the
token of the **lexically enclosing** `task_group` scope. Because an `async fn` is
lowered at its definition site with no enclosing group, calling it inside an
`async fn` would silently produce a never-cancelled token, so it is rejected —
`RS0412`. The prescribed shape is to call it *inside* the `task_group` block and
pass the resulting `CancellationToken` into the async function as a `read`
parameter.

A nested `task_group` owns an independent token and is deliberately excluded
from the enclosing group's traversal.

**Accepted**

```rsscript
struct NetworkError {
    message: String
}

async fn work(token: read CancellationToken) -> Result<Int, NetworkError> {
    if CancellationToken.is_cancelled(token: token) {
        return Ok(0)
    }
    return Ok(1)
}

fn run() -> Result<Int, NetworkError> {
    task_group {
        let token = Task.cancellation_token()
        async let handle = work(token: token)
        let value = await handle?
    }
    return Ok(0)
}
```

**Rejected — `RS0412`**

```rsscript
async fn worker() -> Unit {
    let token = Task.cancellation_token()
    return Unit
}
```

### 9.9 What the scheduler guarantees, and what it does not

Guaranteed (`docs/spec/RSScript_Execution_Spec_v0.1.md`,
`crates/rsscript-vm/src/reg_vm/scheduler.rs`, `crates/rsscript-operation`):

* **Structured lifetime.** A task group drains every child — named or `_` —
  before its scope can return. No child outlives its parent.
* **Select cancels losers.** Once a `select` arm wins, the other arms' tasks are
  cancelled and reaped, and their lexical resource scopes are drained even if
  the task was parked.
* **Cancellation is cooperative and observable.** `CancellationToken` is a
  lock-free atomic flag; the first `cancel()` records the instant, and later
  cancels do not overwrite it. A task cannot cancel itself while running, and
  cancelling an unknown or already-reaped task is an error. Cancellation is not
  treated as successful completion.
* **Unified cleanup.** Normal return, `?` propagation, provider error, deadline,
  and cancellation converge on one resource-slot cleanup path.
* **No partial transfers.** A cancelled channel send or receive does not publish
  a partial transfer; channel closure remains observable through the ordinary
  `Result`/`Option` contract.
* **Deadlines are monotonic.** `MonotonicDeadline` is an `Instant`, not a wall
  clock.

Explicitly **not** guaranteed, and *unspecified* at the language level:

* **Scheduling order.** Nothing specifies which ready child runs first, whether
  scheduling is fair, or in what order sibling `async let` children start.
* **`select` tie-breaking.** When two arms are ready simultaneously, which one
  wins is unspecified.
* **Parallelism.** Nothing in the language says whether children run on separate
  OS threads or are interleaved on one. Purity and parallelism are inferred from
  validated source or supplied by provider metadata.
* **Cancellation latency.** Cancellation is cooperative; how long a child takes
  to observe it depends on the provider descriptor (cooperative, abort-safe, or
  not cancellation-aware) and is not a language guarantee.
* **Wall-clock timers, sockets, async files, subprocesses.** These are host
  services and must arrive through explicit packages/providers (§10).
---

## 10. Core interfaces and the host boundary

### 10.1 The rule

> Host services are never implicit.

The default single-file environment exposes only platform-neutral, deterministic
interfaces. It does **not** implicitly expose files, directories, environment
variables, HTTP, sockets, processes, temporary directories, wall clocks, system
randomness, logging, command-line arguments, or OS handles. Anything in that
list must arrive through an explicit package/provider, and the language front end
never diagnoses host *authorization* — that is deployment policy, outside the
language (`docs/spec/RSScript_v0.7_Spec.md` §1, §8).

The enforcement is simply that the names are not in scope: a call to an
undeclared host API is `RS0206`.

**Rejected — `RS0206`**

```rsscript
fn main() -> Unit {
    let contents = File.read_to_string(path: "a.txt")
    return Unit
}
```

**Accepted** — the deterministic core is available with no import

```rsscript
fn main() -> Unit {
    let quoted = Json.quote_string(value: "x")
    let joined = String.concat(left: "a", right: "b")
    let magnitude = Math.abs(value: 0 - 3)
    Output.write(message: joined)
    Output.write(message: quoted)
    Output.write(message: Int.to_string(value: magnitude))
    return Unit
}
```

Note that `Output.write` exists but `Output.print` does not; the interface is
`write` / `write_json` / `error` / `error_json` / `trace`
(`stdlib/output/output.rssi`).

### 10.2 What is actually in scope for a single file

`crates/rsscript-interface-catalog/src/lib.rs` defines **two** lists, and
`default_interfaces()` is their concatenation. A single-file
`rss check` / `rss build` sees both.

**`CORE_INTERFACES` — 35 platform-neutral core interface files** (this is the
list published as `docs/generated/core-interfaces.md`):

```
stdlib/arguments/arguments.rssi     stdlib/clone/clone.rssi
stdlib/cmp/eq.rssi                  stdlib/cmp/ord.rssi
stdlib/collections/buffer.rssi      stdlib/collections/bytes.rssi
stdlib/collections/deque.rssi       stdlib/collections/list.rssi
stdlib/collections/map.rssi         stdlib/collections/persistent_map.rssi
stdlib/collections/pipeline.rssi    stdlib/collections/set.rssi
stdlib/collections/sorted_map.rssi  stdlib/collections/sorted_set.rssi
stdlib/csv/csv.rssi                 stdlib/date/date.rssi
stdlib/diff/diff.rssi               stdlib/duration/duration.rssi
stdlib/dyn/dyn.rssi                 stdlib/encoding/encoding.rssi
stdlib/hash/hash.rssi               stdlib/hash/hashable.rssi
stdlib/json/json.rssi               stdlib/math/math.rssi
stdlib/option/option.rssi           stdlib/output/output.rssi
stdlib/patch/patch.rssi             stdlib/path/path.rssi
stdlib/regex/regex.rssi             stdlib/result/result.rssi
stdlib/string/string.rssi           stdlib/test/assert.rssi
stdlib/url/url.rssi                 stdlib/weak/weak.rssi
stdlib/yaml/yaml.rssi
```

**`STANDARD_PACKAGE_INTERFACES` — 4 async package contracts** that are
prelude-visible **only** for single-file checks and lowering:

```
packages/async/interface/cancellation.rssi
packages/async/interface/channel.rssi
packages/async/interface/stream.rssi
packages/async/interface/task.rssi
```

These four supply `Channel`, `Sender`, `Receiver`, `ChannelError`, `Stream`,
`CancellationSource`, `CancellationToken`, and `Task.cancellation_token()`. They
are why the channel and cancellation examples in §9 check clean as single files.

Package review and package lowering must receive these through explicit package
dependencies instead — the prelude visibility is a single-file convenience, not
a language guarantee. `docs/generated/core-interfaces.md` documents only the
first list, so the four async files are prelude-visible but undocumented there
(§12).

`rss check` accepts `--no-core` to drop the core prelude and `--interface
<file.rssi>` to add contracts explicitly.

### 10.3 Purity constraints on the core

Where a service could be either pure or ambient, the core interface takes the
pure half and leaves the ambient half to a host package
(`docs/spec/RSScript_v0.7_Spec.md` §8):

| Available in core | Belongs to a host package |
| --- | --- |
| lexical path manipulation with specified cross-platform semantics | ambient path queries, any filesystem I/O |
| `Duration` as a pure value | reading a clock |
| a deterministic PRNG with an explicit seed | system entropy |
| `Arguments.*` over an explicitly supplied `List<String>` | ambient `argv` |
| `Output.write` to the bounded program output channel | logging, stdio handles |

---

## 11. Diagnostics index

Every code the front end can emit, mapped to the section of this document that
explains its rule. Sources: `crates/rsscript-diagnostics/src/implementation.rs`
(the registry) and `docs/generated/diagnostic-catalog.md` (the published
explanations).

### 11.1 Declarations and signatures

| Code | Title | Section |
| --- | --- | --- |
| `RS0001` | reserved diagnostic — never emitted | — |
| `RS0002` | missing return type | §3.1 |
| `RS0003` | missing parameter type | §3.1 |
| `RS0005` | duplicate declaration | §1.9 |
| `RS0007` | invalid retained parameter | §5.7 |
| `RS0015` | unsupported syntax | §1.3, §1.4, §1.7, §1.8, §1.10, §2.3, §2.6, §2.12, §7.1, §9.3 |
| `RS0018` | unresolved import | §1.4 |
| `RS0028` | invalid `self` parameter | §4.3, §7.1 |
| `RS0035` | lowered name conflict / invalid pin | §4.1 |
| `RS0040` | semantic analysis incomplete (work budget exhausted) | §3.4 |
| `RS1301` | package `.rssi` contract mismatch | §1.3 |
| `RSL001` | signature complexity lint | §4.11 |
| `RSL002` | duplicate effect lint | §4.11, §5.7 |

### 11.2 Types

| Code | Title | Section |
| --- | --- | --- |
| `RS0023` | `Fd` outside native/resource internals | §2.11 |
| `RS0024` | unknown type | §2.1, §2.7 |
| `RS0025` | unknown field | §2.14 |
| `RS0026` | unknown binding | §2.14 |
| `RS0027` | unknown protocol | §2.7, §7.2 |
| `RS0033` | integer literal out of range | §1.2 |
| `RS0038` | char literal is not a single scalar | §1.2 |
| `RS0039` | cyclic type alias | §2.8 |
| `RS0210` | operator type mismatch | §2.13 |
| `RS0211` | derive requirement not satisfied | §2.12 |
| `RS0212` | unsupported resource derive | §2.12, §8.1 |
| `RS1001` | operator overload attempt | §2.13 |
| `RS1002` | implicit conversion attempt | §1.10 |
| `RS1003` | `own struct` attempt | §1.10, §2.3 |
| `RS1004` | surface reference (`&T`) attempt | §1.10 |

### 11.3 Inference and calls

| Code | Title | Section |
| --- | --- | --- |
| `RS0034` | binding type cannot be inferred | §3.3 |
| `RS0201` | unnamed argument | §4.4 |
| `RS0202` | call-site data-effect mismatch | §4.6; structured-pattern effects §6.5 |
| `RS0203` | unknown named argument | §4.4 |
| `RS0204` | missing required argument | §4.4 |
| `RS0205` | duplicate named argument | §4.4 |
| `RS0206` | unknown callee | §4.2, §10.1 |
| `RS0207` | argument / initializer / callback-shape type mismatch | §3.3, §4.10, §5.10 |
| `RS0208` | return type mismatch (including fall-through) | §4.7 |
| `RS0032` | protocol bound not satisfied | §3.5, §7.3, §7.4 |

### 11.4 Control flow

| Code | Title | Section |
| --- | --- | --- |
| `RS0013` | invalid try operator | §6.10 |
| `RS0016` | `break`/`continue` outside a loop | §6.3 |
| `RS0017` | binding read before it is assigned | §6.12 |
| `RS0021` | non-exhaustive match | §6.7 |
| `RS0037` | variant pattern arity mismatch | §6.4 |
| `RS0209` | control-flow type mismatch (condition, iterable, scrutinee, literal pattern, variant family, match-arm type) | §6.2, §6.3, §6.6, §6.8 |
| `RS0311` | invalid assignment | §6.11 |
| `RS0312` | assignment target deferred | §6.11 |
| `RS0313` | assignment type mismatch | §6.11 |

### 11.5 Ownership, places, and closures

| Code | Title | Section |
| --- | --- | --- |
| `RS0301` | managed-to-local conversion | §5.1 |
| `RS0302` | whole-base / field partial access conflict | §5.4 |
| `RS0303` | nested field prefix conflict | §5.4 |
| `RS0304` | indexed container partial access conflict | §5.4 |
| `RS0305` | move-base field access conflict | §5.4 |
| `RS0306` | local class binding | §2.3, §5.1 |
| `RS0307` | invalid `manage` operand | §5.1 |
| `RS0308` | invalid `take` operand | §5.1 |
| `RS0309` | managed field split conflict | §5.4 |
| `RS0310` | read-view mutation (`for` element) | §5.6 |
| `RS0401` | use after manage / move | §5.3 |
| `RS0501` | local value retained | §5.7 |
| `RS0601` | fresh return is not clean | §5.8 |
| `RS0602` | freshness unknown (warning) | §5.8 |
| `RS0603` | invalid fresh return type | §5.8, §3.4 |
| `RS0604` | fresh value requires a local binding | §5.8 |
| `RS0801` | local captured by managed closure | §5.9 |
| `RS0802` | noescape callback escape | §5.9, §5.7 |
| `RS0803` | local closure escape | §5.9 |
| `RS0804` | noescape closure consumes captured local | §5.9 |
| `RS0805` | explicit-closure capture contract | §5.9 |
| `RS0901` | `take` of a handle field | §5.5 |
| `RS0902` | invalid weak field | §5.5 |
| `RS0903` | weak field requires upgrade | §5.5 |
| `RS0904` | weak field requires a weak handle | §5.5 |

### 11.6 Resources

| Code | Title | Section |
| --- | --- | --- |
| `RS0701` | resource stored in an ordinary field | §8.1, §8.4 |
| `RS0702` | resource escape / producer outside `with` | §8.2, §8.4 |
| `RS0704` | resource in an ordinary generic type | §8.1, §8.4 |
| `RS0706` | `Result` resource producer missing `?` | §8.3 |

### 11.7 Async

| Code | Title | Section |
| --- | --- | --- |
| `RS0022` | async call not consumed by `await` | §9.1 |
| `RS0029` | `await` outside an async context | §9.1 |
| `RS0030` | `await` of a non-async expression | §9.1 |
| `RS0031` | local / resource live across `await` | §8.5, §9.6 |
| `RS0036` | message-channel payload not transferable | §9.5 |
| `RS0411` | async function not lowerable in this version | §9.2, §9.7 |
| `RS0412` | cancellation token outside a `task_group` | §9.8 |

### 11.8 Outside the language front end

These codes exist in the registry but are produced by the backend, the runtime,
or the package manager, not by the language rules described here.

| Code | Origin |
| --- | --- |
| `RS1101` | a rustc diagnostic mapped back through source-map metadata |
| `RS1102` | a rustc diagnostic whose generated-Rust location could not be mapped |
| `RS1201` | a runtime managed-aliasing or resource conflict with a source span |
| `PKG0101` `PKG0102` `PKG0501` `PKG0601` `PKG0901` | package manager: feature resolution, dependency sources, review policy, native binding metadata, provider declarations |
| `RSR001`–`RSR020` | package review / API-diff codes (features, functions, params, returns, retention, types, boundaries, protocol impls, sums, consts, aliases) |

### 11.9 Codes in the registry but not in the published catalog

`docs/generated/diagnostic-catalog.md` publishes 83 entries. The registry
(`crates/rsscript-diagnostics/src/implementation.rs`) defines these additional
codes, all of which the front end really does emit (each is exercised by a
fixture or by an example in this document):

| Code | Meaning | Section |
| --- | --- | --- |
| `RS0036` | message payload not cross-isolate transferable | §9.5 |
| `RS0038` | char literal is not exactly one Unicode scalar | §1.2 |
| `RS0039` | cyclic type alias | §2.8 |
| `RS0309` | managed field split conflict | §5.4 |
| `RS0805` | explicit-closure capture contract | §5.9 |
| `RSR001`–`RSR020` | package review codes | §11.8 |

This is a documentation gap in the generated catalog, not a language gap (§12).

Note also that the code space has holes: `RS0004`, `RS0006`, `RS0008`–`RS0012`,
`RS0014`, `RS0019`, `RS0020`, `RS0703`, and `RS0705` are not defined.

---

## 12. Unspecified behaviour, gaps, and surprises

Collected here so a reader does not have to infer a rule that is not there. Each
item was confirmed by running the example shown, or by reading the implementation
and finding no enforcing code.

### 12.1 Genuinely unspecified

* **`main`'s signature.** The checker imposes no constraint (§4.9). Which return
  types the runner accepts, and how program arguments reach `main`, is a runner
  concern.
* **Scheduling order, fairness, and parallelism.** Nothing specifies which ready
  child runs first, whether sibling `async let` children start in declaration
  order, or whether children run in parallel (§9.9).
* **`select` tie-breaking** when two arms are ready at once (§9.9).
* **Cancellation latency.** Cooperative; depends on the provider descriptor
  (§9.9).
* **Cross-module privacy.** `pub` gates positional arguments and package
  contracts, but module isolation rewrites a cross-module reference regardless
  of `pub`, and no diagnostic rejects using a non-`pub` declaration from another
  module (§1.6).
* **Evaluation order of call arguments.** Not stated anywhere in the front end.
* **Integer overflow behaviour** for `+`/`-`/`*` on `Int`. `Math.wrapping_add`
  and friends exist, which implies the plain operators are *not* wrapping, but
  the front end does not say what they are.
* **`Float` semantics** beyond "it is not `Ord` and not `Eq`" (§3.5).

### 12.2 Gaps — rules the design implies but the checker does not enforce

These are findings for the maintainer, not features.

* **A used binding with an open generic position is trusted.** `RS0034` fires
  only when the binding is never used (§3.3). `let xs = []` followed by pushes
  of mixed element types, or a bare `let v = Ok(1)` that is later returned, keeps
  the `?`/placeholder position and the dependent checks keep skipping it. There
  is no constraint propagation from later uses back to the binding (§3.2).

### 12.3 Surprises worth calling out

* **A `.rss` function cannot construct a resource.** Returning a resource
  constructor is `RS0702`. Every resource must be produced by a bodyless
  function in an `.rssi` interface (§8.2). Several "pass" fixtures under
  `crates/rsscript-sdk/tests/fixtures/pass/` therefore do *not* pass a plain
  `rss check` — they rely on bodyless `.rss` declarations, which also emit
  `RS0015`. Treat those fixtures as shape examples, not as checkable programs.
* **`String` is not Copy** (§2.2), so `retains(s: String)` is legal while
  `retains(n: Int)` is `RS0007`.
* **`Float` is `Clone` but not `Eq` and not `Ord`; `Bool` is `Ord` but `Char`
  and `Byte` are not** (§3.5). The three builtin predicates in
  `generic_constraints.rs` have different membership and are easy to misread as
  one set.
* **Structured patterns need a scrutinee effect, variant patterns do not.**
  `match value { Some(n) => … }` is fine; `match point { Point { x } => … }` is
  `RS0202` (§6.5).
* **`loop { }` does not count as diverging** for the return check, so an
  intentional infinite loop at the end of a non-`Unit` function produces
  `RS0208` (§4.7).
* **`Type.method` dispatch is by inferred receiver type**, not by method name,
  so a receiver whose type is unknown makes `x.m()` unresolvable — `RS0206`
  (§4.2).
* **Tuple arity is capped at 26** because the checker recognises a generic type
  variable only as a single uppercase letter (§2.9).
* **The exhaustiveness witness product is capped at 512 rows.** A struct or sum
  with many finite-domain fields silently becomes "not provably exhaustive" and
  needs an explicit `_` (§6.7).

### 12.4 Documentation drift found while writing this reference

* `docs/generated/diagnostic-catalog.md` is missing `RS0036`, `RS0038`,
  `RS0039`, `RS0309`, `RS0805`, and the whole `RSR0xx` family, all of which are
  defined in `crates/rsscript-diagnostics/src/implementation.rs` (§11.9).
* `docs/generated/core-interfaces.md` documents only `CORE_INTERFACES` (35
  files). The four `STANDARD_PACKAGE_INTERFACES` under
  `packages/async/interface/` are equally prelude-visible to a single-file check
  and are not listed there (§10.2).
* `docs/spec/RSScript_Execution_Spec_v0.1.md` names conformance anchors at
  `tests/checker_frontend/async_resources.rs` and
  `tests/vm_eval_parity/async_concurrency.rs`. Neither path exists in the
  repository.
* `docs/generated/grammar.md` lists reserved keyword classes from the lexer
  table only. Words the parser gives declaration meaning to — `sum`, `protocol`,
  `impl`, `type`, `const`, `opaque`, `derives`, `retains`, `noescape`, `owned`,
  `captures`, `task_group`, `select`, `spawn`, `use`, `module` — do not appear
  there (§1.1).

---

## Provenance

Primary sources for this document:

| Area | Files |
| --- | --- |
| lexing, parsing, AST | `crates/rsscript-syntax/src/lexer.rs`, `parser/`, `ast.rs` |
| desugars | `crates/rsscript-syntax/src/desugar.rs`, `function_value_desugar.rs`, `async_await_hoist.rs` |
| modules and names | `crates/rsscript-semantics/src/module_isolation.rs`, `symbols.rs`, `source_rules.rs` |
| types and inference | `crates/rsscript-semantics/src/types.rs`, `hir/infer.rs`, `type_compatibility.rs`, `value_properties.rs`, `external_types.rs`, `type_aliases.rs` |
| generics and protocols | `crates/rsscript-semantics/src/generic_constraints.rs`, `protocol_bounds.rs`, `checks/calls/generic_constraints.rs` |
| calls and effects | `crates/rsscript-semantics/src/call_arguments.rs`, `call_binding.rs`, `checks/calls.rs`, `operators.rs` |
| ownership and flow | `crates/rsscript-semantics/src/ownership.rs`, `place.rs`, `moved_use_flow.rs`, `local_flow_*.rs`, `fresh_return_flow.rs`, `take_handle_fields.rs`, `weak_fields.rs`, `assignment.rs` |
| closures | `crates/rsscript-semantics/src/checks/body/closure_captures.rs`, `checks/calls/closure_contracts.rs`, `closure_escape.rs`, `callbacks.rs`, `retained_closure_flow.rs` |
| control flow | `crates/rsscript-semantics/src/control_flow.rs`, `analyzer/exhaustiveness.rs`, `try_checks.rs` |
| resources | `crates/rsscript-semantics/src/resource_types.rs`, `resource_flow.rs`, `resource_producers.rs` |
| async | `crates/rsscript-semantics/src/await_placement.rs`, `task_groups.rs`, `crates/rsscript-vm/src/reg_vm/scheduler.rs`, `crates/rsscript-operation/src/lib.rs` |
| derives | `crates/rsscript-semantics/src/derives.rs`, `derive_fields.rs` |
| interfaces | `crates/rsscript-interface-catalog/src/lib.rs`, `stdlib/**/*.rssi`, `packages/async/interface/*.rssi` |
| diagnostics | `crates/rsscript-diagnostics/src/implementation.rs`, `docs/generated/diagnostic-catalog.{md,json}` |
| fixtures | `crates/rsscript-sdk/tests/fixtures/{pass,fail}/`, `crates/rsscript-sdk/tests/corpus/`, `examples/scripts/`, `evals/fixtures/` |

See also: [`RSScript_v0.7_Spec.md`](RSScript_v0.7_Spec.md) (normative),
[`RSScript_Execution_Spec_v0.1.md`](RSScript_Execution_Spec_v0.1.md),
[`../generated/diagnostic-catalog.md`](../generated/diagnostic-catalog.md),
[`../generated/core-interfaces.md`](../generated/core-interfaces.md),
[`../generated/grammar.md`](../generated/grammar.md).
