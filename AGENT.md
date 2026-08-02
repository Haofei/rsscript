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

```rsscript
fn transform(input: take Image, options: read Options) -> fresh Image {
    return Image.transform(input: take input, options: read options)
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

Before submitting repository changes, run:

```rust
fn main() -> Int {
    return 0
}
```

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Also verify the removed source syntax and compiler policy types do not reappear,
and that `crates/rsscript/src/interfaces.rs` contains no default host includes.
