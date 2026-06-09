# rss-async-runtime

`rss-async-runtime` is the built-in provider package for the `rss-async`
interface package.

It records the backend choice in the package graph:

```toml
[providers]
async = "rss-async-runtime"
```

The actual executable primitives still live in the compiler/runtime intrinsic
layer, so RSScript programs do not import backend `Future`, `Poll`, `Waker`,
Tokio, or platform event-loop types. The provider package exists so executable
packages make the backend selection reviewable instead of receiving async APIs
from an implicit core surface.
