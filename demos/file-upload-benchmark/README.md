# File upload benchmark demo

This demo compares request throughput for the same upload workload:

- `async_upload_client`: Tokio client with bounded concurrency.
- `sync_upload_client`: blocking TCP client that uploads files sequentially.
- `mock_upload_server`: Tokio HTTP server that reads the full request body before responding.

Run it through the Rust test runner:

```sh
cargo test --test file_upload_benchmark_e2e -- --nocapture
```

The test runner starts the server, builds both clients, excludes build time from timing, and prints requests per second for each client. It also asserts that async requests overlap while sync requests stay sequential.
