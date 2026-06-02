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
