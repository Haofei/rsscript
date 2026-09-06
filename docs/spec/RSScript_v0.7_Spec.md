# RSScript Language Specification

Status: breaking platform-neutral language revision.

## 1. Scope and invariants

RSScript defines syntax, types, ownership, lifetime and resource rules, and
structured asynchronous control flow. It does not define operating-system APIs,
host permissions, deployment authorization, risk policy, or sandbox behavior.

The following invariant is normative:

> Program validity, type checking, semantic lowering, and generated code are
> independent of host permissions, deployment grants, runner policy, provider
> selection, and operating-system services.

Parsing and compilation therefore take source and declared interfaces, never a
runner context or deployment profile. External implementation selection happens
after compilation.

## 2. Files and declarations

`.rss` files are implementation files. Every ordinary function declared in an
implementation file has a body. `.rssi` files are interfaces; their functions are
bodyless declarations. A bodyless interface function is an external symbol, not
a distinct function kind in the language AST.

```rsscript
module image.pipeline
use collections.list.*

pub fn resize(image: take Image, width: Int) -> fresh Image {
    return Image.resize(image: take image, width: width)
}
```

Top-level file feature and profile declarations are not grammar productions.
Implementation-origin markers are not declaration modifiers.

## 3. Types and protocol dispatch

The core scalar and structural types include `Unit`, `Bool`, `Int`, `Float`,
`String`, structs, classes, sum types, generics, resources, collections, and
function types.

Protocols define explicit method contracts. `Dyn<P>` is the dynamic protocol
dispatch type for protocol `P`; it does not represent permission or authority.

```rsscript
protocol Render {
    fn render(self: read Self) -> fresh String
}

fn render_dynamic(value: read Dyn<Render>) -> fresh String {
    return Render.render(self: value)
}
```

## 4. Data effects and ownership

Parameters have a closed data-effect set:

- `read`: the call may observe but not mutate or consume the argument;
- `mut`: the call may mutate and writes propagate to the caller;
- `take`: the call consumes the argument and later use is invalid.

These are type/ownership semantics, not host effects.

`local` creates an exclusive local value. `manage` moves a valid local graph into
managed storage. The checker rejects use after move, conflicting places,
managed-to-local leakage, and illegal escape. These constructs need no file-level
enablement.

`fresh T` states that a returned value does not alias caller-visible mutable
state. `noescape Fn(...)` prevents a callback from escaping its call. `owned`
records owned function/value forms where specified by the type system.

## 5. Structured retention

Retention is the only source declaration contract in this family because it
affects escape checking. It is represented directly on a function declaration.

```rsscript
fn Cache.put(cache: mut Cache, value: read Value) -> Unit
    retains(value)
{
    Cache.store(cache: mut cache, value)
}
```

Each retained name must identify a declared non-Copy parameter. The AST stores
retained parameter names, and HIR resolves them to parameter identity. Arbitrary
string declaration effects and source purity assertions do not exist. Purity and
parallelism are inferred from validated source or supplied by provider metadata.

## 6. Resources

`resource` values have linear lifetime rules. `with` introduces a bounded
resource scope and guarantees cleanup on every exit. Resources may not escape
their scope unless the type and ownership rules explicitly permit the transfer.
Handle and weak-reference rules continue to govern managed object graphs.

```rsscript
with Stream.open(config) as stream {
    Stream.consume(stream: mut stream)
}
```

## 7. Asynchronous control flow

`async fn`, `await`, `task_group`, `async let`, `select`, channels,
cancellation, and streams are language/runtime-core constructs. They require no
file header. Task lifetimes remain structured and cancellation propagates through
the owning scope.

Wall-clock timers, sockets, asynchronous files, and subprocesses are host
services and must arrive through explicit packages/providers.

## 8. Core interfaces

The default single-file environment exposes only platform-neutral deterministic
interfaces. It does not implicitly expose files, directories, environment
variables, HTTP, sockets, processes, temporary directories, wall clocks, system
randomness, logging, command-line arguments, or OS handles.

Pure path manipulation may be provided only with specified cross-platform lexical
semantics. Ambient path queries and I/O belong to a host filesystem package.
Durations may be pure values; reading a clock belongs to a host time package.
Deterministic PRNGs require an explicit seed; system entropy belongs to a host
random package.

The generated [core-interface catalog](../generated/core-interfaces.md) names
the exact current interface files. Its companion
[language card](../generated/language-card.md) is a non-normative, source-backed
quick reference for lexical keywords and diagnostics; this specification and
the parser remain authoritative.

## 9. External symbols and bindings

A bodyless `.rssi` function introduces an external symbol. Package binding
metadata maps the symbol to a provider implementation:

```toml
schema = "rsscript.bindings.v1"

[[function]]
symbol = "host.net.http.get"
provider = "rsscript_host_http"
entry = "http_get"
review_effects = ["network.client"]
```

The optional `review_effects` field belongs to binding metadata and is not parsed
as RSScript. Linking diagnoses missing providers, duplicate bindings, and ABI
mismatches. The frontend does not diagnose host authorization.

VM lowering uses an external-call instruction containing a stable symbol/binding
identity. At execution, an external-function registry resolves that identity.
Execution control contains cancellation, deadline, budgets, output bounds, and
trace context only.

## 10. Packages and analysis artifacts

Package build-selection features may exist in `rsspkg.toml`; they are unrelated
to language syntax or host permission. The platform-neutral analysis artifact
uses schema `rsscript.package_analysis.v1` and contains diagnostics, exports,
semantic summaries, retention facts, resource facts, async facts, and external
symbols. It contains no host grants.

Binding/provider review is optional and consumes the validated call graph plus
binding metadata. REIR and deployment policy must not influence validation or
lowering.

## 11. Runtime limits

Execution limits include step, memory, host-call, output, recursion, cancellation,
and deadline controls. Limits protect availability and embedding stability. They
are not proof of isolation and must not be described as language authority or a
sandbox.

## 12. Removed surface

This revision intentionally provides no compatibility aliases or legacy artifact
reader for the removed language surface. Old sources must be migrated to ordinary
declarations, structured `retains(param)`, `Dyn<P>`, and explicit host packages.
