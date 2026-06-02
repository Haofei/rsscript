# rss-async

`rss-async` is the standard async API package.

RSScript keeps async split into three layers:

- The language owns `async fn`, `await`, `task_group`, `select`, and `await for`.
- The compiler/runtime owns the hidden cooperative `Pending` executor substrate.
- This package owns user-facing async APIs: timers, deadlines, cancellation,
  channels, and streams.

This follows the same broad package shape as MoonBit async: async IO and
coordination APIs live in packages instead of becoming special syntax. RSScript
keeps its own review model by preserving explicit `async`, `native`, `read`,
`mut`, `take`, and effect boundaries in the `.rssi` contract.

Future async packages should extend this structure with socket, TLS,
filesystem, process, HTTP/WebSocket, async queue, semaphore, and IO
abstractions without exposing backend `Future`/`Poll` details to RSScript
programs.

Executable packages should depend on both the interface and a reviewed backend
provider:

```toml
[dependencies]
rss-async = { path = "../async" }
rss-async-runtime = { path = "../async-runtime" }

[providers]
async = "rss-async-runtime"
```

Single-file scripts keep a prelude-visible async surface for quick iteration,
but package review and lowering require the explicit dependency/provider graph.
