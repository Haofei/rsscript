# File upload benchmark

This benchmark compares request throughput for the same upload workload against
one Tokio mock upload server:

- `rss-file-upload-benchmark`: RSScript async client lowered to Rust, using the
  RSS runtime pending ABI and a Tokio native upload binding.
- `rust_async_upload_client`: hand-written Tokio client with bounded
  concurrency.
- `sync_upload_client`: blocking TCP client that uploads files sequentially.
- `mock_upload_server`: Tokio HTTP server that reads the full request body before responding.

Run it through the Rust test runner:

```sh
cargo test --test file_upload_benchmark_e2e -- --nocapture
```

The test runner starts the server, builds the native benchmark helpers, lowers and
builds the RSScript package, excludes build time from timing, and prints requests
per second for all clients. The default workload is:

```text
requests=24
payload_bytes=65536
concurrency=8
server_delay_ms=50
```

The RSS and Rust async paths both use Tokio for actual socket IO and hit the
same server with the same request count and payload size. The benchmark reports
the Rust/RSS async RPS ratio so regressions show whether the difference is in
RSScript lowering/runtime scheduling rather than the server or network setup.

Representative local output:

```text
file upload benchmark: rss_async_rps=149.81 rust_async_rps=148.85 sync_rps=15.39 rss_async_ms=160 rust_async_ms=161 sync_ms=1559 rss_async_max_in_flight=8 rust_async_max_in_flight=8 sync_max_in_flight=1 rust_to_rss_rps_ratio=0.994 likely_bottleneck=server_or_io
```

When `rust_to_rss_rps_ratio` stays near `1.0` and both async clients reach the
same `max_in_flight`, the bottleneck is the shared server/IO workload rather
than RSScript's async lowering. A ratio materially above `1.0` points at
RSScript runtime/lowering overhead.
