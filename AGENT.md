# RSScript contributor and generation guide

## Language boundary

Generate platform-neutral RSScript. The language models ownership, retention,
resource lifetime, and structured asynchronous control flow. It does not model
host permission, deployment policy, OS authority, or provider implementation
technology.

Never generate a file feature/profile header, a generic declaration-effect list,
an implementation-origin modifier, or a source-level unsafe marker. Use `Dyn<P>`
for dynamic protocol dispatch.

## Functions

Implementation functions in `.rss` have bodies. Interface functions in `.rssi`
are ordinary bodyless declarations. Parameters use the closed `read`, `mut`, and
`take` data-effect set.

An `.rssi` file is *supplied* to the compiler alongside the implementation, not
copied into it. Never restate an interface declaration inside the `.rss` file
you are writing: its symbols are already in scope, and repeating one declares it
twice.

```rsscript
fn transform(input: take Image, options: read Options) -> fresh Image {
    return Image.transform(input: take input, options)
}
```

If a call retains a parameter after return, add one structured clause per retained
parameter:

```rsscript
fn Store.insert(store: mut Store, key: read String, value: read Value) -> Unit
    retains(key)
    retains(value)
{
    // ...
}
```

Do not use source assertions for purity, allocation, blocking, panic behavior,
parallelism, native implementation, or host service access. These facts are
inferred or supplied by external provider metadata.

## Accepted surface spellings

Two alternate spellings are accepted and desugared by the parser to the
canonical form. Both produce the same AST — the checker never sees the alternate
spelling — and `rss fmt` rewrites them to the canonical one, so formatting is
the normalizer. Prefer the canonical spelling when writing new code.

| Also accepted | Canonical (what `rss fmt` prints) |
| --- | --- |
| `T { field: value }` | `T(field: value)` |
| `Pattern => expr,` | `Pattern => { expr }` |

A trailing comma after a block arm (`Pattern => { ... },`) is accepted too.
Nothing else outside the generated [grammar surface](docs/generated/grammar.md)
is accepted: `::` paths, `task_group`/`with`/`select` in expression position,
tuple destructuring in `for`, `mut x: T = ...` as a binding form, and
`fn(x: T) -> U { ... }` as a closure literal are all errors.

## Closures, resource scopes, and protocols

These four forms are the ones most often written in a neighbouring language's
spelling. The canonical spelling of each, with its wrong twin, is a row in the
generated [language card](docs/generated/language-card.md), which also carries
a worked program showing them together.

- A closure literal is `|x| { return x * 2 }`. It captures implicitly, and the
  binding it goes into is `local`, not `let`. The explicit-capture spelling is
  the other one — `fn(x) captures(read base) { ... }` — and the two do not mix:
  `|x| captures(...)` is a syntax error.
- A resource scope binds with `as`: `with File.open_read(path)? as file { ... }`,
  never `with file = File.open_read(path) { ... }`. A resource producer is
  declared bodyless in an `.rssi`; a `.rss` body cannot mint one.
- An `impl P for T` block maps existing functions into the protocol's slots
  (`format = Point.format`); it does not declare method bodies. Dynamic
  dispatch is a call, not a constructor:
  `Dyn.from<Formatter, Point>(value: take point)`.
- `let Some(x) = value else { return ... }` binds or leaves the block, and the
  `else` block must diverge. `Option` has no `unwrap`: the fallible readers are
  `Option.unwrap_or`, `Option.unwrap_or_else` and `Option.ok_or`.

## Ownership and lifetime

- `read` observes an argument.
- `mut` permits mutation and propagates writes.
- `take` consumes ownership.
- `local` creates an exclusive local value.
- `manage` moves a valid local graph into managed storage.
- `fresh` promises a non-aliasing return.
- `noescape` prevents callback escape.
- `resource` and `with` define scoped cleanup.
- handle/weak rules govern managed references.

These constructs require no file-level enablement.

## Async

Use `async fn`, `await`, `task_group`, `async let`, `select`, channels,
cancellation, and abstract streams for structured concurrency. Do not generate
ambient timers, files, sockets, or subprocess access unless an explicit package
interface and binding are present in the task context.

## Host packages

Filesystem, environment, process, network, time, randomness, logging, CLI
arguments, and OS handles are not implicit core APIs. A host package declares
ordinary bodyless `.rssi` functions and supplies `rsscript.bindings.v1` metadata.
Do not invent or automatically insert a host dependency.

## Validation

Before submitting repository changes, run the local gate in
[docs/development/DEVELOPMENT.md](docs/development/DEVELOPMENT.md). The two
rules that gate matters most for generated code are:

- a program `rss check` accepts must build, verify, and run
  (`fixture_build_corpus` builds every pass fixture with a `main`);
- every diagnostic and generation change is measured with the eval corpus
  (`cargo run -p rsscript-xtask -- agent-eval …`) before and after.

`crates/rsscript-compiler/src/interfaces.rs` must contain no default host
includes; host services reach a program only through explicit `.rssi`
interfaces and bindings.

<!-- BEGIN GENERATED LANGUAGE CARD -->

## Generated language card

The generated [language card](docs/generated/language-card.md) and machine-readable [language card](docs/generated/language-card.json), [grammar](docs/generated/grammar.json), [diagnostic catalog](docs/generated/diagnostic-catalog.json), and [core-interface catalog](docs/generated/core-interfaces.json) are derived from syntax keyword tables, the diagnostic registry, and the core-interface catalog. Refresh them with `cargo run -p rsscript-xtask -- language-card`; verify freshness with the same command plus `--check`.

<!-- END GENERATED LANGUAGE CARD -->
