# RSScript HTTP Server Example

`rss-core-http-server` is a demo-only native package for local examples and
tests. It serves fixed `text/plain` routes sequentially and intentionally does
not implement production HTTP concerns such as concurrent request handling,
backpressure, graceful shutdown, request size limits, authentication, or
per-request deadlines.

Do not expose this package to untrusted networks. Production deployments should
use a maintained HTTP server adapter with explicit resource, cancellation, and
security policies.
