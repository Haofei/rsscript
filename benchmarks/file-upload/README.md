# File upload benchmark

This benchmark compares request throughput for the same upload workload against
one Tokio mock upload server:

- `rss-file-upload-benchmark`: RSScript async client lowered to Rust, using the
  RSS runtime pending ABI and a Tokio native upload binding.
- `rust_async_upload_client`: hand-written Tokio client using the same fixed
  concurrency batches as the generated RSScript client.
- `sync_upload_client`: blocking TCP client that uploads files sequentially.
- `mock_upload_server`: Tokio HTTP server that reads the full request body before responding.

Run it through the Rust test runner:

```sh
cargo test --test file_upload_benchmark_e2e -- --ignored --nocapture
```

The test runner starts the server, builds the native benchmark helpers, lowers and
builds the RSScript package, excludes build time from timing, and prints requests
per second for all clients. The default workload is:

```text
requests=256
payload_bytes=262144
concurrency=16
server_delay_ms=10
```

Override the workload with environment variables when you need a shorter smoke
run or a larger local benchmark:

```sh
RSS_FILE_UPLOAD_REQUESTS=512 \
RSS_FILE_UPLOAD_PAYLOAD_BYTES=1048576 \
RSS_FILE_UPLOAD_CONCURRENCY=32 \
RSS_FILE_UPLOAD_DELAY_MS=5 \
cargo test --test file_upload_benchmark_e2e -- --ignored --nocapture
```

Enable phase tracing when you need to see where RSS async differs from the
hand-written Rust async client:

```sh
RSS_FILE_UPLOAD_TRACE=1 \
cargo test --test file_upload_benchmark_e2e -- --ignored --nocapture
```

With tracing enabled, the benchmark prints per-phase RSS-vs-Rust averages for
`connect`, `write_request`, `read_response`, and per-upload `total` time. It
also reports RSS runtime phases such as `task_group_join` and
`executor_run_pending` with elapsed time, poll count, and yield count.

The RSS and Rust async paths both use Tokio for actual socket IO and hit the
same server with the same request count and payload size. The benchmark reports
the Rust/RSS async RPS ratio so regressions show whether the difference is in
RSScript lowering/runtime scheduling rather than the server or network setup.

Representative local output:

```text
file upload benchmark: requests=256 payload_bytes=262144 concurrency=16 server_delay_ms=10 rss_async_rps=...
```

When `rust_to_rss_rps_ratio` stays near `1.0` and both async clients reach the
same `max_in_flight`, the bottleneck is the shared server/IO workload rather
than RSScript's async lowering. A ratio materially above `1.0` points at
RSScript runtime/lowering overhead.
